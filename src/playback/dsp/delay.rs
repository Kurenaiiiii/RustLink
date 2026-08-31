pub struct DelayLine {
    buffer: Vec<i16>,
    size: usize,
    write_index: usize,
}

impl DelayLine {
    pub fn new(size: usize) -> Self {
        Self {
            buffer: vec![0i16; size],
            size,
            write_index: 0,
        }
    }

    pub fn write(&mut self, sample: f64) {
        self.buffer[self.write_index] = sample.round().clamp(-32768.0, 32767.0) as i16;
        self.write_index = (self.write_index + 1) % self.size;
    }

    pub fn read(&self, delay_in_samples: f64) -> f64 {
        let safe_delay = delay_in_samples.floor().clamp(0.0, (self.size - 1) as f64) as usize;
        let read_index = (self.write_index + self.size - safe_delay) % self.size;
        self.buffer[read_index] as f64
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0);
    }
}
