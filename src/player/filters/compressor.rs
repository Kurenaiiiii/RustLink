use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_COMPRESSOR, clamp_sample};

pub struct CompressorFilter {
    pub priority: u32,
    config: AnimatableConfig,
    env: [f32; 2],
    active: bool,
}

impl CompressorFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_COMPRESSOR,
            config: AnimatableConfig::new(&[-30.0, 4.0, 10.0, 100.0, 0.0]),
            env: [0.0; 2],
            active: false,
        }
    }
}

impl AnimatableFilter for CompressorFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "compressor", &[("threshold", 0), ("ratio", 1), ("attack", 2), ("release", 3)]);
        let c = self.config.get_current();
        let threshold = c[0];
        self.active = threshold > -50.0 && c[1] > 1.0;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let threshold_db = c[0];
        let ratio = c[1].max(1.0);
        let attack_ms = c[2].max(0.1);
        let release_ms = c[3].max(1.0);
        if threshold_db < -50.0 || ratio <= 1.0 { return; }
        let threshold_lin = 10.0_f32.powf(threshold_db / 20.0);
        let attack = (-1.0 / (attack_ms * 0.001 * sample_rate)).exp();
        let release = (-1.0 / (release_ms * 0.001 * sample_rate)).exp();
        let slope = 1.0 - 1.0 / ratio;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            for ch in 0..2 {
                let abs_in = frame[ch].abs();
                let coeff = if abs_in > self.env[ch] { attack } else { release };
                self.env[ch] = self.env[ch] + coeff * (abs_in - self.env[ch]);
                if self.env[ch] > threshold_lin {
                    let gain_db = (self.env[ch] / threshold_lin).log10() * 20.0 * slope;
                    let gain_lin = 10.0_f32.powf(gain_db / 20.0);
                    frame[ch] = clamp_sample(frame[ch] * (1.0 - gain_lin));
                }
            }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.env = [0.0; 2];
        Vec::new()
    }
}
