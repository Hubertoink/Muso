use rodio::{Decoder, OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub enum Cmd {
    Play(String, u64),
    Pause,
    Resume,
    Stop,
}

/// Audio lives on its own thread because the output stream is not `Send`.
pub struct Player {
    tx: Mutex<Sender<Cmd>>,
}

impl Player {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = channel::<Cmd>();
        std::thread::Builder::new()
            .name("muso-audio".into())
            .spawn(move || {
                let (_stream, handle) = match OutputStream::try_default() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = app.emit("playback-error", format!("No audio device: {e}"));
                        while rx.recv().is_ok() {}
                        return;
                    }
                };
                let mut sink: Option<Sink> = None;
                let mut cover_intro = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(Cmd::Play(path, playback_id)) => {
                            cover_intro = None;
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                            let opened = File::open(&path)
                                .map_err(|e| e.to_string())
                                .and_then(|f| Decoder::new(BufReader::new(f)).map_err(|e| e.to_string()));
                            match (opened, Sink::try_new(&handle)) {
                                (Ok(src), Ok(s)) => {
                                    s.append(src);
                                    sink = Some(s);
                                    cover_intro = Some(playback_id);
                                }
                                (Err(e), _) => {
                                    let _ = app.emit("playback-error", e);
                                }
                                (_, Err(e)) => {
                                    let _ = app.emit("playback-error", e.to_string());
                                }
                            }
                        }
                        Ok(Cmd::Pause) => {
                            if let Some(s) = &sink {
                                s.pause();
                            }
                        }
                        Ok(Cmd::Resume) => {
                            if let Some(s) = &sink {
                                s.play();
                            }
                        }
                        Ok(Cmd::Stop) => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if sink.as_ref().is_some_and(|s| s.empty()) {
                                sink = None;
                                let _ = app.emit("track-ended", ());
                            } else if sink.as_ref().is_some_and(|s| s.get_pos() >= Duration::from_secs(10)) {
                                if let Some(id) = cover_intro.take() {
                                    let _ = app.emit("cover-intro-ended", id);
                                }
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("failed to spawn audio thread");
        Self { tx: Mutex::new(tx) }
    }

    pub fn send(&self, cmd: Cmd) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(cmd);
        }
    }
}
