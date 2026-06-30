use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, SAMPLE_RATE, clamp_sample};

pub struct EchoFilter {
    pub priority: u32,
    config: AnimatableConfig,
    buffer: Vec<f32>,
    write_pos: usize,
    max_samples: usize,
    active: bool,
}

impl EchoFilter {
    pub fn new() -> Self {
        let max_samples = (2.0 * SAMPLE_RATE) as usize;
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.5, 0.5, 2.0]),
            buffer: vec![0.0; max_samples * 2],
            write_pos: 0,
            max_samples,
            active: false,
        }
    }
}

impl AnimatableFilter for EchoFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "echo", &[("delay", 0), ("decay", 1)]);
        self.active = self.config.get_current()[1] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let delay = c[0].clamp(0.001, 2.0);
        let decay = c[1].clamp(0.0, 1.0);
        if decay <= 0.001 { return; }
        let delay_samples = (delay * sample_rate) as usize;
        let buf_len = self.buffer.len();
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let mut new_samples = [0.0_f32; 2];
            for ch in 0..2 {
                let buf_idx = self.write_pos * 2 + ch;
                let read_pos = (buf_idx + buf_len - delay_samples * 2) % buf_len;
                let delayed = self.buffer[read_pos];
                let out = clamp_sample(frame[ch] + delayed * decay);
                self.buffer[buf_idx % buf_len] = out;
                new_samples[ch] = out;
            }
            frame[0] = new_samples[0];
            frame[1] = new_samples[1];
            self.write_pos = (self.write_pos + 1) % (buf_len / 2);
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        let remaining: Vec<f32> = self.buffer[self.write_pos * 2..].iter().copied()
            .chain(self.buffer[..self.write_pos * 2].iter().copied())
            .collect();
        self.buffer.fill(0.0);
        self.write_pos = 0;
        remaining
    }
}
