use opus::{self, Channels};
use std::fmt;

#[derive(Debug, Clone)]
pub struct OpusError {
    pub description: String,
}

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opus error: {}", self.description)
    }
}

impl std::error::Error for OpusError {}

impl From<opus::Error> for OpusError {
    fn from(e: opus::Error) -> Self {
        OpusError {
            description: e.description().to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum OpusMode {
    Audio,
    Voice,
    LowDelay,
}

fn channels_to_opus(ch: u32) -> Channels {
    match ch {
        1 => Channels::Mono,
        _ => Channels::Stereo,
    }
}

fn mode_to_application(mode: OpusMode) -> opus::Application {
    match mode {
        OpusMode::Audio => opus::Application::Audio,
        OpusMode::Voice => opus::Application::Voip,
        OpusMode::LowDelay => opus::Application::LowDelay,
    }
}

pub struct OpusDecoder {
    decoder: Option<opus::Decoder>,
    sample_rate: u32,
    channels: u32,
    mode: OpusMode,
}

unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new(sample_rate: u32, channels: u32, _mode: OpusMode) -> Self {
        let decoder = opus::Decoder::new(sample_rate, channels_to_opus(channels)).ok();
        Self {
            decoder,
            sample_rate,
            channels,
            mode: _mode,
        }
    }

    pub fn decode(&mut self, input: &[u8], output: &mut [f32], fec: bool) -> Result<usize, OpusError> {
        match &mut self.decoder {
            Some(dec) => dec.decode_float(input, output, fec).map_err(OpusError::from),
            None => Err(OpusError { description: "decoder not initialized".to_string() }),
        }
    }

    pub fn decode_packet(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, OpusError> {
        self.decode(packet, output, false)
    }

    pub fn reset(&mut self) {
        if let Some(ref mut dec) = self.decoder {
            let _ = dec.reset_state();
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn mode(&self) -> OpusMode {
        self.mode
    }
}

pub struct OpusEncoder {
    encoder: Option<opus::Encoder>,
    sample_rate: u32,
    channels: u32,
    application: opus::Application,
    bitrate: i32,
}

unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    pub fn new(sample_rate: u32, channels: u32, mode: OpusMode) -> Self {
        let application = mode_to_application(mode);
        let encoder = opus::Encoder::new(sample_rate, channels_to_opus(channels), application).ok();
        Self {
            encoder,
            sample_rate,
            channels,
            application,
            bitrate: -1000,
        }
    }

    pub fn encode(&mut self, input: &[f32], output: &mut [u8]) -> Result<usize, OpusError> {
        match &mut self.encoder {
            Some(enc) => enc.encode_float(input, output).map_err(OpusError::from),
            None => Err(OpusError { description: "encoder not initialized".to_string() }),
        }
    }

    pub fn set_bitrate(&mut self, bitrate: i32) -> Result<(), OpusError> {
        self.bitrate = bitrate;
        match &mut self.encoder {
            Some(enc) => enc.set_bitrate(opus::Bitrate::Bits(bitrate)).map_err(OpusError::from),
            None => Err(OpusError { description: "encoder not initialized".to_string() }),
        }
    }

    pub fn set_complexity(&mut self, complexity: i32) -> Result<(), OpusError> {
        match &mut self.encoder {
            Some(enc) => enc.set_complexity(complexity).map_err(OpusError::from),
            None => Err(OpusError { description: "encoder not initialized".to_string() }),
        }
    }

    pub fn reset(&mut self) {
        if let Some(ref mut enc) = self.encoder {
            let _ = enc.reset_state();
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn bitrate(&self) -> i32 {
        self.bitrate
    }
}
