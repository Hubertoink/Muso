use crate::spectrum::{Meter, SpectrumTap};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{atomic::{AtomicBool, AtomicU32, Ordering}, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
mod vinyl;
pub use vinyl::Lead;

pub enum Cmd {
    Play { path: String, id: u64, lead: Option<Lead>, after: Option<u64>, last_track: bool, runout_seconds: u32, change: bool },
    Runout(u64, u32, bool),
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
                let mut groove_duration = Arc::new(AtomicU32::new(0));
                let mut groove_started = Arc::new(AtomicBool::new(false));
                let mut change_armed = Arc::new(AtomicBool::new(false));
                let mut change_started = Arc::new(AtomicBool::new(false));
                let mut handing_off = false;
                loop {
                    let metering = spectrum_on.load(Ordering::Relaxed);
                    match rx.recv_timeout(Duration::from_millis(if metering || change_armed.load(Ordering::Relaxed) { 40 } else { 250 })) {
                        Ok(Cmd::Play { path, id: playback_id, lead, after, last_track, runout_seconds, change }) => {
                            cover_intro = None;
                            let continuing = handing_off && after == Some(current_id) && sink.is_some();
                            let outgoing = if continuing { None } else { sink.take() };
                            let output = match sink.take() {
                                Some(s) => Ok(s),
                                None => Sink::try_new(&handle),
                            };
                            handing_off = false;
                            let opened = File::open(&path)
                                .map_err(|e| e.to_string())
                                .and_then(|f| Decoder::new(BufReader::new(f)).map_err(|e| e.to_string()));
                            match (opened, output) {
                                (Ok(src), Ok(s)) => {
                                    meter = Meter::default();
                                    current_id = playback_id;
                                    let enabled = Arc::clone(&spectrum_on);
                                    let source = SpectrumTap::new(src, meter.clone()).periodic_access(
                                        Duration::from_millis(20), move |tap| tap.set_enabled(enabled.load(Ordering::Relaxed)));
                                    groove_duration = Arc::new(AtomicU32::new(runout_seconds));
                                    groove_started = Arc::new(AtomicBool::new(false));
                                    change_armed = Arc::new(AtomicBool::new(change && last_track));
                                    change_started = Arc::new(AtomicBool::new(false));
                                    // Prepare the adjacent sources before starting the audio device queue.
                                    if !continuing {
                                        s.pause();
                                        if let Some(sound) = lead { s.append(sound.sound()); }
                                    }
                                    s.append(source);
                                    if last_track {
                                        let marker = Arc::clone(&groove_started);
                                        s.append(rodio::source::EmptyCallback::<i16>::new(Box::new(move || {
                                            marker.store(true, Ordering::Relaxed);
                                        })));
                                        s.append(vinyl::Groove::new(Arc::clone(&groove_duration)));
                                        s.append(vinyl::Change::new(Arc::clone(&change_armed), Arc::clone(&change_started)));
                                    }
                                    s.play();
                                    if let Some(old) = outgoing {
                                        if matches!(lead, Some(Lead::Change)) && !old.empty() && !old.is_paused() {
                                            // Ease the playing record out under the needle sound on a manual change.
                                            for step in (0..4).rev() {
                                                old.set_volume(step as f32 / 4.0);
                                                std::thread::sleep(Duration::from_millis(6));
                                            }
                                        }
                                        old.stop();
                                    }
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
                        Ok(Cmd::Runout(id, seconds, change)) => {
                            if current_id == id {
                                change_armed.store(change, Ordering::Relaxed);
                                groove_duration.store(seconds, Ordering::Relaxed);
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
                            if change_started.swap(false, Ordering::Relaxed) {
                                handing_off = true;
                                let _ = app.emit("record-changing", current_id);
                            }
                            if groove_started.swap(false, Ordering::Relaxed) && groove_duration.load(Ordering::Relaxed) > 0 {
                                let _ = app.emit("runout-started", current_id);
                            }
                            if metering && sink.as_ref().is_some_and(|s| !s.is_paused() && !s.empty()) {
                                let levels = meter.lock().ok().and_then(|mut slot| slot.take());
                                if let Some(levels) = levels {
                                    let _ = app.emit("audio-spectrum", (current_id, levels));
                                }
                            }
                            if !handing_off && sink.as_ref().is_some_and(|s| s.empty()) {
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
