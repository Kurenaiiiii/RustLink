use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample, hash_noise};

pub struct PhonographFilter {
    pub priority: u32,
    config: AnimatableConfig,
    crackle_phase: f32,
    pop_phase: f32,
    lp_x1: [f32; 2],
    active: bool,
}

impl PhonographFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.0, 0.0, 0.0, 0.0]),
            crackle_phase: 0.0,
            pop_phase: 0.0,
            lp_x1: [0.0; 2],
            active: false,
        }
    }
}

impl AnimatableFilter for PhonographFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "phonograph", &[("crackleVolume", 0), ("popVolume", 1), ("humVolume", 2), ("lowPassSmoothing", 3)]);
        let c = self.config.get_current();
        self.active = c[0] > 0.001 || c[1] > 0.001 || c[2] > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let crackle_vol = c[0].clamp(0.0, 1.0);
        let pop_vol = c[1].clamp(0.0, 1.0);
        let hum_vol = c[2].clamp(0.0, 1.0);
        let lp_smooth = c[3].clamp(0.0, 1.0);
        if crackle_vol < 0.001 && pop_vol < 0.001 && hum_vol < 0.001 { return; }
        let hum_rate = 50.0 * 2.0 * PI / sample_rate;
        let rc = 1.0 / (2.0 * PI * (4000.0 + (1.0 - lp_smooth) * 16000.0));
        let dt = 1.0 / sample_rate;
        let lp_a = dt / (rc + dt);
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let hum = hum_vol * (self.crackle_phase * 50.0 * 2.0 * PI).sin();
            self.crackle_phase += hum_rate;
            if self.crackle_phase > 1.0 { self.crackle_phase -= 1.0; }
            let crackle = if hash_noise(self.crackle_phase) > 0.998 {
                crackle_vol * (hash_noise(self.crackle_phase + 0.5) * 2.0 - 1.0)
            } else { 0.0 };
            self.pop_phase += 0.0002;
            if self.pop_phase > 1.0 { self.pop_phase -= 1.0; }
            let pop = if hash_noise(self.pop_phase + 0.3) > 0.99995 {
                pop_vol * (hash_noise(self.pop_phase) * 2.0 - 1.0)
            } else { 0.0 };
            let noise = crackle + pop + hum;
            for ch in 0..2 {
                let v = frame[ch] + noise;
                let lp = self.lp_x1[ch] + lp_a * (v - self.lp_x1[ch]);
                self.lp_x1[ch] = lp;
                frame[ch] = clamp_sample(lp);
            }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.lp_x1 = [0.0; 2];
        Vec::new()
    }
}
