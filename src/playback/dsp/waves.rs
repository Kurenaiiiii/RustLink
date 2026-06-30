pub type WaveformFn = fn(f64) -> f64;

pub fn sine(phase: f64) -> f64 {
    phase.sin()
}

pub fn square(phase: f64) -> f64 {
    if phase % (2.0 * std::f64::consts::PI) < std::f64::consts::PI { 1.0 } else { -1.0 }
}

pub fn sawtooth(phase: f64) -> f64 {
    (phase % (2.0 * std::f64::consts::PI)) / std::f64::consts::PI - 1.0
}

pub fn triangle(phase: f64) -> f64 {
    let x = (phase % (2.0 * std::f64::consts::PI)) / (2.0 * std::f64::consts::PI);
    2.0 * (if x < 0.5 { 2.0 * x } else { 2.0 - 2.0 * x }) - 1.0
}

pub fn get_waveform(name: &str) -> Option<WaveformFn> {
    match name {
        "SINE" => Some(sine as WaveformFn),
        "SQUARE" => Some(square as WaveformFn),
        "SAWTOOTH" => Some(sawtooth as WaveformFn),
        "TRIANGLE" => Some(triangle as WaveformFn),
        _ => None,
    }
}
