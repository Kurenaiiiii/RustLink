/// Simplified EBU R128 loudness normalizer.
///
/// Uses a delay buffer (lookahead) to measure integrated loudness
/// before applying gain.  Loudness is estimated as RMS-based LUFS
/// with ITU‑R BS.1770‑4 channel weighting.  Gain changes are smoothed
/// with a one‑pole IIR filter to avoid audible clicks.

const CHANNEL_WEIGHTS: &[f64] = &[1.0, 1.0, 1.0, 1.41, 1.41, 0.0];
const MAX_GAIN_DB: f64 = 15.0;
const SMOOTHING: f32 = 0.05;

pub struct LoudnessNormalizer {
    enabled: bool,
    delay_buf: Vec<f32>,
    write_pos: usize,
    buf_frames: usize,
    channels: usize,
    current_gain: f32,
    target_lufs: f64,
    gate_threshold: f64,
    frame_idx: u64,
}

impl LoudnessNormalizer {
    pub fn new(
        channels: usize,
        sample_rate: u32,
        lookahead_ms: u64,
        target_lufs: f64,
        gate_threshold_lufs: f64,
    ) -> Self {
        let buf_frames = ((lookahead_ms as f64 * sample_rate as f64) / 1000.0).ceil() as usize;
        let buf_len = buf_frames * channels;
        Self {
            enabled: true,
            delay_buf: vec![0.0; buf_len.max(1)],
            write_pos: 0,
            buf_frames: buf_frames.max(1),
            channels,
            current_gain: 1.0,
            target_lufs,
            gate_threshold: gate_threshold_lufs,
            frame_idx: 0,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.current_gain = 1.0;
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.enabled || samples.is_empty() {
            return;
        }

        let frame_size = self.channels;
        let total_frames = samples.len() / frame_size;

        // Measure loudness every 100ms from the accumulated buffer
        if self.frame_idx % 5 == 0 && self.write_pos >= self.buf_frames * frame_size {
            let lufs = self.measure_loudness();
            self.update_gain(lufs);
        }

        // Circular delay: write new samples, read delayed samples
        let mut output = vec![0.0f32; samples.len()];
        let buf = &mut self.delay_buf;
        let buflen = buf.len();
        let pos = self.write_pos;

        for f in 0..total_frames {
            let read_pos = (pos + f * frame_size + frame_size) % buflen;
            let write_pos = (pos + f * frame_size) % buflen;

            for c in 0..frame_size {
                let ri = (read_pos + c) % buflen;
                output[f * frame_size + c] = buf[ri] * self.current_gain;
            }
            for c in 0..frame_size {
                let wi = (write_pos + c) % buflen;
                buf[wi] = samples[f * frame_size + c];
            }
        }

        self.write_pos = (pos + total_frames * frame_size) % buflen;
        self.frame_idx += total_frames as u64;
        samples.copy_from_slice(&output);
    }

    fn measure_loudness(&self) -> f64 {
        let frame_size = self.channels;
        let n = self.buf_frames.min(self.delay_buf.len() / frame_size);
        if n == 0 {
            return -100.0;
        }

        let mut channel_powers = vec![0.0f64; frame_size];
        for ch in 0..frame_size {
            let mut sum_sq = 0.0f64;
            for f in 0..n {
                let s = self.delay_buf[f * frame_size + ch] as f64;
                sum_sq += s * s;
            }
            channel_powers[ch] = sum_sq / n as f64;
        }

        let mut weighted_sum = 0.0f64;
        for ch in 0..frame_size {
            let w = CHANNEL_WEIGHTS.get(ch).copied().unwrap_or(1.0);
            weighted_sum += w * channel_powers[ch];
        }

        if weighted_sum <= 0.0 {
            return -100.0;
        }

        -0.691 + 10.0 * weighted_sum.log10()
    }

    fn compute_target_gain(&self, lufs: f64) -> f32 {
        if lufs < self.gate_threshold {
            return self.current_gain;
        }
        let delta_db = lufs - self.target_lufs;
        let gain_db = (-delta_db).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        10.0f64.powf(gain_db / 20.0) as f32
    }

    fn update_gain(&mut self, lufs: f64) {
        let target = self.compute_target_gain(lufs);
        self.current_gain += (target - self.current_gain) * SMOOTHING;
    }

    pub fn gain(&self) -> f32 {
        self.current_gain
    }
}
