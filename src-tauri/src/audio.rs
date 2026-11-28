use crate::spectrum::{Meter, SpectrumTap};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{atomic::{AtomicBool, AtomicU32, Ordering}, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
mod vinyl;
mod output;
pub(crate) use output::AudioBus;
pub use vinyl::Lead;

pub enum Cmd {
    Play { path: String, duration: Option<Duration>, id: u64, lead: Option<Lead>, after: Option<u64>, last_track: bool, runout_seconds: u32, change: bool },
    Runout(u64, u32, bool),
    VinylVolume(u8),
    Pause,
    Resume,
    Stop,
    Spectrum(bool),
}

/// Audio lives on its own thread because the output stream is not `Send`.
pub struct Player {
    tx: Mutex<Sender<Cmd>>,
    meter_windows: Mutex<std::collections::HashSet<String>>,
    pub(crate) bus: AudioBus,
}

fn music_source<S: Source<Item = i16>>(
    source: S, library_duration: Option<Duration>, meter: Meter,
    spectrum_on: Arc<AtomicBool>, ending: Arc<AtomicBool>,
) -> impl Source<Item = i16> {
    let mut reveal_at = source.total_duration().filter(|d| !d.is_zero())
        .or(library_duration).filter(|d| !d.is_zero())
        .map(|d| d.saturating_sub(Duration::from_secs(2)));
    // Count only music samples, excluding pauses and the vinyl lead-in.
    SpectrumTap::new(source, meter).track_position().periodic_access(Duration::from_millis(20), move |track| {
        track.inner_mut().set_enabled(spectrum_on.load(Ordering::Relaxed));
        if reveal_at.is_some_and(|at| track.get_pos() >= at) {
            ending.store(true, Ordering::Relaxed);
            reveal_at = None;
        }
    })
}

impl Player {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = channel::<Cmd>();
        let bus = AudioBus::new();
        let playback_bus = bus.clone();
        std::thread::Builder::new()
            .name("muso-audio".into())
            .spawn(move || {
                let local = OutputStream::try_default().map_err(crate::err).and_then(|(stream, handle)| {
                    handle.play_raw(playback_bus.reader(0)).map_err(crate::err)?;
                    Ok(stream)
                });
                let mut sink: Option<Sink> = None;
                let mut cover_intro = None;
                let mut track_ending = Arc::new(AtomicBool::new(false));
                let spectrum_on = Arc::new(AtomicBool::new(false));
                let mut meter = Meter::default();
                let mut current_id = 0;
                let mut groove_duration = Arc::new(AtomicU32::new(0));
                let vinyl_volume = Arc::new(AtomicU32::new(100));
                let mut groove_started = Arc::new(AtomicBool::new(false));
                let mut change_armed = Arc::new(AtomicBool::new(false));
                let mut change_started = Arc::new(AtomicBool::new(false));
                let mut handing_off = false;
                loop {
                    let metering = spectrum_on.load(Ordering::Relaxed);
                    match rx.recv_timeout(Duration::from_millis(if metering || change_armed.load(Ordering::Relaxed) { 40 } else { 250 })) {
                        Ok(Cmd::Play { path, duration, id: playback_id, lead, after, last_track, runout_seconds, change }) => {
                            if let Err(e) = &local {
                                if playback_bus.local() {
                                    let _ = app.emit("playback-error", (Some(playback_id), format!("No local audio device: {e}")));
                                    continue;
                                }
                            }
                            cover_intro = None;
                            let continuing = handing_off && after == Some(current_id) && sink.is_some();
                            let outgoing = if continuing { None } else { sink.take() };
                            handing_off = false;
                            let opened = File::open(&path)
                                .map_err(|e| e.to_string())
                                .and_then(|f| Decoder::new(BufReader::new(f)).map_err(|e| e.to_string()));
                            match opened {
                                Ok(src) => {
                                    let s = sink.take().unwrap_or_else(|| playback_bus.sink());
                                    meter = Meter::default();
                                    current_id = playback_id;
                                    track_ending = Arc::new(AtomicBool::new(false));
                                    let source = music_source(src, duration, meter.clone(), spectrum_on.clone(), track_ending.clone());
                                    groove_duration = Arc::new(AtomicU32::new(runout_seconds));
                                    groove_started = Arc::new(AtomicBool::new(false));
                                    change_armed = Arc::new(AtomicBool::new(change && last_track));
                                    change_started = Arc::new(AtomicBool::new(false));
                                    // Prepare the adjacent sources before starting the audio device queue.
                                    if !continuing {
                                        s.pause();
                                        if let Some(sound) = lead { s.append(vinyl::with_volume(sound.sound(), &vinyl_volume)); }
                                    }
                                    s.append(source);
                                    if last_track {
                                        let marker = Arc::clone(&groove_started);
                                        s.append(rodio::source::EmptyCallback::<i16>::new(Box::new(move || {
                                            marker.store(true, Ordering::Relaxed);
                                        })));
                                        let groove = vinyl::Groove::new(Arc::clone(&groove_duration));
                                        let change = vinyl::Change::new(Arc::clone(&change_armed), Arc::clone(&change_started));
                                        s.append(vinyl::with_volume(groove, &vinyl_volume));
                                        s.append(vinyl::with_volume(change, &vinyl_volume));
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
                                Err(e) => {
                                    let _ = app.emit("playback-error", (Some(playback_id), e));
                                }
                            }
                        }
                        Ok(Cmd::Runout(id, seconds, change)) => {
                            if current_id == id {
                                change_armed.store(change, Ordering::Relaxed);
                                groove_duration.store(seconds, Ordering::Relaxed);
                            }
                        }
                        Ok(Cmd::VinylVolume(percent)) => vinyl_volume.store(u32::from(percent), Ordering::Relaxed),
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
                            if track_ending.swap(false, Ordering::Relaxed) && sink.is_some() {
                                let _ = app.emit("track-ending", current_id);
                            }
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
        Self { tx: Mutex::new(tx), meter_windows: Mutex::default(), bus }
    }

    pub fn set_meter(&self, window: &str, enabled: bool) {
        let mut windows = self.meter_windows.lock().unwrap();
        let was_active = !windows.is_empty();
        if enabled { windows.insert(window.to_owned()); } else { windows.remove(window); }
        let active = !windows.is_empty();
        if active != was_active { self.send(Cmd::Spectrum(active)); }
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
    fn the_cover_cue_fires_once_two_seconds_before_the_music_ends() {
        use super::*;
        let ending = Arc::new(AtomicBool::new(false));
        let samples = rodio::buffer::SamplesBuffer::new(2, 1000, vec![0i16; 10000]);
        let mut music = music_source(samples, None, Meter::default(), Default::default(), ending.clone());
        assert_eq!(music.by_ref().take(6000).count(), 6000);
        assert!(!ending.load(Ordering::Relaxed));
        music.next();
        assert!(ending.swap(false, Ordering::Relaxed));
        assert_eq!(music.count(), 3999);
        assert!(!ending.load(Ordering::Relaxed));
    }

    #[test]
    fn tv_keeps_the_meter_running_when_the_controller_stops_listening() {
        let (tx, rx) = super::channel();
        let player = super::Player {
            tx: std::sync::Mutex::new(tx),
            meter_windows: Default::default(),
            bus: super::AudioBus::with_format(2, 44100),
        };
        player.set_meter("main", true);
        assert!(matches!(rx.try_recv(), Ok(super::Cmd::Spectrum(true))));
        player.set_meter("presenter", true);
        player.set_meter("main", false);
        assert!(rx.try_recv().is_err());
        player.set_meter("presenter", false);
        assert!(matches!(rx.try_recv(), Ok(super::Cmd::Spectrum(false))));
    }

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
