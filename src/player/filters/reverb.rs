use serde_json::Value;
use crate::player::animatable_filter::{AnimatableConfig, AnimatableFilter};
use super::{PRIORITY_DEFAULT, clamp_sample};

pub struct ReverbFilter {
    pub priority: u32,
    config: AnimatableConfig,
    comb_delays: [usize; 4],
    comb_feedback: [f32; 4],
    comb_buffers: [Vec<f32>; 4],
    comb_phases: [usize; 4],
    allpass_delays: [usize; 2],
    allpass_buffers: [Vec<f32>; 2],
    allpass_phases: [usize; 2],
    active: bool,
}

impl ReverbFilter {
    pub fn new() -> Self {
        let comb_delays = [1116, 1188, 1277, 1356];
        let allpass_delays = [225, 341];
        let comb_buffers = [
            vec![0.0; 1116 * 2],
            vec![0.0; 1188 * 2],
            vec![0.0; 1277 * 2],
            vec![0.0; 1356 * 2],
        ];
        let allpass_buffers = [
            vec![0.0; 225 * 2],
            vec![0.0; 341 * 2],
        ];
        Self {
            priority: PRIORITY_DEFAULT,
            config: AnimatableConfig::new(&[0.5, 0.5, 0.5, 0.5]),
            comb_delays,
            comb_feedback: [0.5; 4],
            comb_buffers,
            comb_phases: [0; 4],
            allpass_delays,
            allpass_buffers,
            allpass_phases: [0; 2],
            active: false,
        }
    }
}

impl AnimatableFilter for ReverbFilter {
    fn priority(&self) -> u32 { self.priority }
    fn is_active(&self) -> bool { self.active || self.config.is_animating() }

    fn update(&mut self, settings: &Value) {
        self.config.apply_animated_update(settings, "reverb", &[("roomSize", 0), ("damping", 1), ("wetLevel", 2), ("dryLevel", 3)]);
        let c = self.config.get_current();
        self.active = c[2] > 0.001;
        let room_size = c[0].clamp(0.0, 1.0);
        for fb in self.comb_feedback.iter_mut() {
            *fb = room_size * 0.8 + 0.2;
        }
    }

    fn process(&mut self, chunk: &mut [f32], _channels: usize, sample_rate: f32) {
        self.config.process_animation(sample_rate, chunk.len(), 2);
        let c = self.config.get_current();
        let wet = c[2].clamp(0.0, 1.0);
        let dry = c[3].clamp(0.0, 1.0);
        if wet <= 0.001 && (dry - 1.0).abs() < 0.001 { return; }
        for frame in chunk.chunks_mut(2) {
            if frame.len() < 2 { continue; }
            let input_l = frame[0];
            let input_r = frame[1];
            let mut comb_out_l = 0.0;
            let mut comb_out_r = 0.0;
            for i in 0..4 {
                let delay = self.comb_delays[i];
                let fb = self.comb_feedback[i];
                let phase = self.comb_phases[i];
                let buf_idx = phase * 2;
                let buf_len = self.comb_buffers[i].len();
                let delayed_l = self.comb_buffers[i][buf_idx % buf_len];
                let delayed_r = self.comb_buffers[i][(buf_idx + 1) % buf_len];
                let out_l = delayed_l;
                let out_r = delayed_r;
                self.comb_buffers[i][buf_idx % buf_len] = clamp_sample(input_l + delayed_l * fb);
                self.comb_buffers[i][(buf_idx + 1) % buf_len] = clamp_sample(input_r + delayed_r * fb);
                self.comb_phases[i] = (phase + 1) % delay;
                comb_out_l += out_l;
                comb_out_r += out_r;
            }
            comb_out_l *= 0.25;
            comb_out_r *= 0.25;
            let mut ap_l = comb_out_l;
            let mut ap_r = comb_out_r;
            for i in 0..2 {
                let delay = self.allpass_delays[i];
                let phase = self.allpass_phases[i];
                let buf_idx = phase * 2;
                let buf_len = self.allpass_buffers[i].len();
                let delayed_l = self.allpass_buffers[i][buf_idx % buf_len];
                let delayed_r = self.allpass_buffers[i][(buf_idx + 1) % buf_len];
                self.allpass_buffers[i][buf_idx % buf_len] = ap_l;
                self.allpass_buffers[i][(buf_idx + 1) % buf_len] = ap_r;
                ap_l = clamp_sample(delayed_l - ap_l * 0.5);
                ap_r = clamp_sample(delayed_r - ap_r * 0.5);
                self.allpass_phases[i] = (phase + 1) % delay;
            }
            frame[0] = clamp_sample(input_l * dry + ap_l * wet);
            frame[1] = clamp_sample(input_r * dry + ap_r * wet);
        }
    }

    fn flush(&mut self) -> Vec<f32> {
        for buf in self.comb_buffers.iter_mut() { buf.fill(0.0); }
        for buf in self.allpass_buffers.iter_mut() { buf.fill(0.0); }
        Vec::new()
    }
}
