use super::waves;

const SAMPLE_RATE: f64 = 48000.0;

pub struct LFO {
    frequency: f64,
    depth: f64,
    phase: f64,
    waveform: Option<waves::WaveformFn>,
}

impl LFO {
    pub fn new(waveform_name: &str, frequency: f64, depth: f64) -> Self {
        let wf = waves::get_waveform(waveform_name);
        Self {
            frequency,
            depth,
            phase: 0.0,
            waveform: wf,
        }
    }

    pub fn set_waveform(&mut self, waveform_name: &str) {
        self.waveform = waves::get_waveform(waveform_name);
    }

    pub fn update(&mut self, frequency: f64, depth: f64) {
        self.frequency = frequency;
        self.depth = depth;
    }

    pub fn get_value(&mut self) -> f64 {
        let value = match self.waveform {
            Some(wf) => wf(self.phase),
            None => 0.0,
        };
        self.phase += 2.0 * std::f64::consts::PI * self.frequency / SAMPLE_RATE;
        if self.phase >= 2.0 * std::f64::consts::PI {
            self.phase -= 2.0 * std::f64::consts::PI;
        }
        value
    }

    pub fn process(&mut self) -> f64 {
        if self.depth <= 0.0 || self.frequency <= 0.0 {
            return 1.0;
        }
        let raw = self.get_value();
        let normalized = (raw + 1.0) * 0.5;
        1.0 - self.depth * normalized
    }
}
