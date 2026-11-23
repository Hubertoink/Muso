use rodio::{source::SeekError, Source};
use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Levels = [f32; 16];
pub type Meter = Arc<Mutex<Option<Levels>>>;

// Constant-Q bandpass filters spaced logarithmically between 45 Hz and 16 kHz.
struct Band {
    b: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
    energy: f32,
}

impl Band {
    fn new(index: usize, rate: u32) -> Self {
        let hz = 45.0 * (16000.0_f32 / 45.0).powf(index as f32 / 15.0);
        let omega = TAU * (hz / rate as f32).min(0.49);
        let alpha = omega.sin() / 5.0;
        let norm = 1.0 / (1.0 + alpha);
        Self { b: if hz < rate as f32 * 0.5 { alpha * norm } else { 0.0 },
            a1: -2.0 * omega.cos() * norm, a2: (1.0 - alpha) * norm,
            z1: 0.0, z2: 0.0, energy: 0.0 }
    }

    fn sample(&mut self, x: f32) {
        let bx = self.b * x;
        let y = bx + self.z1;
        self.z1 = self.z2 - self.a1 * y;
        self.z2 = -bx - self.a2 * y;
        self.energy += y * y;
    }
}

pub struct SpectrumTap<S> {
    source: S,
    meter: Meter,
    bands: [Band; 16],
    enabled: bool,
    channels: u16,
    rate: u32,
    channel: u16,
    mix: f32,
    frames: u32,
}

impl<S: Source<Item = i16>> SpectrumTap<S> {
    pub fn new(source: S, meter: Meter) -> Self {
        let rate = source.sample_rate();
        Self { channels: source.channels(), rate, source, meter,
            bands: std::array::from_fn(|i| Band::new(i, rate)), enabled: false,
            channel: 0, mix: 0.0, frames: 0 }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        let rate = self.source.sample_rate();
        let channels = self.source.channels();
        if enabled && (!self.enabled || rate != self.rate || channels != self.channels) {
            self.channels = channels;
            self.rate = rate;
            self.bands = std::array::from_fn(|i| Band::new(i, rate));
            self.frames = 0;
            self.channel = 0;
            self.mix = 0.0;
        }
        self.enabled = enabled;
    }
}

impl<S: Source<Item = i16>> Iterator for SpectrumTap<S> {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let value = self.source.next()?;
        if self.enabled {
            self.mix += f32::from(value);
            self.channel += 1;
            if self.channel == self.channels {
                let mono = self.mix / (32768.0 * f32::from(self.channels));
                for band in &mut self.bands { band.sample(mono); }
                self.channel = 0;
                self.mix = 0.0;
                self.frames += 1;
                if self.frames >= self.rate / 25 {
                    let levels = std::array::from_fn(|i| {
                        let power = std::mem::take(&mut self.bands[i].energy) / self.frames as f32;
                        ((10.0 * power.max(1e-10).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
                    });
                    // Audio delivery never waits for the UI.
                    if let Ok(mut slot) = self.meter.try_lock() { *slot = Some(levels); }
                    self.frames = 0;
                }
            }
        }
        Some(value)
    }
}

impl<S: Source<Item = i16>> Source for SpectrumTap<S> {
    fn current_frame_len(&self) -> Option<usize> { self.source.current_frame_len() }
    fn channels(&self) -> u16 { self.source.channels() }
    fn sample_rate(&self) -> u32 { self.source.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.source.total_duration() }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> { self.source.try_seek(pos) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn unaltered_samples_drive_the_correct_band_and_silence_stays_flat() {
        for (hz, expected) in [(0.0, None), (145.0, Some(3)), (5030.0, Some(12))] {
            let pcm: Vec<_> = (0..48000).map(|n| (16000.0 * (TAU * hz * n as f32 / 48000.0).sin()) as i16).collect();
            let meter = Meter::default();
            let mut tap = SpectrumTap::new(SamplesBuffer::new(1, 48000, pcm.clone()), meter.clone());
            tap.set_enabled(true);
            assert_eq!(tap.collect::<Vec<_>>(), pcm);
            let levels = meter.lock().unwrap().unwrap();
            if let Some(index) = expected {
                let peak = (0..16).max_by(|a,b| levels[*a].total_cmp(&levels[*b])).unwrap();
                assert_eq!(peak, index);
            } else { assert_eq!(levels, [0.0; 16]); }
        }
    }
}
