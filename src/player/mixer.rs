use std::f32::consts::FRAC_PI_4;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct MixerLayer {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub url: Option<String>,
}

impl MixerLayer {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            volume: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            url: None,
        }
    }
}

pub struct AudioMixer {
    layers: Vec<MixerLayer>,
    next_id: u64,
}

impl AudioMixer {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            next_id: 1,
        }
    }

    pub fn add_layer(
        &mut self,
        name: &str,
        volume: f32,
        pan: f32,
    ) -> String {
        let id = format!("layer_{}", self.next_id);
        self.next_id += 1;
        self.layers.push(MixerLayer {
            id: id.clone(),
            name: name.to_string(),
            volume: volume.clamp(0.0, 2.0),
            pan: pan.clamp(-1.0, 1.0),
            mute: false,
            solo: false,
            url: None,
        });
        id
    }

    pub fn remove_layer(&mut self, id: &str) -> bool {
        let idx = self.layers.iter().position(|l| l.id == id);
        if let Some(idx) = idx {
            self.layers.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn update_layer(
        &mut self,
        id: &str,
        name: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
        url: Option<String>,
    ) -> bool {
        let layer = self.layers.iter_mut().find(|l| l.id == id);
        if let Some(layer) = layer {
            if let Some(name) = name {
                layer.name = name;
            }
            if let Some(volume) = volume {
                layer.volume = volume.clamp(0.0, 2.0);
            }
            if let Some(pan) = pan {
                layer.pan = pan.clamp(-1.0, 1.0);
            }
            if let Some(mute) = mute {
                layer.mute = mute;
            }
            if let Some(solo) = solo {
                layer.solo = solo;
            }
            if url.is_some() {
                layer.url = url;
            }
            true
        } else {
            false
        }
    }

    pub fn get_layer(&self, id: &str) -> Option<&MixerLayer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn get_layer_mut(&mut self, id: &str) -> Option<&mut MixerLayer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    pub fn has_solo(&self) -> bool {
        self.layers.iter().any(|l| l.solo)
    }

    pub fn layers(&self) -> &[MixerLayer] {
        &self.layers
    }

    pub fn to_json(&self) -> Value {
        json!(self.layers.iter().map(|l| json!({
            "id": l.id,
            "name": l.name,
            "volume": l.volume,
            "pan": l.pan,
            "mute": l.mute,
            "solo": l.solo,
            "url": l.url,
        })).collect::<Vec<_>>())
    }

    pub fn mix_into(
        output: &mut [f32],
        layer_input: &[f32],
        volume: f32,
        pan: f32,
        channels: usize,
    ) {
        let mix_len = output.len().min(layer_input.len());
        if channels == 2 {
            let angle = (pan + 1.0) * FRAC_PI_4;
            let left_gain = volume * angle.cos();
            let right_gain = volume * angle.sin();
            let mut i = 0;
            while i + 1 < mix_len {
                output[i] += layer_input[i] * left_gain;
                output[i + 1] += layer_input[i + 1] * right_gain;
                i += 2;
            }
        } else {
            for i in 0..mix_len {
                output[i] += layer_input[i] * volume;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_remove_layer() {
        let mut m = AudioMixer::new();
        let id = m.add_layer("test", 1.0, 0.0);
        assert!(m.get_layer(&id).is_some());
        assert!(m.remove_layer(&id));
        assert!(m.get_layer(&id).is_none());
    }

    #[test]
    fn test_update_layer() {
        let mut m = AudioMixer::new();
        let id = m.add_layer("test", 1.0, 0.0);
        assert!(m.update_layer(&id, Some("renamed".into()), Some(0.5), Some(-1.0), Some(true), Some(true), None));
        let layer = m.get_layer(&id).unwrap();
        assert_eq!(layer.name, "renamed");
        assert_eq!(layer.volume, 0.5);
        assert_eq!(layer.pan, -1.0);
        assert!(layer.mute);
        assert!(layer.solo);
    }

    #[test]
    fn test_solo_detection() {
        let mut m = AudioMixer::new();
        let id = m.add_layer("a", 1.0, 0.0);
        assert!(!m.has_solo());
        m.update_layer(&id, None, None, None, None, Some(true), None);
        assert!(m.has_solo());
    }

    #[test]
    fn test_mix_into_stereo_center() {
        let mut out = vec![0.0f32; 4];
        let inp = vec![1.0f32; 4];
        AudioMixer::mix_into(&mut out, &inp, 1.0, 0.0, 2);
        let expected = FRAC_PI_4.cos();
        assert!((out[0] - expected).abs() < 1e-6);
        assert!((out[1] - expected).abs() < 1e-6);
    }

    #[test]
    fn test_mix_into_stereo_pan_left() {
        let mut out = vec![0.0f32; 4];
        let inp = vec![1.0f32; 4];
        AudioMixer::mix_into(&mut out, &inp, 1.0, -1.0, 2);
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[1] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_mix_into_stereo_pan_right() {
        let mut out = vec![0.0f32; 4];
        let inp = vec![1.0f32; 4];
        AudioMixer::mix_into(&mut out, &inp, 1.0, 1.0, 2);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_mix_into_mono() {
        let mut out = vec![0.0f32; 4];
        let inp = vec![1.0f32; 4];
        AudioMixer::mix_into(&mut out, &inp, 0.5, 0.0, 1);
        for s in &out {
            assert!((*s - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_mix_volume_clamp() {
        let mut m = AudioMixer::new();
        let id = m.add_layer("test", 3.0, 0.0);
        assert!((m.get_layer(&id).unwrap().volume - 2.0).abs() < 1e-6);
        m.update_layer(&id, None, Some(0.5), None, None, None, None);
        assert!((m.get_layer(&id).unwrap().volume - 0.5).abs() < 1e-6);
    }
}
