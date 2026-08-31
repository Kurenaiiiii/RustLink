use crate::player::loudness::LoudnessNormalizer;

const DEFAULT_FADE_CURVE: FadeCurve = FadeCurve::Sinusoidal;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FadeCurve {
    Linear,
    Sine,
    Sinusoidal,
}

impl FadeCurve {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "linear" => FadeCurve::Linear,
            "sine" => FadeCurve::Sine,
            "sinusoidal" => FadeCurve::Sinusoidal,
            _ => DEFAULT_FADE_CURVE,
        }
    }

    fn value(&self, progress: f32) -> f32 {
        let clamped = progress.clamp(0.0, 1.0);
        match self {
            FadeCurve::Linear => clamped,
            FadeCurve::Sine | FadeCurve::Sinusoidal => {
                0.5 - 0.5 * (clamped * std::f32::consts::PI).cos()
            }
        }
    }
}

pub struct VolumeTransformer {
    pub sample_rate: u32,
    pub channels: usize,
    lookahead_samples: usize,
    lookahead_buffer: Vec<f32>,
    lookahead_index: usize,
    lookahead_full: bool,
    current_volume: f32,
    target_volume: f32,
    start_volume: f32,
    fade_duration_ms: f32,
    fade_frames_total: usize,
    fade_frames_elapsed: usize,
    fade_active: bool,
    fade_curve: FadeCurve,
    limiter_threshold: f32,
    limiter_softness: f32,
    threshold_value: f32,
    limit_headroom: f32,
    agc: Option<LoudnessNormalizer>,
}

impl VolumeTransformer {
    pub fn new(
        sample_rate: u32,
        channels: usize,
        volume: f32,
        fade_duration_ms: f32,
        fade_curve: FadeCurve,
        limiter_threshold: f32,
        limiter_softness: f32,
        enable_agc: bool,
        lookahead_ms: f32,
        gate_threshold_lufs: Option<f64>,
    ) -> Self {
        let sr = if sample_rate > 0 { sample_rate } else { 48000 };
        let ch = channels.max(1);
        let lookahead_samples = ((lookahead_ms.max(0.0) / 1000.0) * sr as f32).round() as usize * ch;
        let fade_frames_total = ((fade_duration_ms.max(0.0) / 1000.0) * sr as f32).round() as usize;
        let vol = if volume.is_finite() { volume } else { 1.0 };
        let threshold = limiter_threshold.clamp(0.0, 0.999);
        let softness = limiter_softness.max(0.01);

        Self {
            sample_rate: sr,
            channels: ch,
            lookahead_samples,
            lookahead_buffer: vec![0.0; lookahead_samples],
            lookahead_index: 0,
            lookahead_full: false,
            current_volume: vol,
            target_volume: vol,
            start_volume: vol,
            fade_duration_ms: fade_duration_ms.max(0.0),
            fade_frames_total,
            fade_frames_elapsed: fade_frames_total,
            fade_active: false,
            fade_curve,
            limiter_threshold: threshold,
            limiter_softness: softness,
            threshold_value: threshold,
            limit_headroom: 1.0 - threshold,
            agc: if enable_agc {
                Some(LoudnessNormalizer::new(
                    ch,
                    sr,
                    lookahead_ms.max(0.0) as u64,
                    -14.0,
                    gate_threshold_lufs.unwrap_or(-30.0),
                ))
            } else {
                None
            },
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        let next = if volume.is_finite() { volume } else { self.target_volume };
        if (next - self.target_volume).abs() < f32::EPSILON {
            return;
        }
        self.start_volume = self.current_volume;
        self.target_volume = next;
        self.fade_frames_elapsed = 0;
        self.fade_active = self.fade_frames_total > 0;
        if !self.fade_active {
            self.current_volume = next;
            self.start_volume = next;
        }
    }

    fn compute_fade_gains(&mut self, sample_count: usize) -> (f32, f32) {
        if !self.fade_active || self.fade_frames_total == 0 {
            self.current_volume = self.target_volume;
            return (self.target_volume, self.target_volume);
        }

        let frames = sample_count / self.channels;
        if frames == 0 {
            return (self.current_volume, self.current_volume);
        }

        let prev_elapsed = self.fade_frames_elapsed;
        let next_elapsed = self.fade_frames_total.min(prev_elapsed + frames);

        let progress_start = prev_elapsed as f32 / self.fade_frames_total as f32;
        let progress_end = next_elapsed as f32 / self.fade_frames_total as f32;

        let mapped_start = self.fade_curve.value(progress_start);
        let mapped_end = self.fade_curve.value(progress_end);
        let range = self.target_volume - self.start_volume;

        let gain_start = self.start_volume + range * mapped_start;
        let gain_end = self.start_volume + range * mapped_end;

        self.fade_frames_elapsed = next_elapsed;
        if next_elapsed >= self.fade_frames_total {
            self.fade_active = false;
            self.current_volume = self.target_volume;
            self.start_volume = self.target_volume;
        } else {
            self.current_volume = gain_end;
        }

        (gain_start, gain_end)
    }

    fn apply_limiter(&self, value: f32) -> f32 {
        let abs = value.abs();
        if abs <= self.threshold_value || self.limit_headroom <= 0.0 {
            return value;
        }
        let normalized_overshoot = (abs - self.threshold_value) / self.limit_headroom;
        let softened = 1.0 - (-normalized_overshoot * self.limiter_softness).exp();
        let limited = self.threshold_value + self.limit_headroom * softened;
        value.signum() * limited.min(1.0)
    }

    fn clamp_to_f32(value: f32) -> f32 {
        value.clamp(-1.0, 1.0)
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        let usable = samples.len();
        if usable == 0 {
            return;
        }

        if let Some(ref mut agc) = self.agc {
            agc.process(samples);
        }

        let (gain_start, gain_end) = self.compute_fade_gains(usable);
        let gain_step = if usable > 1 {
            (gain_end - gain_start) / (usable - 1) as f32
        } else {
            0.0
        };

        if self.lookahead_samples > 0 {
            let mut output = vec![0.0f32; usable];
            let mut gain = gain_start;
            for i in 0..usable {
                let raw = samples[i];
                let scaled = raw * gain;
                let limited = self.apply_limiter(scaled);

                let output_sample = self.lookahead_buffer[self.lookahead_index];
                self.lookahead_buffer[self.lookahead_index] = limited;
                self.lookahead_index = (self.lookahead_index + 1) % self.lookahead_samples;

                output[i] = Self::clamp_to_f32(output_sample);
                gain += gain_step;
            }
            if self.lookahead_index == 0 {
                self.lookahead_full = true;
            }
            samples.copy_from_slice(&output);
        } else {
            let mut gain = gain_start;
            for sample in samples.iter_mut() {
                let scaled = *sample * gain;
                let limited = self.apply_limiter(scaled);
                *sample = Self::clamp_to_f32(limited);
                gain += gain_step;
            }
        }
    }
}
