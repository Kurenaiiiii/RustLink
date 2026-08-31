use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct SpatialFilter {
    pub priority: u32,
    config: AnimatableConfig,
    active: bool,
}

impl SpatialFilter {
    pub fn new() -> Self {
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.0, 0.0, 0.0, 0.0, 1.0]),
            active: false,
        }
    }
}

impl AnimatableFilter for SpatialFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        let obj = settings.get("spatial").and_then(|v| v.as_object()).or_else(|| settings.as_object());
        if let Some(map) = obj {
            let mut vals = self.config.get_current().to_vec();
            if let Some(pos) = map.get("position").and_then(|v| v.as_array()) {
                for (i, v) in pos.iter().enumerate() {
                    if i < 3 {
                        if let Some(n) = v.as_f64() { vals[i] = n as f32; }
                    }
                }
            }
            for (field_name, idx) in &[("rotation", 3), ("intensity", 4)] {
                if let Some(val) = map.get(*field_name).and_then(|v| v.as_f64()) {
                    vals[*idx] = val as f32;
                }
            }
            self.config.set_values(&vals);
        }
        let c = self.config.get_current();
        self.active = c[0].abs() > 0.001 || c[1].abs() > 0.001 || c[2].abs() > 0.001;
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let x = c[0].clamp(-1.0, 1.0);
        let y = c[1].clamp(-1.0, 1.0);
        let z = c[2].clamp(-1.0, 1.0);
        if x.abs() < 0.001 && y.abs() < 0.001 && z.abs() < 0.001 { return; }
        let pan = x * 0.5 + 0.5;
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let l = frame[0];
            let r = frame[1];
            let mono = (l + r) * 0.5;
            frame[0] = clamp_sample(l * (1.0 - pan) + mono * pan * 0.5);
            frame[1] = clamp_sample(r * pan + mono * (1.0 - pan) * 0.5);
        }
    }

    fn flush(&mut self) -> Vec<f32> { Vec::new() }
}
