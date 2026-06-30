use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct RotationFilter {
    pub priority: u32,
    config: AnimatableConfig,
    phase: f32,
    active: bool,
}

impl RotationFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.0]),
            phase: 0.0,
            active: false,
        }
    }
}

impl AnimatableFilter for RotationFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "rotation", &[("rotationHz", 0)]);
        self.active = self.config.get_current()[0] > 0.0;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let hz = self.config.get_current()[0];
        if hz <= 0.0 { return; }
        let dt = 1.0 / sample_rate;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let l = frame[0];
            let r = frame[1];
            let cos_v = self.phase.cos();
            let sin_v = self.phase.sin();
            frame[0] = clamp_sample(l * cos_v - r * sin_v);
            frame[1] = clamp_sample(l * sin_v + r * cos_v);
            self.phase += dt * hz * 2.0 * PI;
            if self.phase > 2.0 * PI { self.phase -= 2.0 * PI; }
        }
    }

    fn flush(&mut self) -> Vec<f32> { Vec::new() }
}
