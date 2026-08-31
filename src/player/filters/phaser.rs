use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, BiquadState, allpass_coeffs, clamp_sample};

pub struct PhaserFilter {
    pub priority: u32,
    config: AnimatableConfig,
    lfo_phase: f32,
    allpass_states: Vec<Vec<BiquadState>>,
    active: bool,
}

impl PhaserFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.5, 0.7, 0.0, 800.0, 6.0]),
            lfo_phase: 0.0,
            allpass_states: vec![vec![BiquadState::new(), BiquadState::new()]; 6],
            active: false,
        }
    }
}

impl AnimatableFilter for PhaserFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "phaser", &[("rate", 0), ("depth", 1), ("feedback", 2), ("centerFreq", 3)]);
        self.active = self.config.get_current()[0] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let rate = c[0].clamp(0.0, 10.0);
        let depth = c[1].clamp(0.0, 1.0);
        let center = c[3].clamp(100.0, 4000.0);
        if rate <= 0.0 { return; }
        let dt = 1.0 / sample_rate;
        let freq_range = center * depth;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let mod_freq = center + freq_range * self.lfo_phase.sin();
            let (b0, b1, b2, a1, a2) = allpass_coeffs(mod_freq, 0.5);
            for ch in 0..2 {
                let mut v = frame[ch];
                for stage in self.allpass_states.iter_mut() {
                    v = stage[ch].process(v, b0, b1, b2, a1, a2);
                }
                frame[ch] = clamp_sample(v);
            }
            self.lfo_phase += dt * rate * 2.0 * PI;
            if self.lfo_phase > 2.0 * PI { self.lfo_phase -= 2.0 * PI; }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        for stage in self.allpass_states.iter_mut() {
            for st in stage.iter_mut() { st.reset(); }
        }
        Vec::new()
    }
}
