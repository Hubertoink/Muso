use rodio::{buffer::SamplesBuffer, Decoder, Source};
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;

pub(super) fn with_volume<S: Source<Item = i16>>(source: S, percent: &Arc<AtomicU32>) -> impl Source<Item = i16> {
    let percent = Arc::clone(percent);
    source.amplify(1.0).periodic_access(Duration::from_millis(10), move |amp| {
        amp.set_factor(percent.load(Ordering::Relaxed) as f32 / 100.0);
    })
}

struct Clip {
    channels: u16,
    rate: u32,
    samples: Vec<i16>,
}

fn decode(bytes: &'static [u8]) -> Clip {
    let decoder = Decoder::new(Cursor::new(bytes)).expect("bundled vinyl recording");
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    let mut samples: Vec<_> = decoder.collect();
    // Trim MP3 encoder silence before the needle touches down.
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

// All six lead recordings are stereo at 44.1 kHz. Prepare them off the
// mixer thread so an armed change can choose at the actual record boundary.
fn recordings() -> &'static [Clip; 6] {
    static CLIPS: OnceLock<[Clip; 6]> = OnceLock::new();
    const BYTES: [&[u8]; 6] = [
        include_bytes!("../../../assets/Muso_VinylDrop_Beginn.mp3"),
        include_bytes!("../../../assets/Muso_Needle_Change_LP.mp3"),
        include_bytes!("../../../assets/Muso_Needle_drop2.mp3"),
        include_bytes!("../../../assets/Muso_Needle_drop3.mp3"),
        include_bytes!("../../../assets/Muso_Needle_drop4.mp3"),
        include_bytes!("../../../assets/Muso_Needle_drop5.mp3"),
    ];
    CLIPS.get_or_init(|| BYTES.map(|bytes| {
        let mut clip = decode(bytes);
        let fade = clip.rate as usize / 100 * usize::from(clip.channels);
        for (i, sample) in clip.samples.iter_mut().rev().take(fade).enumerate() {
            *sample = (f32::from(*sample) * (i as f32 / fade as f32)) as i16;
        }
        clip
    }))
}

impl Lead {
    fn choose(self, previous: Option<usize>) -> usize {
        let mut choices = [match self { Self::Drop => 0, Self::Change => 1 }, 2, 3, 4, 5];
        let mut count = choices.len();
        if let Some(index) = choices.iter().position(|id| Some(*id) == previous) {
            count -= 1;
            choices.swap(index, count);
        }
        choices[fastrand::usize(..count)]
    }

    fn clip(self) -> &'static Clip {
        static LAST: Mutex<Option<usize>> = Mutex::new(None);
        let clips = recordings();
        let mut previous = LAST.lock().unwrap();
        let index = self.choose(*previous);
        *previous = Some(index);
        &clips[index]
    }

    pub(super) fn sound(self) -> SamplesBuffer<i16> {
        let clip = self.clip();
        SamplesBuffer::new(clip.channels, clip.rate, clip.samples.clone())
    }
}

// Decide at the audio boundary, so an unused change doesn't consume a drop.
pub(super) struct Change {
    samples: Option<std::slice::Iter<'static, i16>>,
    armed: Arc<AtomicBool>,
    pub started: Arc<AtomicBool>,
}

impl Change {
    pub(super) fn new(armed: Arc<AtomicBool>, started: Arc<AtomicBool>) -> Self {
        recordings();
        Self { samples: None, armed, started }
    }
}

impl Iterator for Change {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        if self.samples.is_none() {
            if !self.armed.load(Ordering::Relaxed) { return None; }
            self.samples = Some(Lead::Change.clip().samples.iter());
            self.started.store(true, Ordering::Relaxed);
        }
        self.samples.as_mut()?.next().copied()
    }
}

impl Source for Change {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { recordings()[1].channels }
    fn sample_rate(&self) -> u32 { recordings()[1].rate }
    fn total_duration(&self) -> Option<Duration> { None }
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
    fn vinyl_volume_changes_during_playback_without_shortening_the_sound() {
        let percent = Arc::new(AtomicU32::new(100));
        let source = SamplesBuffer::new(1, 1000, vec![1000i16; 30]);
        let mut sound = with_volume(source, &percent);
        for level in [100, 25, 0] {
            percent.store(level, Ordering::Relaxed);
            assert_eq!(sound.by_ref().take(10).collect::<Vec<_>>(), vec![(level * 10) as i16; 10]);
        }
        assert_eq!(sound.next(), None);
    }

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
        let first = output.next().unwrap();
        assert!(started.load(Ordering::Relaxed));
        input.append(track(42));
        let rendered: Vec<_> = std::iter::once(first).chain(output).collect();
        let (change, next_track) = rendered.split_at(rendered.len() - 2);
        assert_eq!(next_track, [42, 42]);
        assert!(recordings()[1..].iter().any(|clip| change == clip.samples));
    }

    #[test]
    fn bundled_recordings_decode_and_groove_loops_to_the_exact_deadline() {
        for clip in recordings() {
            assert_eq!((clip.channels, clip.rate), (2, 44100));
            assert!(clip.samples.len() > 4410);
        }
        for previous in 0..6 {
            for lead in [Lead::Drop, Lead::Change] {
                assert!((0..64).all(|_| lead.choose(Some(previous)) != previous));
            }
        }
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
