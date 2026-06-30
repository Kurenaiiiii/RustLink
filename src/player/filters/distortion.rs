use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct DistortionFilter {
    pub priority: u32,
    config: AnimatableConfig,
    sin_scale: f32, sin_offset: f32,
    cos_scale: f32, cos_offset: f32,
    tan_scale: f32, tan_offset: f32,
    offset: f32, scale: f32,
    active: bool,
}

impl DistortionFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
            sin_scale: 0.0, sin_offset: 0.0,
            cos_scale: 0.0, cos_offset: 0.0,
            tan_scale: 0.0, tan_offset: 0.0,
            offset: 0.0, scale: 1.0,
            active: false,
        }
    }
}

impl AnimatableFilter for DistortionFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "distortion", &[("sinOffset", 0), ("sinScale", 1), ("cosOffset", 2), ("cosScale", 3), ("tanOffset", 4), ("tanScale", 5), ("offset", 6), ("scale", 7)]);
        let c = self.config.get_current();
        self.sin_scale = c[0]; self.sin_offset = c[1];
        self.cos_scale = c[2]; self.cos_offset = c[3];
        self.tan_scale = c[4]; self.tan_offset = c[5];
        self.offset = c[6]; self.scale = c[7].max(0.001);
        self.active = self.sin_scale.abs() > 0.001 || self.cos_scale.abs() > 0.001
            || self.tan_scale.abs() > 0.001 || self.offset.abs() > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        if !self.active { return; }
        for sample in chunk.iter_mut() {
            let mut v = *sample / self.scale + self.offset;
            v = v.sin() * self.sin_scale
                + v.cos() * self.cos_scale
                + v.tan() * self.tan_scale;
            *sample = clamp_sample(v);
        }
    }

    fn flush(&mut self) -> Vec<f32> { Vec::new() }
}
