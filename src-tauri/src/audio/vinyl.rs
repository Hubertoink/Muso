use rodio::{buffer::SamplesBuffer, Decoder, Source};
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;

struct Clip {
    channels: u16,
    rate: u32,
    samples: Vec<i16>,
}

fn decode(bytes: &'static [u8]) -> Clip {
    let decoder = Decoder::new(Cursor::new(bytes)).expect("bundled vinyl recording");
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let mut samples: Vec<_> = decoder.collect();
    // Both recordings have MP3 encoder silence before the needle touches down.
    let start = samples
        .chunks_exact(channels.into())
        .position(|frame| frame.iter().any(|s| s.unsigned_abs() > 12))
        .unwrap_or(0);
    samples.drain(..start * usize::from(channels));
    Clip {
        channels,
        rate,
        samples,
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lead {
    Drop,
    Change,
}

impl Lead {
    fn clip(self) -> &'static Clip {
        static DROP: OnceLock<Clip> = OnceLock::new();
        static CHANGE: OnceLock<Clip> = OnceLock::new();
        let (cache, bytes): (_, &[u8]) = match self {
            Self::Drop => (
                &DROP,
                include_bytes!("../../../assets/Muso_VinylDrop_Beginn.mp3"),
            ),
            Self::Change => (
                &CHANGE,
                include_bytes!("../../../assets/Muso_Needle_Change_LP.mp3"),
            ),
        };
        cache.get_or_init(|| {
            let mut clip = decode(bytes);
            let fade = clip.rate as usize / 100 * usize::from(clip.channels);
            for (i, sample) in clip.samples.iter_mut().rev().take(fade).enumerate() {
                *sample = (f32::from(*sample) * (i as f32 / fade as f32)) as i16;
            }
            clip
        })
    }

    pub(super) fn sound(self) -> SamplesBuffer<i16> {
        let clip = self.clip();
        SamplesBuffer::new(clip.channels, clip.rate, clip.samples.clone())
    }
}

// Decide at the audio boundary, so a record added during the last track can follow it.
pub(super) struct Change {
    sound: SamplesBuffer<i16>,
    armed: Arc<AtomicBool>,
    pub started: Arc<AtomicBool>,
    first: bool,
}

impl Change {
    pub(super) fn new(armed: Arc<AtomicBool>, started: Arc<AtomicBool>) -> Self {
        Self {
            sound: Lead::Change.sound(),
            armed,
            started,
            first: true,
        }
    }
}

impl Iterator for Change {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.first {
            if !self.armed.load(Ordering::Relaxed) {
                return None;
            }
            self.started.store(true, Ordering::Relaxed);
            self.first = false;
        }
        self.sound.next()
    }
}

impl Source for Change {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.sound.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.sound.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub(super) struct Groove {
    clip: &'static Clip,
    cursor: usize,
    elapsed: u64,
    seconds: Arc<AtomicU32>,
}

impl Groove {
    pub(super) fn new(seconds: Arc<AtomicU32>) -> Self {
        static LOOP: OnceLock<Clip> = OnceLock::new();
        let clip = LOOP.get_or_init(|| {
            let mut clip = decode(include_bytes!("../../../assets/Muso_Endneedle.mp3"));
            let stride = usize::from(clip.channels);
            let head = clip.rate as usize / 4 * stride;
            let overlap = clip.rate as usize / 20 * stride;
            let tail = clip.samples.len() - overlap;
            // Play the initial needle transient once. Blend the tail into the running groove.
            for i in 0..overlap {
                let mix = (i / stride) as f32 / (overlap / stride - 1) as f32;
                clip.samples[tail + i] = ((1.0 - mix) * f32::from(clip.samples[tail + i])
                    + mix * f32::from(clip.samples[head + i]))
                    as i16;
            }
            clip
        });
        Self {
            clip,
            cursor: 0,
            elapsed: 0,
            seconds,
        }
    }
}

impl Iterator for Groove {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        let per_second = u64::from(self.clip.rate) * u64::from(self.clip.channels);
        let limit = u64::from(self.seconds.load(Ordering::Relaxed)) * per_second;
        let remaining = limit.checked_sub(self.elapsed + 1)?;
        let value = self.clip.samples[self.cursor];
        self.cursor += 1;
        self.elapsed += 1;
        if self.cursor == self.clip.samples.len() {
            self.cursor = (self.clip.rate as usize / 4 + self.clip.rate as usize / 20)
                * usize::from(self.clip.channels);
        }
        let fade = per_second / 10;
        Some(if remaining < fade {
            (f32::from(value) * remaining as f32 / fade as f32) as i16
        } else {
            value
        })
    }
}

impl Source for Groove {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.clip.channels
    }
    fn sample_rate(&self) -> u32 {
        self.clip.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_change_fills_the_boundary_until_the_next_track_is_ready() {
        let (input, mut output) = rodio::queue::queue(false);
        let armed = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let track = |value| SamplesBuffer::new(2, 44100, vec![value; 2]);
        input.append(track(17));
        input.append(Groove::new(Arc::new(AtomicU32::new(0))));
        input.append(Change::new(armed.clone(), started.clone()));
        assert_eq!(output.next(), Some(17));
        armed.store(true, Ordering::Relaxed);
        assert_eq!(output.next(), Some(17));
        let mut change = Lead::Change.sound();
        assert_eq!(output.next(), change.next());
        assert!(started.load(Ordering::Relaxed));
        input.append(track(42));
        assert!(output.eq(change.chain([42, 42])));
    }

    #[test]
    fn bundled_recordings_decode_and_groove_loops_to_the_exact_deadline() {
        let drop = Lead::Drop.sound();
        assert!(drop.total_duration().unwrap() > Duration::from_secs(1));
        let seconds = Arc::new(AtomicU32::new(8));
        let mut groove = Groove::new(seconds.clone());
        let rate = usize::from(groove.channels()) * groove.sample_rate() as usize;
        assert!(groove.clip.samples.len() < rate * 4);
        assert_eq!(groove.by_ref().take(rate * 4).count(), rate * 4);
        seconds.store(5, Ordering::Relaxed);
        let ending: Vec<_> = groove.collect();
        assert_eq!(ending.len(), rate);
        assert_eq!(ending.last(), Some(&0));
    }
}
