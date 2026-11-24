use crate::spectrum::{Meter, SpectrumTap};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub enum Cmd {
    Play(String, u64),
    Pause,
    Resume,
    Stop,
    Spectrum(bool),
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
                        let _ = app.emit("playback-error", (None::<u64>, format!("No audio device: {e}")));
                        while rx.recv().is_ok() {}
                        return;
                    }
                };
                let mut sink: Option<Sink> = None;
                let mut cover_intro = None;
                let spectrum_on = Arc::new(AtomicBool::new(false));
                let mut meter = Meter::default();
                let mut current_id = 0;
                loop {
                    let metering = spectrum_on.load(Ordering::Relaxed);
                    match rx.recv_timeout(Duration::from_millis(if metering { 40 } else { 250 })) {
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
                                    meter = Meter::default();
                                    current_id = playback_id;
                                    let enabled = Arc::clone(&spectrum_on);
                                    let source = SpectrumTap::new(src, meter.clone()).periodic_access(
                                        Duration::from_millis(20), move |tap| tap.set_enabled(enabled.load(Ordering::Relaxed)));
                                    s.append(source);
                                    sink = Some(s);
                                    cover_intro = Some(playback_id);
                                    let _ = app.emit("track-started", playback_id);
                                }
                                (Err(e), _) => {
                                    let _ = app.emit("playback-error", (Some(playback_id), e));
                                }
                                (_, Err(e)) => {
                                    let _ = app.emit("playback-error", (Some(playback_id), e.to_string()));
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
                        Ok(Cmd::Spectrum(enabled)) => {
                            spectrum_on.store(enabled, Ordering::Relaxed);
                            if let Ok(mut slot) = meter.lock() { slot.take(); }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if metering && sink.as_ref().is_some_and(|s| !s.is_paused() && !s.empty()) {
                                let levels = meter.lock().ok().and_then(|mut slot| slot.take());
                                if let Some(levels) = levels {
                                    let _ = app.emit("audio-spectrum", (current_id, levels));
                                }
                            }
                            if sink.as_ref().is_some_and(|s| s.empty()) {
                                sink = None;
                                let _ = app.emit("track-ended", current_id);
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

#[cfg(test)]
mod tests {
    use super::Decoder;
    use std::io::Cursor;

    #[test]
    fn silent_mp3_channels_at_the_end_of_the_bit_reservoir_play_in_full() {
        // A 32 kbps MPEG-1 Layer III stereo frame has 68 bytes of main data.
        // The first channel uses all 544 bits. The remaining channels need zero bits.
        let mut frame = [0u8; 104];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x10, 0]);
        frame[6..8].copy_from_slice(&544u16.to_be_bytes());
        frame[36..].fill(0xff);
        let decoded = Decoder::new(Cursor::new(frame.repeat(8))).unwrap();
        assert_eq!(decoded.count(), 8 * 1152 * 2);
    }
}
