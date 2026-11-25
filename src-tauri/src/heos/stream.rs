use crate::audio::AudioBus;
use audio2mp3_core::{Bitrate, EncoderConfig, Mp3Encoder};
use axum::{
    body::{Body, Bytes},
    http::{header, Response},
    routing::get,
    Router,
};
use rodio::source::UniformSourceIterator;
use std::io;
use std::net::IpAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Notify};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

const RATE: u32 = 44100;
const FRAMES: usize = 1152;

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
    server: tokio::task::JoinHandle<()>,
}
impl Session {
    pub async fn serve(ip: IpAddr, pid: i64, bus: AudioBus) -> Result<Self, String> {
        let listener = TcpListener::bind((ip, 0)).await.map_err(crate::err)?;
        let path = format!("/{}.mp3", uuid::Uuid::new_v4().simple());
        let url = format!(
            "http://{}{path}",
            listener.local_addr().map_err(crate::err)?
        );
        let id = fastrand::u64(1..);
        let clients = broadcast::channel::<Option<Bytes>>(96).0;
        let connected = Arc::new(Notify::new());
        let running = Arc::new(AtomicBool::new(true));
        let held = Arc::new(AtomicBool::new(false));
        let handler_clients = clients.clone();
        let signal = connected.clone();
        let router = Router::new().route(
            &path,
            get(move || {
                let frames = BroadcastStream::new(handler_clients.subscribe());
                signal.notify_one();
                async move {
                    let stream = frames
                        .take_while(|r| !matches!(r, Ok(None)))
                        .map(|r| r.map(|b| b.unwrap()).map_err(io::Error::other));
                    response(Body::from_stream(stream))
                }
            })
            .head(|| async { response(Body::empty()) }),
        );
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
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
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
        })
    }
    pub async fn activate(&self) -> Result<(), String> {
        tokio::time::timeout(Duration::from_secs(12), async {
            while self.clients.receiver_count() == 0 { self.connected.notified().await; }
        }).await.map_err(|_| "The speaker did not fetch the audio stream. Check the private-network firewall and Wi-Fi isolation.".to_owned())?;
        self.reserve();
        Ok(())
    }
    pub fn reserve(&self) {
        self.bus.select(self.id);
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
        self.server.abort();
    }
}
