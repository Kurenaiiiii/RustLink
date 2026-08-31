use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, SAMPLE_RATE, clamp_sample};

pub struct FlangerFilter {
    pub priority: u32,
    config: AnimatableConfig,
    buffer: Vec<f32>,
    write_pos: usize,
    phase: f32,
    max_delay: usize,
    active: bool,
}

impl FlangerFilter {
    pub fn new() -> Self {
        let max_delay = (0.015 * SAMPLE_RATE) as usize;
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.003, 0.002, 0.5, 0.5]),
            buffer: vec![0.0; max_delay * 2],
            write_pos: 0,
            phase: 0.0,
            max_delay,
            active: false,
        }
    }
}

impl AnimatableFilter for FlangerFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "flanger", &[("delay", 0), ("depth", 1), ("rate", 2), ("feedback", 3)]);
        self.active = self.config.get_current()[2] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let delay = c[0].clamp(0.001, 0.015);
        let depth = c[1].clamp(0.0, 0.005);
        let rate = c[2].clamp(0.0, 10.0);
        let feedback = c[3].clamp(0.0, 1.0);
        if rate <= 0.001 { return; }
        let dt = 1.0 / sample_rate;
        let delay_samples = (delay * sample_rate) as usize;
        let depth_samples = (depth * sample_rate) as usize;
        let buf_len = self.buffer.len();
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            for ch in 0..2 {
                let idx = (self.write_pos * 2 + ch) % buf_len;
                let feedback_val = self.buffer[idx] * feedback;
                self.buffer[idx] = frame[ch] + feedback_val;
            }
            let mod_delay = delay_samples as f32 + depth_samples as f32 * 0.5 * (1.0 - self.phase.cos());
            let d = mod_delay as usize;
            let frac = mod_delay - d as f32;
            let mut out = [0.0_f32; 2];
            for ch in 0..2 {
                let read_pos = (self.write_pos * 2 + ch + buf_len - d * 2) % buf_len;
                let s0 = self.buffer[read_pos];
                let s1 = self.buffer[(read_pos + 2) % buf_len];
                let delayed = s0 + (s1 - s0) * frac;
                out[ch] = clamp_sample(frame[ch] + delayed * 0.6);
            }
            frame[0] = out[0];
            frame[1] = out[1];
            self.write_pos = (self.write_pos + 1) % (buf_len / 2);
            self.phase += dt * rate * 2.0 * PI;
            if self.phase > 2.0 * PI { self.phase -= 2.0 * PI; }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        Vec::new()
    }
}
