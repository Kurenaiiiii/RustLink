pub fn clamp16bit(sample: f64) -> i16 {
    (sample.round().clamp(-32768.0, 32767.0)) as i16
}
