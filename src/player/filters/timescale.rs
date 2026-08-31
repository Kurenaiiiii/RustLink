use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_TIMESCALE, clamp_sample};

pub struct TimescaleFilter {
    pub priority: u32,
    config: AnimatableConfig,
    pending: Vec<f32>,
    bypass: bool,
    uses_stretch: bool,
}

impl TimescaleFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_TIMESCALE,
            config: AnimatableConfig::new(&[1.0, 1.0, 1.0]),
            pending: Vec::new(),
            bypass: true,
            uses_stretch: false,
        }
    }

    pub fn get_rate(&self) -> f32 {
        let c = self.config.get_current();
        c[0] * c[2]
    }
}

impl AnimatableFilter for TimescaleFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { !self.bypass || self.config.is_animating() }

    fn get_rate(&self) -> Option<f32> { Some(self.get_rate()) }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "timescale", &[("speed", 0), ("pitch", 1), ("rate", 2)]);
        let c = self.config.get_current();
        let speed = c[0];
        let pitch = c[1];
        let rate = c[2];
        self.bypass = (speed - 1.0).abs() < f32::EPSILON
            && (pitch - 1.0).abs() < f32::EPSILON
            && (rate - 1.0).abs() < f32::EPSILON;
        self.uses_stretch = (speed - rate).abs() > 0.001 || (pitch - rate).abs() > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), channels);
        if self.bypass { return; }
        let c = self.config.get_current();
        let speed = c[0];
        let rate = c[2];
        if !self.uses_stretch {
            let ratio = speed / rate;
            if (ratio - 1.0).abs() > 0.001 {
                for sample in chunk.iter_mut() {
                    *sample = clamp_sample(*sample * ratio);
                }
            }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.pending.drain(..).collect()
    }
}
