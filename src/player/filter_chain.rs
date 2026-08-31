use std::f32::consts::PI;

fn hash_noise(phase: f32) -> f32 {
    let bits = phase.to_bits();
    let h = bits.wrapping_mul(1103515245).wrapping_add(12345);
    ((h >> 16) & 0x7fff) as f32 / 32768.0
}

const EQ_FREQUENCIES: [f32; 15] = [
    25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0,
    2500.0, 4000.0, 6300.0, 10000.0, 16000.0,
];

const SAMPLE_RATE: f32 = 48000.0;

struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    fn new() -> Self {
        Self { x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    fn process(&mut self, sample: f32, b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> f32 {
        let y = b0 * sample + b1 * self.x1 + b2 * self.x2 - a1 * self.y1 - a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = sample;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn allpass_coeffs(freq: f32, q: f32) -> (f32, f32, f32) {
    let omega = 2.0 * PI * freq / SAMPLE_RATE;
    let alpha = omega.sin() / (2.0 * q);
    let cos_w = omega.cos();
    let b0 = 1.0 - alpha;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 + alpha;
    (b0, b1, b2)
}

fn peaking_eq_coeffs(freq: f32, gain_db: f32, q: f32) -> (f32, f32, f32, f32, f32) {
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

fn one_pole_lowpass_coeffs(smoothing: f32) -> (f32, f32) {
    let rc = 1.0 / (smoothing * 2.0 * PI);
    let dt = 1.0 / SAMPLE_RATE;
    let alpha = dt / (rc + dt);
    (alpha, 1.0 - alpha)
}

fn one_pole_highpass_coeffs(smoothing: f32) -> (f32, f32) {
    let rc = 1.0 / (smoothing * 2.0 * PI);
    let dt = 1.0 / SAMPLE_RATE;
    let alpha = rc / (rc + dt);
    (alpha, 1.0 - alpha)
}

fn bandpass_coeffs(freq: f32, bandwidth: f32) -> (f32, f32, f32, f32, f32) {
    let omega = 2.0 * PI * freq / SAMPLE_RATE;
    let alpha = omega.sin() * (bandwidth * omega / omega.sin()).sinh().max(0.001) / 2.0;
    let cos_w = omega.cos();

    let b0 = alpha;
    let b1 = 0.0;
    let b2 = -alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;

    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

pub struct FilterChain {
    pub equalizer: [f32; 15],
    pub timescale: Option<TimescaleSettings>,
    pub tremolo: Option<TremoloSettings>,
    pub vibrato: Option<VibratoSettings>,
    pub rotation: Option<RotationSettings>,
    pub karaoke: Option<KaraokeSettings>,
    pub distortion: Option<DistortionSettings>,
    pub channel_mix: Option<ChannelMixSettings>,
    pub low_pass: Option<f32>,
    pub high_pass: Option<f32>,
    pub band_pass: Option<BandPassSettings>,
    pub echo: Option<EchoSettings>,
    pub chorus: Option<ChorusSettings>,
    pub compressor: Option<CompressorSettings>,
    pub phaser: Option<PhaserSettings>,
    pub spatial: Option<SpatialSettings>,
    pub flanger: Option<FlangerSettings>,
    pub phonograph: Option<PhonographSettings>,
    pub reverb: Option<ReverbSettings>,

    eq_states: Vec<Vec<BiquadState>>,
    bp_state: Vec<BiquadState>,
    lp_x1: Vec<f32>,
    hp_x1: Vec<f32>,
    tremolo_phase: f32,
    vibrato_phase: f32,
    rotation_phase: f32,
    phaser_lfo_phase: f32,

    echo_buffer: Vec<f32>,
    echo_write: usize,

    chorus_buffer: Vec<f32>,
    chorus_write: usize,
    chorus_phase: f32,

    compressor_env: Vec<f32>,

    phaser_allpass: Vec<Vec<BiquadState>>,

    flanger_buffer: Vec<f32>,
    flanger_write: usize,
    flanger_phase: f32,

    phonograph_crackle_phase: f32,
    phonograph_pop_phase: f32,
    phonograph_lp_x1: Vec<f32>,

    reverb_comb_buffers: Vec<Vec<f32>>,
    reverb_comb_phases: Vec<usize>,
    reverb_allpass_buffers: Vec<Vec<f32>>,
    reverb_allpass_phases: Vec<usize>,
}

#[derive(Clone)]
pub struct TimescaleSettings {
    pub speed: f32,
    pub pitch: f32,
    pub rate: f32,
}

#[derive(Clone)]
pub struct TremoloSettings {
    pub frequency: f32,
    pub depth: f32,
}

#[derive(Clone)]
pub struct VibratoSettings {
    pub frequency: f32,
    pub depth: f32,
}

#[derive(Clone)]
pub struct RotationSettings {
    pub rotation_hz: f32,
}

#[derive(Clone)]
pub struct KaraokeSettings {
    pub level: f32,
    pub mono_level: f32,
    pub filter_band: f32,
    pub filter_width: f32,
}

#[derive(Clone)]
pub struct DistortionSettings {
    pub sin_offset: f32,
    pub sin_scale: f32,
    pub cos_offset: f32,
    pub cos_scale: f32,
    pub tan_offset: f32,
    pub tan_scale: f32,
    pub offset: f32,
    pub scale: f32,
}

#[derive(Clone)]
pub struct ChannelMixSettings {
    pub left_to_left: f32,
    pub left_to_right: f32,
    pub right_to_left: f32,
    pub right_to_right: f32,
}

#[derive(Clone)]
pub struct BandPassSettings {
    pub frequency: f32,
    pub bandwidth: f32,
}

#[derive(Clone)]
pub struct EchoSettings {
    pub delay: f32,
    pub decay: f32,
    pub max_delay: f32,
}

#[derive(Clone)]
pub struct ChorusSettings {
    pub delay: f32,
    pub depth: f32,
    pub rate: f32,
}

#[derive(Clone)]
pub struct CompressorSettings {
    pub threshold: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup_gain: f32,
}

#[derive(Clone)]
pub struct PhaserSettings {
    pub rate: f32,
    pub depth: f32,
    pub feedback_q: f32,
    pub center_freq: f32,
    pub stages: i32,
}

#[derive(Clone)]
pub struct SpatialSettings {
    pub position: [f32; 3],
    pub rotation: f32,
    pub intensity: f32,
    pub algorithm: String,
}

#[derive(Clone)]
pub struct FlangerSettings {
    pub delay: f32,
    pub depth: f32,
    pub rate: f32,
    pub feedback: f32,
}

#[derive(Clone)]
pub struct PhonographSettings {
    pub crackle_volume: f32,
    pub pop_volume: f32,
    pub hum_volume: f32,
    pub low_pass_smoothing: f32,
}

#[derive(Clone)]
pub struct ReverbSettings {
    pub room_size: f32,
    pub damping: f32,
    pub wet_level: f32,
    pub dry_level: f32,
    pub delay: f32,
}

impl Default for FilterChain {
    fn default() -> Self {
        let mut eq_states = Vec::with_capacity(15);
        for _ in 0..15 {
            let mut bands = Vec::with_capacity(2);
            bands.push(BiquadState::new());
            bands.push(BiquadState::new());
            eq_states.push(bands);
        }

        let max_echo_samples = (SAMPLE_RATE * 2.0) as usize;
        let chorus_delay_samples = (0.03 * SAMPLE_RATE) as usize;

        let mut phaser_allpass = Vec::new();
        for _ in 0..6 {
            let mut stages = Vec::with_capacity(2);
            stages.push(BiquadState::new());
            stages.push(BiquadState::new());
            phaser_allpass.push(stages);
        }

        Self {
            equalizer: [0.0; 15],
            timescale: None,
            tremolo: None,
            vibrato: None,
            rotation: None,
            karaoke: None,
            distortion: None,
            channel_mix: None,
            low_pass: None,
            high_pass: None,
            band_pass: None,
            echo: None,
            chorus: None,
            compressor: None,
            phaser: None,
            spatial: None,
            flanger: None,
            phonograph: None,
            reverb: None,
            eq_states,
            bp_state: vec![BiquadState::new(), BiquadState::new()],
            lp_x1: vec![0.0, 0.0],
            hp_x1: vec![0.0, 0.0],
            tremolo_phase: 0.0,
            vibrato_phase: 0.0,
            rotation_phase: 0.0,
            phaser_lfo_phase: 0.0,
            echo_buffer: vec![0.0; max_echo_samples],
            echo_write: 0,
            chorus_buffer: vec![0.0; chorus_delay_samples * 2],
            chorus_write: 0,
            chorus_phase: 0.0,
            compressor_env: vec![1.0, 1.0],
            phaser_allpass,

            flanger_buffer: vec![0.0; 0],
            flanger_write: 0,
            flanger_phase: 0.0,

            phonograph_crackle_phase: 0.0,
            phonograph_pop_phase: 0.0,
            phonograph_lp_x1: vec![0.0, 0.0],

            reverb_comb_buffers: Vec::new(),
            reverb_comb_phases: Vec::new(),
            reverb_allpass_buffers: Vec::new(),
            reverb_allpass_phases: Vec::new(),
        }
    }
}

impl FilterChain {
    pub fn update_from_json(&mut self, json: &serde_json::Value) {
        if let Some(eq) = json.get("equalizer").and_then(|v| v.as_array()) {
            for band in eq {
                if let (Some(b), Some(g)) = (
                    band.get("band").and_then(|v| v.as_i64()),
                    band.get("gain").and_then(|v| v.as_f64()),
                ) {
                    if b >= 0 && (b as usize) < 15 {
                        self.equalizer[b as usize] = g as f32;
                    }
                }
            }
        }

        self.timescale = json.get("timescale").and_then(|v| {
            Some(TimescaleSettings {
                speed: v.get("speed")?.as_f64()? as f32,
                pitch: v.get("pitch")?.as_f64()? as f32,
                rate: v.get("rate")?.as_f64()? as f32,
            })
        });

        self.tremolo = json.get("tremolo").and_then(|v| {
            Some(TremoloSettings {
                frequency: v.get("frequency")?.as_f64()? as f32,
                depth: v.get("depth")?.as_f64()? as f32,
            })
        });

        self.vibrato = json.get("vibrato").and_then(|v| {
            Some(VibratoSettings {
                frequency: v.get("frequency")?.as_f64()? as f32,
                depth: v.get("depth")?.as_f64()? as f32,
            })
        });

        self.rotation = json.get("rotation").and_then(|v| {
            Some(RotationSettings {
                rotation_hz: v.get("rotationHz")?.as_f64()? as f32,
            })
        });

        self.karaoke = json.get("karaoke").and_then(|v| {
            Some(KaraokeSettings {
                level: v.get("level")?.as_f64()? as f32,
                mono_level: v.get("monoLevel")?.as_f64()? as f32,
                filter_band: v.get("filterBand")?.as_f64()? as f32,
                filter_width: v.get("filterWidth")?.as_f64()? as f32,
            })
        });

        self.distortion = json.get("distortion").and_then(|v| {
            Some(DistortionSettings {
                sin_offset: v.get("sinOffset")?.as_f64()? as f32,
                sin_scale: v.get("sinScale")?.as_f64()? as f32,
                cos_offset: v.get("cosOffset")?.as_f64()? as f32,
                cos_scale: v.get("cosScale")?.as_f64()? as f32,
                tan_offset: v.get("tanOffset")?.as_f64()? as f32,
                tan_scale: v.get("tanScale")?.as_f64()? as f32,
                offset: v.get("offset")?.as_f64()? as f32,
                scale: v.get("scale")?.as_f64()? as f32,
            })
        });

        self.channel_mix = json.get("channelMix").and_then(|v| {
            Some(ChannelMixSettings {
                left_to_left: v.get("leftToLeft")?.as_f64()? as f32,
                left_to_right: v.get("leftToRight")?.as_f64()? as f32,
                right_to_left: v.get("rightToLeft")?.as_f64()? as f32,
                right_to_right: v.get("rightToRight")?.as_f64()? as f32,
            })
        });

        self.low_pass = json
            .get("lowPass")
            .and_then(|v| v.get("smoothing")?.as_f64())
            .map(|v| v as f32);

        self.high_pass = json
            .get("highPass")
            .and_then(|v| v.get("smoothing")?.as_f64())
            .map(|v| v as f32);

        self.band_pass = json.get("bandPass").and_then(|v| {
            Some(BandPassSettings {
                frequency: v.get("frequency")?.as_f64()? as f32,
                bandwidth: v.get("bandwidth")?.as_f64()? as f32,
            })
        });

        self.echo = json.get("echo").and_then(|v| {
            Some(EchoSettings {
                delay: v.get("delay")?.as_f64()? as f32,
                decay: v.get("decay")?.as_f64()? as f32,
                max_delay: v.get("maxDelay")?.as_f64()? as f32,
            })
        });

        self.chorus = json.get("chorus").and_then(|v| {
            Some(ChorusSettings {
                delay: v.get("delay")?.as_f64()? as f32,
                depth: v.get("depth")?.as_f64()? as f32,
                rate: v.get("rate")?.as_f64()? as f32,
            })
        });

        self.compressor = json.get("compressor").and_then(|v| {
            Some(CompressorSettings {
                threshold: v.get("threshold")?.as_f64()? as f32,
                ratio: v.get("ratio")?.as_f64()? as f32,
                attack: v.get("attack")?.as_f64()? as f32,
                release: v.get("release")?.as_f64()? as f32,
                makeup_gain: v.get("makeupGain")?.as_f64()? as f32,
            })
        });

        self.phaser = json.get("phaser").and_then(|v| {
            Some(PhaserSettings {
                rate: v.get("rate")?.as_f64()? as f32,
                depth: v.get("depth")?.as_f64()? as f32,
                feedback_q: v.get("feedbackQ")?.as_f64()? as f32,
                center_freq: v.get("centerFreq")?.as_f64()? as f32,
                stages: v.get("stages")?.as_i64()? as i32,
            })
        });

        self.spatial = json.get("spatial").and_then(|v| {
            let pos = v.get("position").and_then(|p| p.as_array()).map_or(
                [0.0, 0.0, 0.0],
                |a| {
                    let x = a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let y = a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let z = a.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    [x, y, z]
                },
            );
            Some(SpatialSettings {
                position: pos,
                rotation: v.get("rotation")?.as_f64()? as f32,
                intensity: v.get("intensity")?.as_f64()? as f32,
                algorithm: v.get("algorithm").and_then(|a| a.as_str()).unwrap_or("stereo").to_string(),
            })
        });

        self.flanger = json.get("flanger").and_then(|v| {
            Some(FlangerSettings {
                delay: v.get("delay")?.as_f64()? as f32,
                depth: v.get("depth")?.as_f64()? as f32,
                rate: v.get("rate")?.as_f64()? as f32,
                feedback: v.get("feedback")?.as_f64()? as f32,
            })
        });

        self.phonograph = json.get("phonograph").and_then(|v| {
            Some(PhonographSettings {
                crackle_volume: v.get("crackleVolume")?.as_f64()? as f32,
                pop_volume: v.get("popVolume")?.as_f64()? as f32,
                hum_volume: v.get("humVolume")?.as_f64()? as f32,
                low_pass_smoothing: v.get("lowPassSmoothing")?.as_f64()? as f32,
            })
        });

        self.reverb = json.get("reverb").and_then(|v| {
            Some(ReverbSettings {
                room_size: v.get("roomSize")?.as_f64()? as f32,
                damping: v.get("damping")?.as_f64()? as f32,
                wet_level: v.get("wetLevel")?.as_f64()? as f32,
                dry_level: v.get("dryLevel")?.as_f64()? as f32,
                delay: v.get("delay")?.as_f64()? as f32,
            })
        });
    }

    pub fn clear(&mut self) {
        self.equalizer = [0.0; 15];
        self.timescale = None;
        self.tremolo = None;
        self.vibrato = None;
        self.rotation = None;
        self.karaoke = None;
        self.distortion = None;
        self.channel_mix = None;
        self.low_pass = None;
        self.high_pass = None;
        self.band_pass = None;
        self.echo = None;
        self.chorus = None;
        self.compressor = None;
        self.phaser = None;
        self.spatial = None;
        self.flanger = None;
        self.phonograph = None;
        self.reverb = None;
    }

    pub fn is_active(&self) -> bool {
        self.equalizer.iter().any(|&g| g.abs() > 0.01)
            || self.timescale.is_some()
            || self.tremolo.is_some()
            || self.vibrato.is_some()
            || self.rotation.is_some()
            || self.karaoke.is_some()
            || self.distortion.is_some()
            || self.channel_mix.is_some()
            || self.low_pass.is_some()
            || self.high_pass.is_some()
            || self.band_pass.is_some()
            || self.echo.is_some()
            || self.chorus.is_some()
            || self.compressor.is_some()
            || self.phaser.is_some()
            || self.spatial.is_some()
            || self.flanger.is_some()
            || self.phonograph.is_some()
            || self.reverb.is_some()
    }

    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if !self.is_active() {
            return;
        }

        let frame_count = samples.len() / channels;
        let c = channels.min(2);

        if let Some(ref mix) = self.channel_mix.clone() {
            for i in 0..frame_count {
                let base = i * channels;
                if c >= 2 {
                    let l = samples[base];
                    let r = samples[base + 1];
                    samples[base] = l * mix.left_to_left + r * mix.right_to_left;
                    samples[base + 1] = l * mix.left_to_right + r * mix.right_to_right;
                }
            }
        }

        if let Some(ref k) = self.karaoke.clone() {
            if c >= 2 {
                for i in 0..frame_count {
                    let base = i * channels;
                    let l = samples[base];
                    let r = samples[base + 1];
                    let center = (l + r) * k.mono_level;
                    samples[base] = l - center * k.level;
                    samples[base + 1] = r - center * k.level;
                }
            }
        }

        if let Some(ref s) = self.spatial.clone() {
            if c >= 2 && s.algorithm == "stereo" {
                let intensity = s.intensity;
                for i in 0..frame_count {
                    let base = i * channels;
                    let l = samples[base];
                    let r = samples[base + 1];
                    let pan = s.position[0].clamp(-1.0, 1.0);
                    let spread = 1.0 - pan.abs();
                    let balance_l = if pan < 0.0 { 1.0 } else { spread };
                    let balance_r = if pan > 0.0 { 1.0 } else { spread };
                    samples[base] = l * (1.0 + (balance_l - 1.0) * intensity);
                    samples[base + 1] = r * (1.0 + (balance_r - 1.0) * intensity);
                }
            }
        }

        let has_eq = self.equalizer.iter().any(|&g| g.abs() > 0.01);
        if has_eq {
            for ch in 0..c {
                for band in 0..15 {
                    if self.equalizer[band].abs() <= 0.01 {
                        continue;
                    }
                    let gain_db = self.equalizer[band];
                    let (b0, b1, b2, a1, a2) = peaking_eq_coeffs(
                        EQ_FREQUENCIES[band],
                        gain_db,
                        1.0,
                    );
                    for i in 0..frame_count {
                        let idx = i * channels + ch;
                        samples[idx] = self.eq_states[band][ch].process(
                            samples[idx], b0, b1, b2, a1, a2,
                        );
                    }
                }
            }
        }

        if let Some(smoothing) = self.low_pass {
            let (a0, b0) = one_pole_lowpass_coeffs(smoothing);
            for ch in 0..c {
                for i in 0..frame_count {
                    let idx = i * channels + ch;
                    let out = a0 * samples[idx] + b0 * self.lp_x1[ch];
                    self.lp_x1[ch] = out;
                    samples[idx] = out;
                }
            }
        }

        if let Some(smoothing) = self.high_pass {
            let (a0, _b0) = one_pole_highpass_coeffs(smoothing);
            for ch in 0..c {
                for i in 0..frame_count {
                    let idx = i * channels + ch;
                    let out = a0 * (self.hp_x1[ch] + samples[idx] - samples[idx]);
                    self.hp_x1[ch] = samples[idx];
                    samples[idx] = out;
                }
            }
        }

        if let Some(ref bp) = self.band_pass.clone() {
            let (b0, b1, b2, a1, a2) = bandpass_coeffs(bp.frequency, bp.bandwidth);
            for ch in 0..c {
                for i in 0..frame_count {
                    let idx = i * channels + ch;
                    samples[idx] = self.bp_state[ch].process(
                        samples[idx], b0, b1, b2, a1, a2,
                    );
                }
            }
        }

        if let Some(ref e) = self.echo.clone() {
            let delay_samples = (e.delay * SAMPLE_RATE) as usize;
            let max_delay = (e.max_delay * SAMPLE_RATE).max(1.0) as usize;
            let buf_len = self.echo_buffer.len();
            if delay_samples > 0 && delay_samples <= buf_len {
                for i in 0..frame_count {
                    let base = i * channels;
                    for ch in 0..c {
                        let idx = base + ch;
                        let read_pos = (self.echo_write + buf_len - delay_samples) % buf_len;
                        let delayed = self.echo_buffer[read_pos];
                        let out = samples[idx] + delayed * e.decay;
                        self.echo_buffer[self.echo_write] = out;
                        samples[idx] = out.clamp(-1.0, 1.0);
                        self.echo_write = (self.echo_write + 1) % max_delay;
                    }
                }
            }
        }

        if let Some(ref ch) = self.chorus.clone() {
            let max_delay_samples = (ch.delay * SAMPLE_RATE / 1000.0) as usize;
            let depth_samples = (ch.depth * SAMPLE_RATE / 1000.0) as usize;
            let buf_len = self.chorus_buffer.len() / c;
            if buf_len == 0 {
                return;
            }
            let phase_inc = ch.rate / SAMPLE_RATE;

            for i in 0..frame_count {
                let base = i * channels;
                let mod_delay = max_delay_samples as f32
                    + depth_samples as f32 * 0.5 * (1.0 + (2.0 * PI * self.chorus_phase).sin());
                let read_pos_f = (self.chorus_write as f32 - mod_delay).rem_euclid(buf_len as f32);
                let read_pos = read_pos_f as usize % buf_len;
                let frac = read_pos_f - read_pos as f32;
                let next_pos = (read_pos + 1) % buf_len;

                for ch_idx in 0..c {
                    let delayed = self.chorus_buffer[read_pos * c + ch_idx]
                        + frac * (self.chorus_buffer[next_pos * c + ch_idx]
                            - self.chorus_buffer[read_pos * c + ch_idx]);
                    self.chorus_buffer[self.chorus_write * c + ch_idx] = samples[base + ch_idx];
                    samples[base + ch_idx] = (samples[base + ch_idx] + delayed) * 0.5;
                }

                self.chorus_write = (self.chorus_write + 1) % buf_len;
                self.chorus_phase += phase_inc;
                if self.chorus_phase >= 1.0 {
                    self.chorus_phase -= 1.0;
                }
            }
        }

        if let Some(ref cp) = self.compressor.clone() {
            let threshold_linear = 10.0_f32.powf(cp.threshold / 20.0);
            for ch_idx in 0..c {
                for i in 0..frame_count {
                    let idx = i * channels + ch_idx;
                    let abs_sample = samples[idx].abs();
                    if abs_sample > threshold_linear {
                        let db = 20.0 * abs_sample.log10().max(-100.0);
                        let reduction = (db - cp.threshold) * (1.0 - 1.0 / cp.ratio);
                        let target_gain = 10.0_f32.powf(-reduction / 20.0);
                        let coeff = if target_gain < self.compressor_env[ch_idx] {
                            (-1.0 / (SAMPLE_RATE * cp.attack / 1000.0)).exp()
                        } else {
                            (-1.0 / (SAMPLE_RATE * cp.release / 1000.0)).exp()
                        };
                        self.compressor_env[ch_idx] +=
                            (target_gain - self.compressor_env[ch_idx]) * (1.0 - coeff);
                    } else {
                        self.compressor_env[ch_idx] +=
                            (1.0 - self.compressor_env[ch_idx]) * (1.0 - (-1.0 / (SAMPLE_RATE * cp.release / 1000.0)).exp());
                    }
                    let gain = self.compressor_env[ch_idx];

                    let makeup = 10.0_f32.powf(cp.makeup_gain / 20.0);
                    samples[idx] = (samples[idx] * gain * makeup).clamp(-1.0, 1.0);
                }
            }
        }

        if let Some(ref p) = self.phaser.clone() {
            let stages = p.stages.max(1).min(6) as usize;
            let phase_inc = p.rate / SAMPLE_RATE;

            for i in 0..frame_count {
                let base = i * channels;
                let mod_freq = p.center_freq
                    * (1.0 + p.depth * (2.0 * PI * self.phaser_lfo_phase).sin());
                let q = p.feedback_q.max(1.0);
                let (b0, b1, b2) = allpass_coeffs(mod_freq, q);

                for ch_idx in 0..c {
                    let mut x = samples[base + ch_idx];
                    for stage in 0..stages.min(self.phaser_allpass.len()) {
                        let prev = if stage > 0 { samples[base + ch_idx] } else { x };
                        x = self.phaser_allpass[stage][ch_idx].process(x, b0, b1, b2, 1.0, 0.0) - b0 * prev;
                    }
                    samples[base + ch_idx] = x;
                }

                self.phaser_lfo_phase += phase_inc;
                if self.phaser_lfo_phase >= 1.0 {
                    self.phaser_lfo_phase -= 1.0;
                }
            }
        }

        if let Some(ref d) = self.distortion.clone() {
            for sample in samples.iter_mut() {
                let x = *sample;
                *sample = (x + d.offset) * d.scale
                    + (x * d.sin_scale + d.sin_offset).sin()
                    + (x * d.cos_scale + d.cos_offset).cos()
                    + (x * d.tan_scale + d.tan_offset).tan();
                *sample = sample.clamp(-1.0, 1.0);
            }
        }

        if let Some(ref t) = self.tremolo.clone() {
            let phase_inc = t.frequency / SAMPLE_RATE;
            for i in 0..frame_count {
                let idx = i * channels;
                let mod_val = 1.0 - t.depth * (2.0 * PI * (self.tremolo_phase + phase_inc * i as f32)).sin();
                self.tremolo_phase += phase_inc;
                if self.tremolo_phase >= 1.0 {
                    self.tremolo_phase -= 1.0;
                }
                for ch in 0..c {
                    samples[idx + ch] *= mod_val;
                }
            }
        }

        if let Some(ref v) = self.vibrato.clone() {
            let max_delay = 0.005 * SAMPLE_RATE;
            let delay_samples = max_delay as usize;
            let mut delay_buf = vec![0.0f32; delay_samples * c];
            let mut write_pos = 0usize;
            let phase_inc = v.frequency / SAMPLE_RATE;

            for i in 0..frame_count {
                let delay = v.depth * 0.5 * max_delay * (2.0 * PI * self.vibrato_phase).sin();
                let read_pos_f = (write_pos as f32 - delay).rem_euclid(delay_samples as f32);
                let read_pos = read_pos_f as usize;
                let frac = read_pos_f - read_pos as f32;
                let next_pos = (read_pos + 1) % delay_samples;

                let base = i * channels;
                for ch in 0..c {
                    let delayed = delay_buf[read_pos * c + ch]
                        + frac * (delay_buf[next_pos * c + ch] - delay_buf[read_pos * c + ch]);
                    delay_buf[write_pos * c + ch] = samples[base + ch];
                    samples[base + ch] = delayed;
                }

                write_pos = (write_pos + 1) % delay_samples;
                self.vibrato_phase += phase_inc;
                if self.vibrato_phase >= 1.0 {
                    self.vibrato_phase -= 1.0;
                }
            }
        }

        if let Some(ref r) = self.rotation.clone() {
            let phase_inc = r.rotation_hz / SAMPLE_RATE;
            for i in 0..frame_count {
                let base = i * channels;
                let pan = (2.0 * PI * self.rotation_phase).sin();
                if c >= 2 {
                    let l = samples[base];
                    let r = samples[base + 1];
                    let gain_l = if pan < 0.0 { (-pan).sqrt() } else { 1.0 };
                    let gain_r = if pan > 0.0 { pan.sqrt() } else { 1.0 };
                    samples[base] = l * gain_l;
                    samples[base + 1] = r * gain_r;
                }
                self.rotation_phase += phase_inc;
                if self.rotation_phase >= 1.0 {
                    self.rotation_phase -= 1.0;
                }
            }
        }

        if let Some(ref ts) = self.timescale.clone() {
            if (ts.speed - 1.0).abs() > 0.01 {
                for ch in 0..c {
                    let mut out = Vec::with_capacity(frame_count);
                    let mut read_pos = 0.0f32;
                    while (read_pos as usize) < frame_count - 1 {
                        let idx = read_pos as usize;
                        let frac = read_pos - idx as f32;
                        let next = (idx + 1).min(frame_count - 1);
                        let sample = samples[idx * channels + ch]
                            + frac * (samples[next * channels + ch] - samples[idx * channels + ch]);
                        out.push(sample);
                        read_pos += ts.speed;
                    }
                    for (j, &s) in out.iter().enumerate() {
                        if j < frame_count {
                            samples[j * channels + ch] = s;
                        }
                    }
                }
            }
        }

        if let Some(ref f) = self.flanger.clone() {
            let max_delay_samples = (f.delay * SAMPLE_RATE / 1000.0) as usize;
            let depth_samples = (f.depth * SAMPLE_RATE / 1000.0) as usize;
            let buf_len = self.flanger_buffer.len();
            if buf_len == 0 || buf_len != max_delay_samples * c {
                self.flanger_buffer = vec![0.0; (max_delay_samples * c).max(1)];
                self.flanger_write = 0;
                self.flanger_phase = 0.0;
            }
            let buf_len = self.flanger_buffer.len() / c;
            if buf_len == 0 {
                return;
            }
            let phase_inc = f.rate / SAMPLE_RATE;

            for i in 0..frame_count {
                let base = i * channels;
                let mod_delay = depth_samples as f32 * 0.5 * (1.0 + (2.0 * PI * self.flanger_phase).sin());
                let read_pos_f = (self.flanger_write as f32 - mod_delay).rem_euclid(buf_len as f32);
                let read_pos = read_pos_f as usize % buf_len;
                let frac = read_pos_f - read_pos as f32;
                let next_pos = (read_pos + 1) % buf_len;

                for ch_idx in 0..c {
                    let delayed = self.flanger_buffer[read_pos * c + ch_idx]
                        + frac * (self.flanger_buffer[next_pos * c + ch_idx]
                            - self.flanger_buffer[read_pos * c + ch_idx]);
                    self.flanger_buffer[self.flanger_write * c + ch_idx] = samples[base + ch_idx];
                    samples[base + ch_idx] = (samples[base + ch_idx] + delayed * f.feedback) * 0.5;
                }

                self.flanger_write = (self.flanger_write + 1) % buf_len;
                self.flanger_phase += phase_inc;
                if self.flanger_phase >= 1.0 {
                    self.flanger_phase -= 1.0;
                }
            }
        }

        if let Some(ref p) = self.phonograph.clone() {
            let crackle_inc = 1.0 / SAMPLE_RATE;
            let pop_inc = 0.5 / SAMPLE_RATE;

            for i in 0..frame_count {
                let base = i * channels;

                self.phonograph_crackle_phase += crackle_inc;
                if self.phonograph_crackle_phase >= 1.0 {
                    self.phonograph_crackle_phase -= 1.0;
                }
                let crackle = if (self.phonograph_crackle_phase * SAMPLE_RATE) as i32 % 100 < 2 {
                    (hash_noise(self.phonograph_crackle_phase) * 2.0 - 1.0) * p.crackle_volume
                } else {
                    0.0
                };

                self.phonograph_pop_phase += pop_inc;
                let pop = if self.phonograph_pop_phase >= 1.0 {
                    self.phonograph_pop_phase -= 1.0;
                    (hash_noise(self.phonograph_pop_phase) * 2.0 - 1.0) * p.pop_volume
                } else {
                    0.0
                };

                for ch_idx in 0..c {
                    let idx = base + ch_idx;
                    samples[idx] += crackle + pop;
                }
            }

            if p.hum_volume > 0.0 {
                for ch_idx in 0..c {
                    for i in 0..frame_count {
                        let idx = i * channels + ch_idx;
                        let hum = (2.0 * PI * 50.0 * (i as f32 + self.phonograph_crackle_phase * SAMPLE_RATE) / SAMPLE_RATE).sin()
                            * p.hum_volume * 0.1;
                        samples[idx] += hum;
                    }
                }
            }

            if p.low_pass_smoothing > 0.0 {
                let (a0, b0) = one_pole_lowpass_coeffs(p.low_pass_smoothing);
                for ch_idx in 0..c {
                    for i in 0..frame_count {
                        let idx = i * channels + ch_idx;
                        let out = a0 * samples[idx] + b0 * self.phonograph_lp_x1[ch_idx];
                        self.phonograph_lp_x1[ch_idx] = out;
                        samples[idx] = out;
                    }
                }
            }
        }

        if let Some(ref r) = self.reverb.clone() {
            let comb_delays: [usize; 4] = [
                (r.delay * SAMPLE_RATE / 1000.0) as usize,
                ((r.delay + 1.6) * SAMPLE_RATE / 1000.0) as usize,
                ((r.delay + 3.2) * SAMPLE_RATE / 1000.0) as usize,
                ((r.delay + 5.4) * SAMPLE_RATE / 1000.0) as usize,
            ];
            let allpass_delays: [usize; 2] = [
                (0.5 * SAMPLE_RATE / 1000.0) as usize,
                (1.2 * SAMPLE_RATE / 1000.0) as usize,
            ];

            if self.reverb_comb_buffers.len() != 4 {
                self.reverb_comb_buffers = vec![vec![0.0; 2]; 4];
                self.reverb_comb_phases = vec![0; 4];
                self.reverb_allpass_buffers = vec![vec![0.0; 2]; 2];
                self.reverb_allpass_phases = vec![0; 2];
            }
            for ch_idx in 0..c {
                for i in 0..frame_count {
                    let idx = i * channels + ch_idx;
                    let input = samples[idx];
                    let mut wet = 0.0;

                    for stage in 0..4 {
                        let delay = comb_delays[stage].max(1);
                        let buf_size = delay * c;
                        if self.reverb_comb_buffers[stage].len() < buf_size {
                            self.reverb_comb_buffers[stage].resize(buf_size, 0.0);
                        }
                        let read_pos = (self.reverb_comb_phases[stage] + buf_size - delay * c) % buf_size.max(1);
                        let out = self.reverb_comb_buffers[stage][read_pos];
                        let feedback = out * r.room_size * r.damping;
                        self.reverb_comb_buffers[stage][self.reverb_comb_phases[stage]] = input + feedback;
                        self.reverb_comb_phases[stage] = (self.reverb_comb_phases[stage] + 1) % buf_size.max(1);
                        wet += out;
                    }

                    for stage in 0..2 {
                        let delay = allpass_delays[stage].max(1);
                        let buf_size = delay * c;
                        if self.reverb_allpass_buffers[stage].len() < buf_size {
                            self.reverb_allpass_buffers[stage].resize(buf_size, 0.0);
                        }
                        let read_pos = (self.reverb_allpass_phases[stage] + buf_size - delay * c) % buf_size.max(1);
                        let out = self.reverb_allpass_buffers[stage][read_pos];
                        self.reverb_allpass_buffers[stage][self.reverb_allpass_phases[stage]] = wet + out * 0.5;
                        wet = -wet + self.reverb_allpass_buffers[stage][self.reverb_allpass_phases[stage]];
                        self.reverb_allpass_phases[stage] = (self.reverb_allpass_phases[stage] + 1) % buf_size.max(1);
                    }

                    samples[idx] = input * r.dry_level + wet * r.wet_level;
                }
            }
        }

        for sample in samples.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}
