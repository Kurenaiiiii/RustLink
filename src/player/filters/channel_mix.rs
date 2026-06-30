use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct ChannelMixFilter {
    pub priority: u32,
    config: AnimatableConfig,
    matrix: [f32; 4],
    active: bool,
}

impl ChannelMixFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[1.0, 0.0, 0.0, 1.0]),
            matrix: [1.0, 0.0, 0.0, 1.0],
            active: false,
        }
    }
}

impl AnimatableFilter for ChannelMixFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "channelMix", &[("leftToLeft", 0), ("leftToRight", 1), ("rightToLeft", 2), ("rightToRight", 3)]);
        let c = self.config.get_current();
        self.matrix = [c[0], c[1], c[2], c[3]];
        self.active = (c[0] - 1.0).abs() > 0.001
            || c[1].abs() > 0.001
            || c[2].abs() > 0.001
            || (c[3] - 1.0).abs() > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        if !self.active { return; }
        let [l2l, l2r, r2l, r2r] = self.matrix;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let l = frame[0];
            let r = frame[1];
            frame[0] = clamp_sample(l * l2l + r * r2l);
            frame[1] = clamp_sample(l * l2r + r * r2r);
        }
    }

    fn flush(&mut self) -> Vec<f32> { Vec::new() }
}
