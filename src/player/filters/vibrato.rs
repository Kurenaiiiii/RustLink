use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, SAMPLE_RATE, clamp_sample};

pub struct VibratoFilter {
    pub priority: u32,
    config: AnimatableConfig,
    phase: f32,
    buffer: Vec<f32>,
    write_pos: usize,
    max_delay_samples: usize,
    active: bool,
}

impl VibratoFilter {
    pub fn new() -> Self {
        let max_delay = (0.01 * SAMPLE_RATE) as usize;
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[5.0, 0.5]),
            phase: 0.0,
            buffer: vec![0.0; max_delay * 2],
            write_pos: 0,
            max_delay_samples: max_delay,
            active: false,
        }
    }
}

impl AnimatableFilter for VibratoFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "vibrato", &[("frequency", 0), ("depth", 1)]);
        self.active = self.config.get_current()[1] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let freq = self.config.get_current()[0];
        let depth = self.config.get_current()[1];
        if depth <= 0.001 { return; }
        let dt = 1.0 / sample_rate;
        let depth_samples = depth * self.max_delay_samples as f32;
        let buf_len = self.buffer.len();
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            for ch in 0..2 {
                let idx = (self.write_pos * 2 + ch) % buf_len;
                self.buffer[idx] = frame[ch];
            }
            let mod_delay = 0.5 * depth_samples * (1.0 - self.phase.cos());
            let delay_samples = mod_delay as usize;
            let frac = mod_delay - delay_samples as f32;
            let mut out = [0.0_f32; 2];
            for ch in 0..2 {
                let read_pos = ((self.write_pos * 2 + ch).wrapping_sub(delay_samples * 2)) % buf_len;
                let s0 = self.buffer[read_pos];
                let s1 = self.buffer[(read_pos + 2) % buf_len];
                out[ch] = clamp_sample(s0 + (s1 - s0) * frac);
            }
            frame[0] = out[0];
            frame[1] = out[1];
            self.write_pos = (self.write_pos + 1) % (buf_len / 2);
            self.phase += dt * freq * 2.0 * PI;
            if self.phase > 2.0 * PI { self.phase -= 2.0 * PI; }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        Vec::new()
    }
}
