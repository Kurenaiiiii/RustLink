pub struct Allpass {
    pub x1: f64,
    pub y1: f64,
    a: f64,
}

impl Allpass {
    pub fn new() -> Self {
        Self { x1: 0.0, y1: 0.0, a: 0.0 }
    }

    pub fn set_coefficient(&mut self, a: f64) {
        self.a = a.clamp(-0.999, 0.999);
    }

    pub fn process(&mut self, sample: f64) -> f64 {
        let output = self.a * sample + self.x1 - self.a * self.y1;
        self.x1 = sample;
        self.y1 = output;
        output
    }
}

impl Default for Allpass {
    fn default() -> Self {
        Self::new()
    }
}
