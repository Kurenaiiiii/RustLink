pub struct Float64DelayLine {
    buffer: Vec<f64>,
    size: usize,
    write_index: usize,
}

impl Float64DelayLine {
    pub fn new(size: usize) -> Self {
        let s = size.max(1);
        Self {
            buffer: vec![0.0; s],
            size: s,
            write_index: 0,
        }
    }

    pub fn write(&mut self, sample: f64) {
        self.buffer[self.write_index] = sample;
        self.write_index = (self.write_index + 1) % self.size;
    }

    pub fn read(&self, delay_in_samples: f64) -> f64 {
        if delay_in_samples <= 0.0 {
            let idx = (self.write_index + self.size - 1) % self.size;
            return self.buffer[idx];
        }
        let clamped = delay_in_samples.min((self.size - 1) as f64);
        let int_delay = clamped.floor() as usize;
        let frac = clamped - int_delay as f64;
        let idx0 = (self.write_index + self.size * 2 - int_delay - 1) % self.size;
        let idx1 = (idx0 + self.size - 1) % self.size;
        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];
        s0 + frac * (s1 - s0)
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}
