use std::collections::HashMap;
use serde_json::Value;

use crate::player::animatable_filter::AnimatableFilter;
use crate::player::filters::*;

pub struct FiltersManager {
    bypass: bool,
    instances: HashMap<String, Box<dyn AnimatableFilter>>,
    active_order: Vec<String>,
}

impl FiltersManager {
    pub fn new() -> Self {
        Self {
            bypass: false,
            instances: HashMap::new(),
            active_order: Vec::new(),
        }
    }

    fn ensure_instance(&mut self, name: &str) {
        if self.instances.contains_key(name) { return; }
        let filter: Box<dyn AnimatableFilter> = match name {
            "timescale" => Box::new(TimescaleFilter::new()),
            "equalizer" => Box::new(EqualizerFilter::new()),
            "lowpass" => Box::new(LowPassFilter::new()),
            "highpass" => Box::new(HighPassFilter::new()),
            "karaoke" => Box::new(KaraokeFilter::new()),
            "channelMix" => Box::new(ChannelMixFilter::new()),
            "distortion" => Box::new(DistortionFilter::new()),
            "rotation" => Box::new(RotationFilter::new()),
            "tremolo" => Box::new(TremoloFilter::new()),
            "vibrato" => Box::new(VibratoFilter::new()),
            "echo" => Box::new(EchoFilter::new()),
            "reverb" => Box::new(ReverbFilter::new()),
            "chorus" => Box::new(ChorusFilter::new()),
            "compressor" => Box::new(CompressorFilter::new()),
            "phaser" => Box::new(PhaserFilter::new()),
            "flanger" => Box::new(FlangerFilter::new()),
            "spatial" => Box::new(SpatialFilter::new()),
            "phonograph" => Box::new(PhonographFilter::new()),
            "bandPass" => Box::new(BandPassFilter::new()),
            _ => return,
        };
        self.instances.insert(name.to_string(), filter);
    }

    /// Canonical key map: lowercase -> camelCase
    fn canonical_key(key: &str) -> &'static str {
        let lower = key.to_lowercase();
        match lower.as_str() {
            "timescale" => "timescale",
            "equalizer" => "equalizer",
            "lowpass" => "lowpass",
            "highpass" => "highpass",
            "karaoke" => "karaoke",
            "channelmix" => "channelMix",
            "distortion" => "distortion",
            "rotation" => "rotation",
            "tremolo" => "tremolo",
            "vibrato" => "vibrato",
            "echo" => "echo",
            "reverb" => "reverb",
            "chorus" => "chorus",
            "compressor" => "compressor",
            "phaser" => "phaser",
            "flanger" => "flanger",
            "spatial" => "spatial",
            "phonograph" => "phonograph",
            "bandpass" => "bandPass",
            _ => "unknown",
        }
    }

    pub fn update(&mut self, filters: &Value) {
        let settings = match filters {
            Value::Object(m) if m.contains_key("filters") => &filters["filters"],
            other => other,
        };

        let updated_keys: Vec<String> = match settings {
            Value::Object(map) => map.keys().cloned().collect(),
            _ => return,
        };

        for key in &updated_keys {
            let canonical = Self::canonical_key(key);
            self.ensure_instance(canonical);
            let config = &settings[key];
            if config.is_null() { continue; }
            if let Some(instance) = self.instances.get_mut(canonical) {
                instance.update(config);
            }
        }

        self.rebuild_active(&updated_keys);
    }

    fn rebuild_active(&mut self, updated_keys: &[String]) {
        let mut new_active: Vec<(u32, String)> = Vec::new();
        for (name, instance) in &self.instances {
            let in_update = updated_keys.iter().any(|k| Self::canonical_key(k) == name.as_str());
            if in_update || instance.is_active() {
                new_active.push((instance.priority(), name.clone()));
            }
        }
        new_active.sort_by_key(|(p, _)| *p);
        self.active_order = new_active.into_iter().map(|(_, n)| n).collect();
    }

    pub fn process(&mut self, chunk: &mut [f32], channels: usize, sample_rate: f32) {
        if self.bypass || self.active_order.is_empty() { return; }
        for name in &self.active_order {
            if let Some(instance) = self.instances.get_mut(name) {
                if instance.is_active() {
                    instance.process(chunk, channels, sample_rate);
                }
            }
        }
    }

    /// Flush all active filter instances and return concatenated residual audio.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut result = Vec::new();
        for name in &self.active_order {
            if let Some(instance) = self.instances.get_mut(name) {
                let flushed = instance.flush();
                result.extend(flushed);
            }
        }
        result
    }

    /// Reset filter state: flush all instances, delete standard ones, clear active list.
    pub fn reset_state(&mut self) {
        let keys: Vec<String> = self.instances.keys().cloned().collect();
        for name in &keys {
            if let Some(instance) = self.instances.get_mut(name) {
                instance.flush();
            }
            if is_standard_filter(name) {
                self.instances.remove(name);
            }
        }
        self.active_order.clear();
    }

    pub fn get_rate(&self) -> f32 {
        if self.bypass { return 1.0; }
        self.instances
            .get("timescale")
            .and_then(|t| t.get_rate())
            .unwrap_or(1.0)
    }

    pub fn is_bypass(&self) -> bool { self.bypass }
    pub fn set_bypass(&mut self, bypass: bool) { self.bypass = bypass; }

    /// Check if any filter is currently active.
    pub fn has_active(&self) -> bool {
        !self.active_order.is_empty()
    }
}

fn is_standard_filter(name: &str) -> bool {
    matches!(name,
        "timescale" | "equalizer" | "lowpass" | "highpass" | "karaoke"
        | "channelMix" | "distortion" | "rotation" | "tremolo" | "vibrato"
        | "echo" | "reverb" | "chorus" | "compressor" | "phaser"
        | "flanger" | "spatial" | "phonograph" | "bandPass"
    )
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::filters::*;

    fn stereo_buf(l: f32, r: f32, frames: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for _ in 0..frames { v.push(l); v.push(r); }
        v
    }

    #[test]
    fn test_filters_manager_create() {
        let mut fm = FiltersManager::new();
        assert!(!fm.has_active());
        assert_eq!(fm.get_rate(), 1.0);
    }

    #[test]
    fn test_timescale_filter() {
        let mut ts = TimescaleFilter::new();
        assert!(ts.is_active() == false);
        let settings = serde_json::json!({ "speed": 1.5, "pitch": 1.0, "rate": 1.0 });
        ts.update(&settings);
        assert!((ts.get_rate() - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_equalizer_filter() {
        let mut eq = EqualizerFilter::new();
        assert!(!eq.is_active());
        let settings = serde_json::json!({ "band_0": 6.0, "band_1": -3.0 });
        eq.update(&settings);
        assert!(eq.is_active());
    }

    #[test]
    fn test_filter_chain_process() {
        let mut fm = FiltersManager::new();
        let settings = serde_json::json!({
            "timescale": { "speed": 1.0, "pitch": 1.0, "rate": 1.0 },
        });
        fm.update(&settings);
        let mut buf = vec![0.5_f32; 128];
        fm.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 128);
    }

    #[test]
    fn test_canonical_key_mapping() {
        assert_eq!(FiltersManager::canonical_key("timescale"), "timescale");
        assert_eq!(FiltersManager::canonical_key("Timescale"), "timescale");
        assert_eq!(FiltersManager::canonical_key("channelmix"), "channelMix");
        assert_eq!(FiltersManager::canonical_key("bandpass"), "bandPass");
    }

    #[test]
    fn test_reset_state() {
        let mut fm = FiltersManager::new();
        fm.update(&serde_json::json!({ "timescale": { "speed": 2.0 } }));
        assert!(fm.has_active());
        fm.reset_state();
        assert!(!fm.has_active());
    }

    #[test]
    fn test_bypass() {
        let mut fm = FiltersManager::new();
        fm.set_bypass(true);
        assert!(fm.is_bypass());
        let mut buf = vec![0.5_f32; 128];
        fm.process(&mut buf, 2, 48000.0);
        assert_eq!(buf, vec![0.5_f32; 128]);
    }

    #[test]
    fn test_karaoke_filter() {
        let mut k = KaraokeFilter::new();
        assert!(!k.is_active());
        k.update(&serde_json::json!({ "level": 0.5 }));
        assert!(k.is_active());
        let mut buf = stereo_buf(0.5_f32, -0.3_f32, 64);
        k.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 128);
    }

    #[test]
    fn test_reverb_filter() {
        let mut r = ReverbFilter::new();
        assert!(!r.is_active());
        r.update(&serde_json::json!({ "wetLevel": 0.5, "dryLevel": 0.5 }));
        assert!(r.is_active());
        let mut buf = stereo_buf(0.2_f32, -0.1_f32, 64);
        r.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 128);
    }

    #[test]
    fn test_compressor_filter() {
        let mut c = CompressorFilter::new();
        assert!(!c.is_active());
        c.update(&serde_json::json!({ "threshold": -20.0, "ratio": 4.0 }));
        assert!(c.is_active());
        let mut buf = stereo_buf(0.9_f32, -0.8_f32, 32);
        c.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn test_echo_filter_flush() {
        let mut e = EchoFilter::new();
        e.update(&serde_json::json!({ "delay": 0.1, "decay": 0.5 }));
        let mut buf = vec![0.5_f32; 128];
        e.process(&mut buf, 2, 48000.0);
        let flushed = e.flush();
        assert!(!flushed.is_empty());
    }

    #[test]
    fn test_multiple_filters() {
        let mut fm = FiltersManager::new();
        fm.update(&serde_json::json!({
            "equalizer": { "band_0": 3.0, "band_1": -2.0 },
            "timescale": { "speed": 1.2, "pitch": 1.0, "rate": 1.0 },
            "karaoke": { "level": 0.3 },
        }));
        assert!(fm.has_active());
        assert!((fm.get_rate() - 1.2).abs() < 0.001);
        let mut buf = stereo_buf(0.3_f32, -0.2_f32, 128);
        fm.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 256);
    }

    #[test]
    fn test_priority_ordering() {
        let mut fm = FiltersManager::new();
        fm.update(&serde_json::json!({
            "compressor": { "threshold": -20.0, "ratio": 3.0 },
            "timescale": { "speed": 1.0, "pitch": 1.0, "rate": 1.0 },
            "equalizer": { "band_0": 2.0 },
        }));
        let active = fm.active_order.clone();
        // timescale (1) < equalizer (5) < compressor (11)
        let ts_pos = active.iter().position(|n| n == "timescale").unwrap();
        let eq_pos = active.iter().position(|n| n == "equalizer").unwrap();
        let comp_pos = active.iter().position(|n| n == "compressor").unwrap();
        assert!(ts_pos < eq_pos);
        assert!(eq_pos < comp_pos);
    }

    #[test]
    fn test_lowpass_highpass() {
        let mut lp = LowPassFilter::new();
        lp.update(&serde_json::json!({ "frequency": 5000.0 }));
        assert!(lp.is_active());
        let mut hp = HighPassFilter::new();
        hp.update(&serde_json::json!({ "frequency": 200.0 }));
        assert!(hp.is_active());
        let mut buf = stereo_buf(0.5_f32, -0.5_f32, 32);
        lp.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 64);
        hp.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn test_channel_mix() {
        let mut cm = ChannelMixFilter::new();
        cm.update(&serde_json::json!({ "leftToLeft": 0.5, "leftToRight": 0.5, "rightToLeft": 0.5, "rightToRight": 0.5 }));
        assert!(cm.is_active());
        let mut buf = stereo_buf(1.0_f32, 0.0_f32, 4);
        cm.process(&mut buf, 2, 48000.0);
        assert!(buf[0].abs() > 0.0);
    }

    #[test]
    fn test_distortion() {
        let mut d = DistortionFilter::new();
        d.update(&serde_json::json!({ "sinOffset": 0.5, "sinScale": 1.0 }));
        assert!(d.is_active());
        let mut buf = stereo_buf(0.3_f32, -0.3_f32, 4);
        d.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_rotation() {
        let mut r = RotationFilter::new();
        r.update(&serde_json::json!({ "rotationHz": 0.5 }));
        assert!(r.is_active());
        let mut buf = stereo_buf(0.5_f32, 0.0_f32, 4);
        r.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_phaser() {
        let mut p = PhaserFilter::new();
        p.update(&serde_json::json!({ "rate": 0.5, "depth": 0.5, "centerFreq": 800.0 }));
        assert!(p.is_active());
        let mut buf = stereo_buf(0.3_f32, -0.3_f32, 4);
        p.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_flanger() {
        let mut f = FlangerFilter::new();
        f.update(&serde_json::json!({ "rate": 0.5, "delay": 0.003, "depth": 0.002 }));
        assert!(f.is_active());
        let mut buf = stereo_buf(0.3_f32, -0.3_f32, 4);
        f.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_vibrato() {
        let mut v = VibratoFilter::new();
        v.update(&serde_json::json!({ "frequency": 5.0, "depth": 0.3 }));
        assert!(v.is_active());
        let mut buf = stereo_buf(0.3_f32, -0.3_f32, 4);
        v.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_tremolo() {
        let mut t = TremoloFilter::new();
        t.update(&serde_json::json!({ "frequency": 5.0, "depth": 0.5 }));
        assert!(t.is_active());
        let mut buf = stereo_buf(0.5_f32, 0.5_f32, 4);
        t.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_spatial() {
        let mut s = SpatialFilter::new();
        s.update(&serde_json::json!({ "position": [0.5, 0.0, 0.0] }));
        assert!(s.is_active());
        let mut buf = stereo_buf(0.5_f32, -0.5_f32, 4);
        s.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_bandpass() {
        let mut bp = BandPassFilter::new();
        bp.update(&serde_json::json!({ "frequency": 1000.0, "bandwidth": 1.0 }));
        assert!(bp.is_active());
        let mut buf = stereo_buf(0.5_f32, -0.5_f32, 4);
        bp.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn test_phonograph() {
        let mut p = PhonographFilter::new();
        p.update(&serde_json::json!({ "crackleVolume": 0.1, "popVolume": 0.1 }));
        assert!(p.is_active());
        let mut buf = stereo_buf(0.5_f32, -0.5_f32, 16);
        p.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 32);
    }

    #[test]
    fn test_chorus() {
        let mut c = ChorusFilter::new();
        c.update(&serde_json::json!({ "rate": 1.0, "depth": 0.003, "delay": 0.02 }));
        assert!(c.is_active());
        let mut buf = stereo_buf(0.3_f32, -0.3_f32, 4);
        c.process(&mut buf, 2, 48000.0);
        assert_eq!(buf.len(), 8);
    }
}
