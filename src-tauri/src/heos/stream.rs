use crate::audio::AudioBus;
use audio2mp3_core::{Bitrate, EncoderConfig, Mp3Encoder};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, Method, Response, StatusCode},
    routing::get,
    Router,
};
use rodio::source::UniformSourceIterator;
use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use std::{collections::HashMap, net::SocketAddr};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Notify};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

const RATE: u32 = 44100;
const FRAMES: usize = 1152;
// A speaker opens an incoming connection to Muso. Linux firewalls need a stable destination.
pub(super) const STREAM_PORT: u16 = if cfg!(target_os = "linux") { 48125 } else { 0 };

#[derive(Clone)]
struct Feed {
    clients: broadcast::Sender<Option<Bytes>>,
    connected: Arc<Notify>,
}
type Feeds = Arc<Mutex<HashMap<String, Feed>>>;

pub(super) struct StreamServer {
    pub addr: SocketAddr,
    feeds: Feeds,
    task: tokio::task::JoinHandle<()>,
}

impl StreamServer {
    pub async fn bind(addr: SocketAddr) -> Result<Arc<Self>, String> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Could not listen for HEOS audio on {addr}: {e}"))?;
        let addr = listener.local_addr().map_err(crate::err)?;
        let feeds = Feeds::default();
        let router = Router::new()
            .route("/{file}", get(read_stream))
            .with_state(feeds.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Arc::new(Self { addr, feeds, task }))
    }
}
impl Drop for StreamServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_stream(
    State(feeds): State<Feeds>,
    Path(file): Path<String>,
    method: Method,
) -> Result<Response<Body>, StatusCode> {
    let feed = feeds
        .lock()
        .unwrap()
        .get(&file)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    if method == Method::HEAD {
        return Ok(response(Body::empty()));
    }
    let frames = BroadcastStream::new(feed.clients.subscribe());
    feed.connected.notify_one();
    let stream = frames
        .take_while(|r| !matches!(r, Ok(None)))
        .map(|r| r.map(|b| b.unwrap()).map_err(io::Error::other));
    Ok(response(Body::from_stream(stream)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Transport {
    Ready,
    Playing,
    Paused,
}

/// A session exposes one live URL, with no filesystem routes. All HTTP readers
/// share the encoded frames; a group or reconnect never clocks the mix twice.
pub(super) struct Session {
    pub pid: i64,
    pub id: u64,
    pub url: String,
    pub transport: Transport,
    bus: AudioBus,
    clients: broadcast::Sender<Option<Bytes>>,
    connected: Arc<Notify>,
    running: Arc<AtomicBool>,
    held: Arc<AtomicBool>,
    pub server: Arc<StreamServer>,
    file: String,
}
impl Session {
    pub fn serve(server: Arc<StreamServer>, pid: i64, bus: AudioBus) -> Result<Self, String> {
        let file = format!("{}.mp3", uuid::Uuid::new_v4().simple());
        let url = format!("http://{}/{file}", server.addr);
        let id = fastrand::u64(1..);
        let clients = broadcast::channel::<Option<Bytes>>(96).0;
        let connected = Arc::new(Notify::new());
        let running = Arc::new(AtomicBool::new(true));
        let held = Arc::new(AtomicBool::new(false));
        let config = EncoderConfig {
            channels: 2,
            sample_rate: RATE,
            bitrate: Bitrate::Kbps320,
        };
        let mut encoder = Mp3Encoder::new(config).map_err(crate::err)?;
        let source = bus.reader(id);
        let sending = running.clone();
        let hold = held.clone();
        let destination = clients.clone();
        std::thread::Builder::new()
            .name("muso-heos-mp3".into())
            .spawn(move || {
                let mut pcm = UniformSourceIterator::<_, i16>::new(source, 2, RATE);
                let mut buffer = [0; FRAMES * 2];
                let interval = Duration::from_secs_f64(FRAMES as f64 / f64::from(RATE));
                let mut next = Instant::now();
                while sending.load(Ordering::Acquire) {
                    if hold.load(Ordering::Acquire) || destination.receiver_count() == 0 {
                        std::thread::sleep(interval);
                        next = Instant::now();
                        continue;
                    }
                    for sample in &mut buffer {
                        *sample = pcm.next().unwrap_or(0);
                    }
                    let mut bytes = Vec::new();
                    if encoder.encode(&buffer, &mut bytes).is_err() {
                        break;
                    }
                    if !bytes.is_empty() {
                        let _ = destination.send(Some(Bytes::from(bytes)));
                    }
                    next += interval;
                    let now = Instant::now();
                    if now > next + interval {
                        next = now;
                    }
                    std::thread::sleep(next.saturating_duration_since(now));
                }
                sending.store(false, Ordering::Release);
                let _ = destination.send(None);
            })
            .map_err(crate::err)?;
        server.feeds.lock().unwrap().insert(
            file.clone(),
            Feed {
                clients: clients.clone(),
                connected: connected.clone(),
            },
        );
        Ok(Self {
            pid,
            id,
            url,
            transport: Transport::Ready,
            bus,
            clients,
            connected,
            running,
            held,
            server,
            file,
        })
    }
    pub async fn activate(&self) -> Result<(), String> {
        tokio::time::timeout(Duration::from_secs(12), async {
            while self.clients.receiver_count() == 0 {
                self.connected.notified().await;
            }
        })
        .await
        .map_err(|_| "The speaker did not fetch the audio stream.".to_owned())?;
        self.reserve();
        Ok(())
    }
    pub fn reserve(&self) {
        self.bus.select(self.id);
    }
    pub fn connection_error(&self, reason: &str) -> String {
        format!("{reason} The speaker needs to reach this computer at {}. Check incoming TCP from the speaker and Wi-Fi client isolation.", self.server.addr)
    }
    pub fn hold(&self, value: bool) {
        self.held.store(value, Ordering::Release);
    }
    pub fn listening(&self) -> bool {
        self.running.load(Ordering::Acquire) && self.clients.receiver_count() > 0
    }
}
fn response(body: Body) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCEPT_RANGES, "none")
        .body(body)
        .unwrap()
}
impl Drop for Session {
    fn drop(&mut self) {
        self.bus.release(self.id);
        self.running.store(false, Ordering::Release);
        let _ = self.clients.send(None);
        self.server.feeds.lock().unwrap().remove(&self.file);
    }
}
