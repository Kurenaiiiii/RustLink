use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, BiquadState, bandpass_coeffs};

pub struct BandPassFilter {
    pub priority: u32,
    config: AnimatableConfig,
    state: Vec<BiquadState>,
    active: bool,
}

impl BandPassFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[1000.0, 1.0]),
            state: vec![BiquadState::new(), BiquadState::new()],
            active: false,
        }
    }
}

impl AnimatableFilter for BandPassFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "bandPass", &[("frequency", 0), ("bandwidth", 1)]);
        let c = self.config.get_current();
        self.active = c[0] > 10.0 && c[0] < 24000.0;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let freq = c[0].clamp(10.0, 24000.0);
        let bw = c[1].clamp(0.1, 4.0);
        if freq < 10.0 || freq > 24000.0 { return; }
        let (b0, b1, b2, a1, a2) = bandpass_coeffs(freq, bw);
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
