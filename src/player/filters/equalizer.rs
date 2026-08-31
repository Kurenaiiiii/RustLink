use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_EQUALIZER, BiquadState, EQ_BANDS, peaking_eq_coeffs, clamp_sample};

pub struct EqualizerFilter {
    pub priority: u32,
    config: AnimatableConfig,
    filters_l: Vec<BiquadState>,
    filters_r: Vec<BiquadState>,
    coeffs: Vec<(f32, f32, f32, f32, f32)>,
    active: bool,
}

impl EqualizerFilter {
    pub fn new() -> Self {
        let defaults: Vec<f32> = vec![0.0; 15];
        let filters_l = (0..15).map(|_| BiquadState::new()).collect();
        let filters_r = (0..15).map(|_| BiquadState::new()).collect();
        let coeffs = EQ_BANDS.iter().map(|&f| peaking_eq_coeffs(f, 0.0, 1.0)).collect();
        Self {
            priority: PRIORITY_EQUALIZER,
            config: AnimatableConfig::new(&defaults),
            filters_l,
            filters_r,
            coeffs,
            active: false,
        }
    }

    fn recalc_coeffs(&mut self) {
        let gains = self.config.get_current();
        self.active = gains.iter().any(|&g| g.abs() > 0.001);
        for (i, &freq) in EQ_BANDS.iter().enumerate() {
            self.coeffs[i] = peaking_eq_coeffs(freq, gains[i], 1.0);
        }
    }
}

impl AnimatableFilter for EqualizerFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        let obj = settings.get("equalizer").and_then(|v| v.as_object()).or_else(|| settings.as_object());
        if let Some(map) = obj {
            let mut gains = self.config.get_current().to_vec();
            for i in 0..15 {
                let key = format!("band_{}", i);
                if let Some(val) = map.get(&key).and_then(|v| v.as_f64()) {
                    gains[i] = val as f32;
                }
            }
            self.config.set_values(&gains);
        }
        self.recalc_coeffs();
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        let changed = self.config.process_animation(sample_rate, chunk.len(), 2);
        if changed { self.recalc_coeffs(); }
        if !self.active { return; }

        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let mut left = frame[0];
            let mut right = frame[1];
            for j in 0..15 {
                let (b0, b1, b2, a1, a2) = self.coeffs[j];
                left = self.filters_l[j].process(left, b0, b1, b2, a1, a2);
                right = self.filters_r[j].process(right, b0, b1, b2, a1, a2);
            }
            frame[0] = clamp_sample(left);
            frame[1] = clamp_sample(right);
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        for f in self.filters_l.iter_mut() { f.reset(); }
        for f in self.filters_r.iter_mut() { f.reset(); }
        Vec::new()
    }
}
