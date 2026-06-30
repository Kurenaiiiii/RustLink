use std::f32::consts::PI;
use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::PRIORITY_DEFAULT;

pub struct KaraokeFilter {
    pub priority: u32,
    config: AnimatableConfig,
    alpha: f32,
    active: bool,
    prev_gain: f32,
    lp_x1: [f32; 2],
    hp_x1: [f32; 2],
}

impl KaraokeFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.0]),
            alpha: 0.0,
            active: false,
            prev_gain: f32::MAX,
            lp_x1: [0.0; 2],
            hp_x1: [0.0; 2],
        }
    }

    fn reset_state(&mut self) {
        self.lp_x1 = [0.0; 2];
        self.hp_x1 = [0.0; 2];
        self.prev_gain = f32::MAX;
    }
}

impl AnimatableFilter for KaraokeFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.alpha > 0.001 || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "karaoke", &[("level", 0)]);
        self.alpha = self.config.get_current()[0].clamp(0.0, 1.0);
        self.active = self.alpha > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let alpha = self.alpha;
        if alpha <= 0.001 { return; }

        let fc = 240.0;
        let rc = 1.0 / (2.0 * PI * fc);
        let dt = 1.0 / sample_rate;
        let lp_a = dt / (rc + dt);
        let hp_a = rc / (rc + dt);

        let mut max_gain: f32 = 0.0;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let l = frame[0];
            let r = frame[1];
            let mid = (l + r) * 0.5;
            let side = (l - r) * 0.5;

            let lp = self.lp_x1[0] + lp_a * (mid - self.lp_x1[0]);
            self.lp_x1[0] = lp;
            let hp_mid = self.hp_x1[0] + hp_a * (mid - self.hp_x1[0]);
            self.hp_x1[0] = hp_mid;
            let band = mid - hp_mid;

            let filtered_side = side - lp * alpha;
            let out_l = filtered_side + band * (1.0 - alpha);
            let out_r = -filtered_side + band * (1.0 - alpha);

            let gain = (out_l.abs().max(out_r.abs())).max(1e-6);
            if gain > max_gain { max_gain = gain; }

            frame[0] = out_l;
            frame[1] = out_r;
        }

        if max_gain > 1.0 {
            let scale = 1.0 / max_gain;
            for frame in chunk.chunks_mut(2) {
                if frame.len() < 2 { continue; }
                frame[0] *= scale;
                frame[1] *= scale;
            }
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        self.reset_state();
        Vec::new()
    }
}
