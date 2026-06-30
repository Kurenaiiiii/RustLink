use std::f32::consts::PI;

pub mod band_pass;
pub mod channel_mix;
pub mod chorus;
pub mod compressor;
pub mod distortion;
pub mod echo;
pub mod equalizer;
pub mod flanger;
pub mod highpass;
pub mod karaoke;
pub mod lowpass;
pub mod phaser;
pub mod phonograph;
pub mod reverb;
pub mod rotation;
pub mod spatial;
pub mod timescale;
pub mod tremolo;
pub mod vibrato;

pub use band_pass::BandPassFilter;
pub use channel_mix::ChannelMixFilter;
pub use chorus::ChorusFilter;
pub use compressor::CompressorFilter;
pub use distortion::DistortionFilter;
pub use echo::EchoFilter;
pub use equalizer::EqualizerFilter;
pub use flanger::FlangerFilter;
pub use highpass::HighPassFilter;
pub use karaoke::KaraokeFilter;
pub use lowpass::LowPassFilter;
pub use phaser::PhaserFilter;
pub use phonograph::PhonographFilter;
pub use reverb::ReverbFilter;
pub use rotation::RotationFilter;
pub use spatial::SpatialFilter;
pub use timescale::TimescaleFilter;
pub use tremolo::TremoloFilter;
pub use vibrato::VibratoFilter;

pub const PRIORITY_TIMESCALE: u32 = 1;
pub const PRIORITY_EQUALIZER: u32 = 5;
pub const PRIORITY_DEFAULT: u32 = 10;
pub const PRIORITY_COMPRESSOR: u32 = 11;

pub const SAMPLE_RATE: f32 = 48000.0;
pub const CHANNELS: usize = 2;
pub const EQ_BANDS: [f32; 15] = [
    25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0,
    2500.0, 4000.0, 6300.0, 10000.0, 16000.0,
];

#[derive(Clone)]
pub struct BiquadState {
    x1: f32, x2: f32, y1: f32, y2: f32,
}

impl BiquadState {
    pub fn new() -> Self { Self { x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 } }
    pub fn process(&mut self, sample: f32, b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> f32 {
        let y = b0 * sample + b1 * self.x1 + b2 * self.x2 - a1 * self.y1 - a2 * self.y2;
        self.x2 = self.x1; self.x1 = sample; self.y2 = self.y1; self.y1 = y;
        y
    }
    pub fn reset(&mut self) { self.x1 = 0.0; self.x2 = 0.0; self.y1 = 0.0; self.y2 = 0.0; }
}

pub fn peaking_eq_coeffs(freq: f32, gain_db: f32, q: f32) -> (f32, f32, f32, f32, f32) {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let omega = 2.0 * PI * freq / SAMPLE_RATE;
    let alpha = omega.sin() / (2.0 * q);
    let cos_w = omega.cos();
    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha / a;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

pub fn lowpass_coeffs(freq: f32) -> (f32, f32, f32, f32, f32) {
    let rc = 1.0 / (2.0 * PI * freq);
    let dt = 1.0 / SAMPLE_RATE;
    let alpha = dt / (rc + dt);
    (alpha, 0.0, 0.0, -(1.0 - alpha), 0.0)
}

pub fn highpass_coeffs(freq: f32) -> (f32, f32, f32, f32, f32) {
    let rc = 1.0 / (2.0 * PI * freq);
    let dt = 1.0 / SAMPLE_RATE;
    let alpha = rc / (rc + dt);
    (alpha, -alpha, 0.0, -(1.0 - alpha), 0.0)
}

pub fn bandpass_coeffs(freq: f32, bw: f32) -> (f32, f32, f32, f32, f32) {
    let omega = 2.0 * PI * freq / SAMPLE_RATE;
    let alpha = omega.sin() * (bw * 0.5).sinh();
    let cos_w = omega.cos();
    let b0 = alpha;
    let b1 = 0.0;
    let b2 = -alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

pub fn allpass_coeffs(freq: f32, q: f32) -> (f32, f32, f32, f32, f32) {
    let omega = 2.0 * PI * freq / SAMPLE_RATE;
    let alpha = omega.sin() / (2.0 * q);
    let cos_w = omega.cos();
    let b0 = 1.0 - alpha;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 + alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

pub fn clamp_sample(s: f32) -> f32 { s.clamp(-1.0, 1.0) }

pub fn hash_noise(phase: f32) -> f32 {
    let bits = phase.to_bits();
    let h = bits.wrapping_mul(1103515245).wrapping_add(12345);
    ((h >> 16) & 0x7fff) as f32 / 32768.0
}
