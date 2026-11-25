use rodio::cpal::{
    default_host,
    traits::{DeviceTrait, HostTrait},
};
use rodio::dynamic_mixer::{self, DynamicMixer, DynamicMixerController};
use rodio::{Sink, Source};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BLOCK_FRAMES: usize = 128;

/// Exactly one output clocks the mix. Switching destinations keeps the live sinks,
/// their offsets, vinyl sources and spectrum taps in place.
#[derive(Clone)]
pub(crate) struct AudioBus {
    input: Arc<DynamicMixerController<f32>>,
    mix: Arc<Mutex<DynamicMixer<f32>>>,
    route: Arc<AtomicU64>,
    local_volume: Arc<AtomicU8>,
    channels: u16,
    rate: u32,
}

impl AudioBus {
    pub fn new() -> Self {
        let format = default_host()
            .default_output_device()
            .and_then(|d| d.default_output_config().ok());
        let (channels, rate) = format.map_or((2, 44100), |f| (f.channels(), f.sample_rate().0));
        Self::with_format(channels, rate)
    }

    pub(crate) fn with_format(channels: u16, rate: u32) -> Self {
        let (input, mix) = dynamic_mixer::mixer(channels, rate);
        Self {
            input,
            mix: Arc::new(Mutex::new(mix)),
            route: Arc::new(AtomicU64::new(0)),
            local_volume: Arc::new(AtomicU8::new(100)),
            channels,
            rate,
        }
    }

    pub fn sink(&self) -> Sink {
        let (sink, source) = Sink::new_idle();
        self.input.add(source);
        sink
    }

    pub fn select(&self, id: u64) {
        self.route.store(id, Ordering::Release);
    }
    pub fn release(&self, id: u64) {
        let _ = self
            .route
            .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn local(&self) -> bool {
        self.route.load(Ordering::Acquire) == 0
    }
    pub fn local_volume(&self) -> u8 {
        self.local_volume.load(Ordering::Relaxed)
    }
    pub fn set_local_volume(&self, value: u8) {
        self.local_volume.store(value.min(100), Ordering::Relaxed);
    }
    pub fn reader(&self, id: u64) -> impl Source<Item = f32> + Send + 'static {
        let buffer = vec![0.; BLOCK_FRAMES * usize::from(self.channels)];
        Reader {
            bus: self.clone(),
            id,
            pos: buffer.len(),
            gain: 1.0,
            buffer,
        }
    }
}

struct Reader {
    bus: AudioBus,
    id: u64,
    buffer: Vec<f32>,
    pos: usize,
    gain: f32,
}
impl Iterator for Reader {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos == self.buffer.len() {
            if self.bus.route.load(Ordering::Acquire) == self.id {
                let mut mix = self.bus.mix.lock().unwrap();
                for sample in &mut self.buffer {
                    *sample = mix.next().unwrap_or(0.);
                }
                if self.id == 0 {
                    let target = f32::from(self.bus.local_volume()) / 100.;
                    let step = (target - self.gain) / BLOCK_FRAMES as f32;
                    if target != 1. || self.gain != target {
                        for frame in self.buffer.chunks_mut(usize::from(self.bus.channels)) {
                            self.gain += step;
                            for sample in frame {
                                *sample *= self.gain;
                            }
                        }
                    }
                    self.gain = target;
                }
            } else {
                self.buffer.fill(0.);
            }
            self.pos = 0;
        }
        let sample = self.buffer[self.pos];
        self.pos += 1;
        Some(sample)
    }
}
impl Source for Reader {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.bus.channels
    }
    fn sample_rate(&self) -> u32 {
        self.bus.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_volume_scales_the_mix_and_leaves_the_heos_feed_at_full_level() {
        let bus = AudioBus::with_format(2, 44100);
        bus.input.add(rodio::buffer::SamplesBuffer::new(2, 44100, vec![0.5; 4096]));
        let mut local = bus.reader(0);
        bus.set_local_volume(25);
        assert_eq!(local.nth(BLOCK_FRAMES * 2), Some(0.125));
        bus.set_local_volume(0);
        assert_eq!(local.nth(BLOCK_FRAMES * 4), Some(0.0));
        bus.select(9);
        assert_eq!(bus.reader(9).next(), Some(0.5));
        bus.release(9);
        assert_eq!(bus.local_volume(), 0);
    }
}
