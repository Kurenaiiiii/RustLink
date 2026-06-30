use std::f32::consts::PI;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AnimationCurve {
    Linear,
    Exponential,
    Sinusoidal,
}

impl Default for AnimationCurve {
    fn default() -> Self {
        Self::Sinusoidal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnimationTransition {
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub curve: AnimationCurve,
}

#[derive(Debug, Clone, Default)]
struct AnimationState {
    elapsed_ms: f64,
    duration_ms: f64,
    curve: AnimationCurve,
}

#[derive(Debug, Clone)]
pub struct AnimatableConfig {
    current: Vec<f32>,
    target: Vec<f32>,
    start: Vec<f32>,
    animation: Option<AnimationState>,
    defaults: Vec<f32>,
    config_changed: bool,
}

impl AnimatableConfig {
    pub fn new(defaults: &[f32]) -> Self {
        let current = defaults.to_vec();
        let target = defaults.to_vec();
        let start = defaults.to_vec();
        Self {
            current,
            target,
            start,
            animation: None,
            defaults: defaults.to_vec(),
            config_changed: false,
        }
    }

    /// Extract transition and _disabled from a raw JSON object.
    fn extract_transition(obj: &serde_json::Map<String, Value>) -> (Option<AnimationTransition>, bool) {
        let transition = obj.get("transition")
            .and_then(|v| serde_json::from_value::<AnimationTransition>(v.clone()).ok());
        let disabled = obj.get("_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        (transition, disabled)
    }

    /// Update with named values from a JSON object.
    /// `settings` may be either `{ "filterName": { ... } }` (wrapped) or `{ ... }` (flat).
    /// `config_key` is the filter name (e.g. "timescale", "equalizer").
    /// `field_map` maps parameter names to indices in the config vector: `&[("speed", 0), ("pitch", 1), ("rate", 2)]`
    pub fn apply_animated_update(
        &mut self,
        settings: &Value,
        config_key: &str,
        field_map: &[(&str, usize)],
    ) {
        // Try wrapped mode first: settings[config_key], then fall back to flat mode
        let obj = settings.get(config_key)
            .and_then(|v| v.as_object())
            .or_else(|| settings.as_object());

        let obj = match obj {
            Some(o) => o,
            None => return,
        };

        let (transition, disabled) = Self::extract_transition(obj);

        let mut new_target = self.current.clone();

        if disabled {
            new_target.copy_from_slice(&self.defaults);
        } else {
            for (field_name, idx) in field_map {
                if *idx < new_target.len() {
                    if let Some(val) = obj.get(*field_name).and_then(|v| v.as_f64()) {
                        new_target[*idx] = val as f32;
                    }
                }
            }
        }

        if let Some(transition) = transition {
            if transition.duration_ms > 0 {
                self.animation = Some(AnimationState {
                    elapsed_ms: 0.0,
                    duration_ms: transition.duration_ms as f64,
                    curve: transition.curve,
                });
                self.start = self.current.clone();
                self.target = new_target;
                for i in 0..self.current.len().min(self.defaults.len()) {
                    if self.start[i].is_nan() || self.start[i].is_infinite() {
                        self.start[i] = self.defaults[i];
                        self.current[i] = self.defaults[i];
                    }
                }
                self.config_changed = true;
                return;
            }
        }

        self.animation = None;
        self.current = new_target.clone();
        self.target = new_target;
        self.config_changed = true;
    }

    /// Instant-set all values (no animation).
    pub fn set_values(&mut self, values: &[f32]) {
        self.animation = None;
        self.current = values.to_vec();
        self.target = values.to_vec();
        self.start = values.to_vec();
        self.config_changed = true;
    }

    pub fn process_animation(
        &mut self,
        sample_rate: f32,
        chunk_len: usize,
        channels: usize,
    ) -> bool {
        let mut changed = false;
        if let Some(anim) = &mut self.animation {
            let num_samples = chunk_len / 2;
            let num_frames = num_samples / channels.max(1);
            let chunk_duration_ms = (num_frames as f64 / sample_rate as f64) * 1000.0;

            anim.elapsed_ms += chunk_duration_ms;

            if anim.elapsed_ms >= anim.duration_ms {
                self.current = self.target.clone();
                self.animation = None;
                changed = true;
            } else {
                let t = anim.elapsed_ms / anim.duration_ms;
                let curve_t = Self::curve_value(t as f32, anim.curve);

                for i in 0..self.current.len().min(self.target.len()) {
                    let start = self.start[i];
                    let target = self.target[i];
                    self.current[i] = start + (target - start) * curve_t;
                }
                changed = true;
            }
        }
        self.config_changed = changed;
        changed
    }

    fn curve_value(t: f32, curve: AnimationCurve) -> f32 {
        match curve {
            AnimationCurve::Linear => t,
            AnimationCurve::Exponential => t * t,
            AnimationCurve::Sinusoidal => (1.0 - (t * PI).cos()) / 2.0,
        }
    }

    pub fn is_animating(&self) -> bool {
        self.animation.is_some()
    }

    pub fn get_current(&self) -> &[f32] {
        &self.current
    }

    pub fn set_instant(&mut self, values: &[f32]) {
        self.animation = None;
        self.current = values.to_vec();
        self.target = values.to_vec();
        self.start = values.to_vec();
        self.config_changed = true;
    }

    pub fn take_config_changed(&mut self) -> bool {
        std::mem::take(&mut self.config_changed)
    }
}

pub trait AnimatableFilter: Send + Sync {
    fn priority(&self) -> u32;
    fn process(&mut self, chunk: &mut [f32], channels: usize, sample_rate: f32);
    fn update(&mut self, settings: &Value);
    fn flush(&mut self) -> Vec<f32>;
    fn is_active(&self) -> bool;
    fn get_rate(&self) -> Option<f32> { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_animatable_config() {
        let mut config = AnimatableConfig::new(&[1.0, 0.5, 2.0]);
        assert_eq!(config.get_current(), &[1.0, 0.5, 2.0]);
    }

    #[test]
    fn test_curve_values() {
        assert_eq!(AnimatableConfig::curve_value(0.0, AnimationCurve::Linear), 0.0);
        assert_eq!(AnimatableConfig::curve_value(1.0, AnimationCurve::Linear), 1.0);
        assert_eq!(AnimatableConfig::curve_value(0.5, AnimationCurve::Exponential), 0.25);
        assert!((AnimatableConfig::curve_value(0.5, AnimationCurve::Sinusoidal) - 0.5).abs() < 0.001);
    }
}