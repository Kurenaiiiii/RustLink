#![allow(dead_code)]

use crate::config::FadingSection;

#[derive(Debug, Clone, PartialEq)]
pub enum FadeCurve {
    Linear,
    Exponential,
    Sinusoidal,
}

impl FadeCurve {
    pub fn from_str(s: &str) -> Self {
        match s {
            "exponential" => Self::Exponential,
            "sinusoidal" | "sine" => Self::Sinusoidal,
            _ => Self::Linear,
        }
    }
    pub fn apply(&self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::Exponential => t * t,
            Self::Sinusoidal => (1.0 - (t * std::f32::consts::PI).cos()) / 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FadeDirection {
    In,
    Out,
}

pub struct VolumeFade {
    pub start_gain: f32,
    pub target_gain: f32,
    pub duration_ms: f32,
    pub elapsed_ms: f32,
    pub curve: FadeCurve,
    pub active: bool,
}

impl VolumeFade {
    pub fn new() -> Self {
        Self {
            start_gain: 1.0,
            target_gain: 1.0,
            duration_ms: 0.0,
            elapsed_ms: 0.0,
            curve: FadeCurve::Linear,
            active: false,
        }
    }

    pub fn trigger(&mut self, target_gain: f32, duration_ms: f32, curve: &str) {
        self.start_gain = self.target_gain;
        self.target_gain = target_gain.clamp(0.0, 1.0);
        self.duration_ms = duration_ms.max(0.0);
        self.elapsed_ms = 0.0;
        self.curve = FadeCurve::from_str(curve);
        self.active = self.duration_ms > 0.0 && (self.start_gain - self.target_gain).abs() > 0.001;

        if !self.active {
            self.start_gain = self.target_gain;
        }
    }

    pub fn process(&mut self, samples: &mut [f32], delta_ms: f32) {
        if !self.active {
            if (self.target_gain - 1.0).abs() > 0.001 {
                for s in samples.iter_mut() {
                    *s *= self.target_gain;
                }
            }
            return;
        }

        let sample_count = samples.len();
        if sample_count == 0 {
            return;
        }

        let prev_elapsed = self.elapsed_ms;
        let next_elapsed = (self.elapsed_ms + delta_ms).min(self.duration_ms);

        let progress_start = prev_elapsed / self.duration_ms;
        let progress_end = next_elapsed / self.duration_ms;

        let mapped_start = self.curve.apply(progress_start);
        let mapped_end = self.curve.apply(progress_end);
        let range = self.target_gain - self.start_gain;

        let gain_start = self.start_gain + range * mapped_start;
        let gain_end = self.start_gain + range * mapped_end;

        self.elapsed_ms = next_elapsed;
        if next_elapsed >= self.duration_ms {
            self.active = false;
            self.start_gain = self.target_gain;
        }

        let step = if sample_count > 1 {
            (gain_end - gain_start) / (sample_count - 1) as f32
        } else {
            0.0
        };

        for (i, s) in samples.iter_mut().enumerate() {
            *s *= gain_start + step * i as f32;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TapeAction {
    Start,
    Stop,
}

pub struct TapeFade {
    pub current_rate: f32,
    pub start_rate: f32,
    pub target_rate: f32,
    pub duration_ms: f32,
    pub elapsed_ms: f32,
    pub curve: FadeCurve,
    pub active: bool,
    read_pos: f64,
    buffer: Vec<f32>,
}

impl TapeFade {
    pub fn new() -> Self {
        Self {
            current_rate: 1.0,
            start_rate: 1.0,
            target_rate: 1.0,
            duration_ms: 0.0,
            elapsed_ms: 0.0,
            curve: FadeCurve::Sinusoidal,
            active: false,
            read_pos: 0.0,
            buffer: Vec::new(),
        }
    }

    pub fn trigger(&mut self, action: TapeAction, duration_ms: f32, curve: &str) {
        self.start_rate = self.current_rate;
        self.target_rate = match action {
            TapeAction::Start => 1.0,
            TapeAction::Stop => 0.01,
        };
        self.duration_ms = duration_ms.max(0.0);
        self.elapsed_ms = 0.0;
        self.curve = FadeCurve::from_str(curve);
        self.active = self.duration_ms > 0.0
            && (self.start_rate - self.target_rate).abs() > 0.001;
        self.read_pos = 0.0;

        if !self.active {
            self.current_rate = self.target_rate;
            self.start_rate = self.target_rate;
        }
    }

    pub fn process(
        &mut self,
        input: &[f32],
        output: &mut Vec<f32>,
        channels: usize,
        delta_ms: f32,
    ) {
        output.clear();
        if input.is_empty() {
            return;
        }

        self.buffer.extend_from_slice(input);
        let frames_in = self.buffer.len() / channels;

        if self.active {
            let frame_delta_ms = delta_ms / frames_in as f32;
            for _ in 0..frames_in {
                self.elapsed_ms += frame_delta_ms;
                let t = (self.elapsed_ms / self.duration_ms).min(1.0);
                let curve_t = self.curve.apply(t);
                self.current_rate =
                    self.start_rate + (self.target_rate - self.start_rate) * curve_t;

                if t >= 1.0 {
                    self.current_rate = self.target_rate;
                    self.active = false;
                    self.start_rate = self.target_rate;
                }

                let i_pos = self.read_pos.floor() as usize * channels;
                if i_pos + channels * 2 >= self.buffer.len() {
                    break;
                }

                let frac = self.read_pos - self.read_pos.floor();

                for c in 0..channels {
                    let p0 = *self.buffer.get(i_pos.wrapping_sub(channels).wrapping_add(c)).unwrap_or(&0.0);
                    let p1 = *self.buffer.get(i_pos + c).unwrap_or(&0.0);
                    let p2 = *self.buffer.get(i_pos + channels + c).unwrap_or(&0.0);
                    let p3 = *self.buffer.get(i_pos + channels * 2 + c).unwrap_or(&0.0);

                    let val = 0.5
                        * (2.0 * p1
                            + (-p0 + p2) * frac as f32
                            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * (frac * frac) as f32
                            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * (frac * frac * frac) as f32);

                    output.push(val);
                }

                self.read_pos += self.current_rate as f64;
            }
        } else {
            let rate = self.current_rate;
            let frames_out = (frames_in as f64 / rate as f64).ceil() as usize;

            for _ in 0..frames_out {
                let i_pos = self.read_pos.floor() as usize * channels;
                if i_pos + channels * 2 >= self.buffer.len() {
                    break;
                }

                let frac = self.read_pos - self.read_pos.floor();

                for c in 0..channels {
                    let p1 = *self.buffer.get(i_pos + c).unwrap_or(&0.0);
                    let p2 = *self.buffer.get(i_pos + channels + c).unwrap_or(&0.0);

                    let val = p1 + (p2 - p1) * frac as f32;
                    output.push(val);
                }

                self.read_pos += rate as f64;
            }
        }

        let consumed = self.read_pos.floor() as usize * channels;
        if consumed > 0 && consumed <= self.buffer.len() {
            self.buffer.drain(0..consumed);
            self.read_pos -= self.read_pos.floor();
        }
    }

    pub fn is_active(&self) -> bool {
        self.active || (self.current_rate - 1.0).abs() > 0.001
    }
}

pub fn get_fade_params(section: &FadingSection) -> (f32, String) {
    (section.duration as f32, section.kind.clone())
}
