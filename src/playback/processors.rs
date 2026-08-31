use std::collections::VecDeque;
use std::time::Instant;

use crate::player::fading::FadeCurve;

const SAMPLE_RATE: f32 = 48000.0;

// --- SilenceDetector ---

pub struct SilenceDetector {
    threshold: f32,
    min_silence_ms: u64,
    silence_samples: u64,
    sample_rate: u32,
    is_silent: bool,
    in_silence: bool,
}

impl SilenceDetector {
    pub fn new(threshold_db: f32, min_silence_ms: u64, sample_rate: u32) -> Self {
        let threshold = 10.0_f32.powf(threshold_db / 20.0);
        Self {
            threshold,
            min_silence_ms,
            silence_samples: 0,
            sample_rate,
            is_silent: false,
            in_silence: false,
        }
    }

    pub fn process(&mut self, samples: &[f32]) -> bool {
        for &sample in samples {
            if sample.abs() < self.threshold {
                self.silence_samples += 1;
            } else {
                self.silence_samples = 0;
            }
        }

        let required = (self.min_silence_ms * self.sample_rate as u64) / 1000;
        self.is_silent = self.silence_samples >= required;

        if self.is_silent && !self.in_silence {
            self.in_silence = true;
            return true;
        }
        if !self.is_silent {
            self.in_silence = false;
            if self.silence_samples == 0 && self.silence_samples >= required {
            }
        }
        false
    }

    pub fn is_silent(&self) -> bool {
        self.is_silent
    }

    pub fn reset(&mut self) {
        self.silence_samples = 0;
        self.is_silent = false;
        self.in_silence = false;
    }
}

// --- ScratchTransformer ---

#[derive(Clone)]
pub struct ScratchSettings {
    pub frequency: f32,
    pub amplitude: f32,
    pub position: f32,
}

pub struct ScratchTransformer {
    phase: f32,
    write_pos: usize,
    buffer: Vec<f32>,
    buffer_channels: usize,
}

impl ScratchTransformer {
    pub fn new(max_delay_ms: f32, channels: usize) -> Self {
        let buffer_size = (max_delay_ms * SAMPLE_RATE / 1000.0) as usize * channels;
        Self {
            phase: 0.0,
            write_pos: 0,
            buffer: vec![0.0; buffer_size.max(1)],
            buffer_channels: channels,
        }
    }

    pub fn process(&mut self, samples: &mut [f32], channels: usize, settings: &ScratchSettings) {
        let c = channels.min(self.buffer_channels);
        if c == 0 || self.buffer.len() < c {
            return;
        }

        let scr = settings.amplitude
            * (2.0 * std::f32::consts::PI * settings.frequency * self.phase).sin()
            * settings.position;
        let direction = if scr > 0.0 { 1.0 } else { -1.0 };
        let speed = scr.abs().clamp(0.0, 1.0);
        let buf_len = self.buffer.len() / c;
        if buf_len == 0 {
            return;
        }

        let frame_count = samples.len() / channels;

        for i in 0..frame_count {
            let base = i * channels;

            for ch in 0..c {
                self.buffer[self.write_pos * c + ch] = samples[base + ch];
            }

            let read_pos_f = if direction > 0.0 {
                (self.write_pos as f32 - speed * buf_len as f32).rem_euclid(buf_len as f32)
            } else {
                (self.write_pos as f32 + speed * buf_len as f32).rem_euclid(buf_len as f32)
            };
            let read_pos = read_pos_f as usize % buf_len;
            let frac = read_pos_f - read_pos as f32;
            let next_pos = (read_pos + 1) % buf_len;

            for ch in 0..c {
                let delayed = self.buffer[read_pos * c + ch]
                    + frac * (self.buffer[next_pos * c + ch] - self.buffer[read_pos * c + ch]);
                samples[base + ch] = delayed * settings.amplitude
                    + samples[base + ch] * (1.0 - settings.amplitude);
            }

            self.write_pos = (self.write_pos + 1) % buf_len;
        }

        self.phase += 1.0 / SAMPLE_RATE;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }
}

// --- FlowController ---

pub struct FlowConfig {
    pub target_bitrate: u64,
    pub max_buffered_ms: u64,
    pub min_buffered_ms: u64,
}

pub struct FlowController {
    config: FlowConfig,
    buffer_size: u64,
    last_check: Instant,
    accumulated_pause: f64,
    pcm_queue: VecDeque<f32>,
    channels: usize,
}

pub struct FlowMetrics {
    pub buffer_size_ms: u64,
    pub paused: bool,
    pub adjusted_bitrate: u64,
}

impl FlowController {
    pub fn new(config: FlowConfig, channels: usize) -> Self {
        Self {
            config,
            buffer_size: 0,
            last_check: Instant::now(),
            accumulated_pause: 0.0,
            pcm_queue: VecDeque::new(),
            channels,
        }
    }

    pub fn push_pcm(&mut self, samples: &[f32]) {
        self.pcm_queue.extend(samples.iter().copied());
        self.buffer_size += samples.len() as u64 / self.channels.max(1) as u64;
    }

    pub fn drain_pcm(&mut self, output: &mut Vec<f32>, needed: usize) -> bool {
        let available = self.pcm_queue.len();
        let to_drain = needed.min(available);
        for _ in 0..to_drain {
            output.push(self.pcm_queue.pop_front().unwrap());
        }
        if to_drain > 0 {
            self.buffer_size = self.buffer_size.saturating_sub(
                to_drain as u64 / self.channels.max(1) as u64,
            );
        }
        to_drain == needed
    }

    pub fn read_pcm(&mut self, sample_count: usize) -> Vec<f32> {
        let elapsed = self.last_check.elapsed().as_secs_f64();
        self.last_check = Instant::now();

        let frame_rate = self.config.target_bitrate as f64
            / (16.0 * self.channels as f64); // 16-bit samples
        let expected_frames = (frame_rate * elapsed) as usize;
        let buffered_ms = self.buffer_size * 1000 / self.config.target_bitrate.max(1);

        let should_pause = buffered_ms > self.config.max_buffered_ms;
        if should_pause {
            self.accumulated_pause += elapsed;
        }

        let available = expected_frames
            - (self.accumulated_pause * frame_rate) as usize;
        let available = available.max(0).min(self.pcm_queue.len());
        let to_read = sample_count.min(available);

        let mut out = Vec::with_capacity(to_read);
        for _ in 0..to_read {
            if let Some(s) = self.pcm_queue.pop_front() {
                out.push(s);
            }
        }
        self.buffer_size = self.buffer_size.saturating_sub(to_read as u64 / self.channels.max(1) as u64);
        out
    }

    pub fn metrics(&self) -> FlowMetrics {
        let buffered_ms = self.buffer_size * 1000 / self.config.target_bitrate.max(1);
        FlowMetrics {
            buffer_size_ms: buffered_ms,
            paused: buffered_ms > self.config.max_buffered_ms,
            adjusted_bitrate: self.config.target_bitrate,
        }
    }

    pub fn reconfigure(&mut self, config: FlowConfig) {
        if config.target_bitrate != self.config.target_bitrate {
            let current_frames = self.pcm_queue.len() / self.channels.max(1);
            let current_ms = current_frames as u64 * 1000
                * 16 * self.channels as u64
                / self.config.target_bitrate.max(1);
            self.buffer_size = current_ms * config.target_bitrate / 1000;
        }
        self.config = config;
    }

    pub fn clear(&mut self) {
        self.pcm_queue.clear();
        self.buffer_size = 0;
        self.accumulated_pause = 0.0;
    }
}

// --- TapeTransformer ---

#[derive(Clone, Copy, PartialEq)]
pub enum TapeDirection {
    Forward,
    Reverse,
}

pub struct TapeTransformer {
    pub playback_rate: f32,
    pub target_rate: f32,
    pub direction: TapeDirection,
    ramp_samples: u32,
    sample_index: u32,
    channels: usize,
}

impl TapeTransformer {
    pub fn new(channels: usize) -> Self {
        Self {
            playback_rate: 1.0,
            target_rate: 1.0,
            direction: TapeDirection::Forward,
            ramp_samples: 0,
            sample_index: 0,
            channels,
        }
    }

    pub fn set_target_rate(&mut self, rate: f32) {
        self.target_rate = rate.clamp(0.5, 2.0);
        self.ramp_samples = (0.05 * SAMPLE_RATE) as u32;
        self.sample_index = 0;
    }

    pub fn set_direction(&mut self, dir: TapeDirection) {
        self.direction = dir;
    }

    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        let c = channels.min(self.channels.max(1));
        if c == 0 {
            return;
        }
        let frame_count = samples.len() / c;

        for i in 0..frame_count {
            let base = i * c;

            if self.sample_index < self.ramp_samples && self.ramp_samples > 0 {
                let t = self.sample_index as f32 / self.ramp_samples as f32;
                let ease = t * t * (3.0 - 2.0 * t);
                self.playback_rate = 1.0 + (self.target_rate - 1.0) * ease;
                self.sample_index += 1;
            } else {
                self.playback_rate = self.target_rate;
            }

            let speed_factor = match self.direction {
                TapeDirection::Forward => self.playback_rate,
                TapeDirection::Reverse => -self.playback_rate * 0.5,
            };

            for ch in 0..c {
                let idx = (base + ch) as f32 * speed_factor;
                let idx_int = idx.floor() as usize % frame_count.max(1);
                let frac = idx - idx_int as f32;
                let idx_next = (idx_int + 1) % frame_count.max(1);
                let s0 = samples[idx_int * c + ch];
                let s1 = samples[idx_next * c + ch];
                samples[base + ch] = s0 + frac * (s1 - s0);
            }
        }
    }

    pub fn reset(&mut self) {
        self.playback_rate = 1.0;
        self.target_rate = 1.0;
        self.direction = TapeDirection::Forward;
        self.ramp_samples = 0;
        self.sample_index = 0;
    }
}

// --- FadeTransformer ---

#[derive(Clone, Copy, PartialEq)]
pub enum FadeState {
    Idle,
    FadingIn,
    FadingOut,
}

pub struct FadeTransformer {
    pub target_gain: f32,
    pub start_gain: f32,
    pub duration_ms: f32,
    pub elapsed_ms: f32,
    pub curve: FadeCurve,
    pub state: FadeState,
}

impl FadeTransformer {
    pub fn new() -> Self {
        Self {
            target_gain: 1.0,
            start_gain: 1.0,
            duration_ms: 0.0,
            elapsed_ms: 0.0,
            curve: FadeCurve::Linear,
            state: FadeState::Idle,
        }
    }

    pub fn fade_in(&mut self, duration_ms: f32, curve: &str) {
        self.start_gain = 0.0;
        self.target_gain = 1.0;
        self.duration_ms = duration_ms.max(0.0);
        self.elapsed_ms = 0.0;
        self.curve = FadeCurve::from_str(curve);
        self.state = if self.duration_ms > 0.0 { FadeState::FadingIn } else { FadeState::Idle };
        if self.state == FadeState::Idle {
            self.start_gain = 1.0;
        }
    }

    pub fn fade_out(&mut self, duration_ms: f32, curve: &str) {
        self.start_gain = 1.0;
        self.target_gain = 0.0;
        self.duration_ms = duration_ms.max(0.0);
        self.elapsed_ms = 0.0;
        self.curve = FadeCurve::from_str(curve);
        self.state = if self.duration_ms > 0.0 { FadeState::FadingOut } else { FadeState::Idle };
    }

    pub fn process(&mut self, samples: &mut [f32], delta_ms: f32) -> bool {
        match self.state {
            FadeState::Idle => {
                if (self.target_gain - 1.0).abs() > 0.001 {
                    for s in samples.iter_mut() {
                        *s *= self.target_gain;
                    }
                }
                false
            }
            FadeState::FadingIn | FadeState::FadingOut => {
                let sample_count = samples.len();
                if sample_count == 0 {
                    return false;
                }

                let _step = delta_ms / self.duration_ms;
                for s in samples.iter_mut() {
                    let t = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);
                    let eased = self.curve.apply(t);
                    let gain = self.start_gain + (self.target_gain - self.start_gain) * eased;
                    *s *= gain;
                    self.elapsed_ms += delta_ms / sample_count as f32;
                }

                if self.elapsed_ms >= self.duration_ms {
                    let final_gain = self.target_gain;
                    for s in samples.iter_mut() {
                        *s *= final_gain / (if final_gain > 0.0 { final_gain } else { 1.0 });
                    }
                    self.state = FadeState::Idle;
                    self.start_gain = self.target_gain;
                    true // fade completed
                } else {
                    false
                }
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.state != FadeState::Idle
    }

    pub fn reset(&mut self) {
        self.target_gain = 1.0;
        self.start_gain = 1.0;
        self.duration_ms = 0.0;
        self.elapsed_ms = 0.0;
        self.state = FadeState::Idle;
    }
}
