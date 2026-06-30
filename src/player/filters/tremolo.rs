use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct TremoloFilter {
    pub priority: u32,
    config: AnimatableConfig,
    phase: f32,
    active: bool,
}

impl TremoloFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[5.0, 0.5]),
            phase: 0.0,
            active: false,
        }
    }
}

impl AnimatableFilter for TremoloFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "tremolo", &[("frequency", 0), ("depth", 1)]);
        self.active = self.config.get_current()[1] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let freq = self.config.get_current()[0];
        let depth = self.config.get_current()[1];
        if depth <= 0.001 { return; }
        let dt = 1.0 / sample_rate;
        for sample in chunk.iter_mut() {
            let mod_v = 1.0 - depth * 0.5 * (1.0 - self.phase.cos());
            *sample = clamp_sample(*sample * mod_v);
            self.phase += dt * freq * 2.0 * PI;
            if self.phase > 2.0 * PI { self.phase -= 2.0 * PI; }
        }
    }

    fn flush(&mut self) -> Vec<f32> { Vec::new() }
}
