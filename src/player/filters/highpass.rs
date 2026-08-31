use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, BiquadState, highpass_coeffs};

pub struct HighPassFilter {
    pub priority: u32,
    config: AnimatableConfig,
    state: Vec<BiquadState>,
    active: bool,
    freq: f32,
}

impl HighPassFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[10.0]),
            state: vec![BiquadState::new(), BiquadState::new()],
            active: false,
            freq: 10.0,
        }
    }
}

impl AnimatableFilter for HighPassFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "highpass", &[("frequency", 0)]);
        self.freq = self.config.get_current()[0].clamp(10.0, 24000.0);
        self.active = self.freq > 10.0;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        if !self.active { return; }
        let (b0, b1, b2, a1, a2) = highpass_coeffs(self.freq);
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            frame[0] = self.state[0].process(frame[0], b0, b1, b2, a1, a2);
            frame[1] = self.state[1].process(frame[1], b0, b1, b2, a1, a2);
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        for s in self.state.iter_mut() { s.reset(); }
        Vec::new()
    }
}
