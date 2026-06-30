use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use anyhow::Context;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use tokio::sync::mpsc;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::Decoder as SymphDecoder;
use symphonia::core::formats::FormatReader;
use symphonia::core::formats::{SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::probe::Hint;

const OPUS_FRAME_SIZE: usize = 960;
const TARGET_SAMPLE_RATE: u32 = 48000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResampleQuality {
    Best,
    Medium,
    Fast,
}

impl ResampleQuality {
    pub fn from_str(s: &str) -> Self {
        match s {
            "best" | "high" => ResampleQuality::Best,
            "medium" => ResampleQuality::Medium,
            "fast" | "low" => ResampleQuality::Fast,
            _ => ResampleQuality::Best,
        }
    }
}

struct ResampleEngine {
    resampler: Fft<f32>,
    input_buffer: Vec<f32>,
    output_buffer: Vec<f32>,
    channels: usize,
    input_frames_needed: usize,
    output_frames_per_chunk: usize,
}

impl ResampleEngine {
    fn new(input_rate: u32, output_rate: u32, channels: usize, _quality: ResampleQuality) -> anyhow::Result<Self> {
        let chunk_size = 1024;
        let max_frames_reservoir = 2;
        let resampler = Fft::<f32>::new(
            output_rate as usize,
            input_rate as usize,
            chunk_size,
            channels,
            max_frames_reservoir,
            FixedSync::Both,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create FFT resampler: {e:?}"))?;

        let input_frames_needed = resampler.input_frames_next();
        let output_frames_per_chunk = resampler.output_frames_next();
        let output_buffer = vec![0.0; output_frames_per_chunk * channels];

        Ok(Self {
            resampler,
            input_buffer: Vec::new(),
            output_buffer,
            channels,
            input_frames_needed,
            output_frames_per_chunk,
        })
    }

    fn push_input(&mut self, samples: &[f32]) {
        self.input_buffer.extend_from_slice(samples);
    }

    fn drain_resampled(&mut self, output: &mut Vec<f32>) -> anyhow::Result<()> {
        let channels = self.channels;
        let input_frames_needed = self.input_frames_needed;
        let output_frames_per_chunk = self.output_frames_per_chunk;

        while self.input_buffer.len() / channels >= input_frames_needed {
            let n_samples = input_frames_needed * channels;

            let (input_slice, rest) = self.input_buffer.split_at(n_samples);
            let input_adapter = InterleavedSlice::new(input_slice, channels, input_frames_needed)
                .map_err(|e| anyhow::anyhow!("InterleavedSlice input error: {e:?}"))?;

            let out_samples = output_frames_per_chunk * channels;
            if self.output_buffer.len() < out_samples {
                self.output_buffer.resize(out_samples, 0.0);
            }
            let output_slice = &mut self.output_buffer[..out_samples];
            let mut output_adapter =
                InterleavedSlice::new_mut(output_slice, channels, output_frames_per_chunk)
                    .map_err(|e| anyhow::anyhow!("InterleavedSlice output error: {e:?}"))?;

            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            self.resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .map_err(|e| anyhow::anyhow!("Resample error: {e:?}"))?;

            self.input_buffer = rest.to_vec();
            output.extend_from_slice(&self.output_buffer[..out_samples]);
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.input_buffer.clear();
        let _ = self.resampler.reset();
    }
}

// --- Streaming HTTP source ---

struct SharedState {
    data: Vec<u8>,
    finished: bool,
    error: Option<String>,
}

pub struct StreamingSource {
    inner: Arc<(Mutex<SharedState>, Condvar)>,
    pos: u64,
    content_length: Option<u64>,
}

impl StreamingSource {
    fn new(content_length: Option<u64>) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(SharedState {
                    data: Vec::new(),
                    finished: false,
                    error: None,
                }),
                Condvar::new(),
            )),
            pos: 0,
            content_length,
        }
    }

    fn writer(&self) -> StreamWriter {
        StreamWriter {
            inner: self.inner.clone(),
        }
    }

    fn buffered_len(&self) -> usize {
        let (lock, _) = &*self.inner;
        lock.lock().unwrap().data.len()
    }
}

struct StreamWriter {
    inner: Arc<(Mutex<SharedState>, Condvar)>,
}

impl StreamWriter {
    fn append(&self, bytes: &[u8]) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.data.extend_from_slice(bytes);
        cvar.notify_all();
    }

    fn finish(&self) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.finished = true;
        cvar.notify_all();
    }

    fn set_error(&self, error: String) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.error = Some(error);
        state.finished = true;
        cvar.notify_all();
    }
}

impl Read for StreamingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();
        loop {
            if (self.pos as usize) < state.data.len() {
                let available = state.data.len() - self.pos as usize;
                let to_read = buf.len().min(available);
                let src = &state.data[self.pos as usize..self.pos as usize + to_read];
                buf[..to_read].copy_from_slice(src);
                self.pos += to_read as u64;
                return Ok(to_read);
            }
            if state.finished {
                return Ok(0);
            }
            if let Some(ref err) = state.error {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, err.clone()));
            }
            state = cvar.wait(state).unwrap();
        }
    }
}

impl Seek for StreamingSource {
    fn seek(&mut self, style: SeekFrom) -> std::io::Result<u64> {
        let (lock, _) = &*self.inner;
        let state = lock.lock().unwrap();
        let bound = state.data.len() as u64;
        match style {
            SeekFrom::Start(p) => self.pos = p.min(bound),
            SeekFrom::End(_) => self.pos = bound,
            SeekFrom::Current(off) => {
                let new = (self.pos as i64 + off).max(0) as u64;
                self.pos = new.min(bound);
            }
        }
        Ok(self.pos)
    }
}

impl MediaSource for StreamingSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

// --- Audio pipeline ---

pub struct AudioPipeline {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphDecoder>,
    track_id: u32,
    channels: usize,
    sample_rate: u32,
    resampler: Option<ResampleEngine>,
    reached_end: bool,
    pcm_queue: Vec<f32>,
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

pub fn encode_opus_frame(pcm: &[f32], encoder: &mut opus::Encoder) -> anyhow::Result<Vec<u8>> {
    let mut pcm_i16 = vec![0i16; pcm.len()];
    for (i, sample) in pcm_i16.iter_mut().enumerate() {
        *sample = f32_to_i16(pcm[i]);
    }
    let mut opus_buf = vec![0u8; 4096];
    let len = encoder
        .encode(&pcm_i16, &mut opus_buf)
        .map_err(|e| anyhow::anyhow!("Opus encode error: {e:?}"))?;
    opus_buf.truncate(len);
    Ok(opus_buf)
}

impl AudioPipeline {
    pub async fn new(url: &str, resample_quality: Option<ResampleQuality>) -> anyhow::Result<Self> {
        let response = reqwest::get(url)
            .await
            .context("Failed to fetch audio stream")?;
        let content_length = response.content_length();
        let source = StreamingSource::new(content_length);
        let writer = source.writer();

        tokio::spawn(async move {
            let mut resp = response;
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => writer.append(&chunk),
                    Ok(None) => {
                        writer.finish();
                        break;
                    }
                    Err(e) => {
                        writer.set_error(e.to_string());
                        break;
                    }
                }
            }
        });

        // Wait for minimum data before probing (up to ~5s)
        for _ in 0..50 {
            if source.buffered_len() >= 65536 || {
                let (lock, _) = &*source.inner;
                lock.lock().unwrap().finished
            } {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let mss = MediaSourceStream::new(Box::new(source), Default::default());

        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &Default::default(), &Default::default())
            .context("Symphonia probe failed")?;
        let format = probed.format;

        let track = format
            .tracks()
            .first()
            .ok_or_else(|| anyhow::anyhow!("No tracks in stream"))?;

        let track_id = track.id;
        let codec_params = track.codec_params.clone();

        let channels = codec_params
            .channels
            .map(|c| c.count() as usize)
            .unwrap_or(2);

        let sample_rate = codec_params.sample_rate.unwrap_or(TARGET_SAMPLE_RATE);

        let decoder = symphonia::default::get_codecs()
            .make(&codec_params, &Default::default())
            .context("Symphonia failed to create decoder")?;

        let resampler = if sample_rate != TARGET_SAMPLE_RATE {
            let quality = resample_quality.unwrap_or(ResampleQuality::Best);
            Some(ResampleEngine::new(sample_rate, TARGET_SAMPLE_RATE, channels, quality)?)
        } else {
            None
        };

        Ok(Self {
            format,
            decoder,
            track_id,
            channels,
            sample_rate,
            resampler,
            reached_end: false,
            pcm_queue: Vec::new(),
        })
    }

    pub async fn from_channel(
        mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
        resample_quality: Option<ResampleQuality>,
    ) -> anyhow::Result<Self> {
        let source = StreamingSource::new(None);
        let writer = source.writer();

        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                writer.append(&chunk);
            }
            writer.finish();
        });

        // Wait for minimum data before probing
        for _ in 0..50 {
            if source.buffered_len() >= 65536 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &Default::default(), &Default::default())
            .context("Symphonia probe failed for SABR stream")?;
        let format = probed.format;
        let track = format
            .tracks()
            .first()
            .ok_or_else(|| anyhow::anyhow!("No tracks in SABR stream"))?;
        let track_id = track.id;
        let codec_params = track.codec_params.clone();
        let channels = codec_params
            .channels
            .map(|c| c.count() as usize)
            .unwrap_or(2);
        let sample_rate = codec_params.sample_rate.unwrap_or(TARGET_SAMPLE_RATE);
        let decoder = symphonia::default::get_codecs()
            .make(&codec_params, &Default::default())
            .context("Symphonia failed to create decoder for SABR stream")?;
        let resampler = if sample_rate != TARGET_SAMPLE_RATE {
            let quality = resample_quality.unwrap_or(ResampleQuality::Best);
            Some(ResampleEngine::new(sample_rate, TARGET_SAMPLE_RATE, channels, quality)?)
        } else {
            None
        };

        Ok(Self {
            format,
            decoder,
            track_id,
            channels,
            sample_rate,
            resampler,
            reached_end: false,
            pcm_queue: Vec::new(),
        })
    }

    pub fn seek_to(&mut self, time_ms: u64) -> anyhow::Result<()> {
        let seek_to = SeekTo::Time {
            time: Duration::from_millis(time_ms).into(),
            track_id: Some(self.track_id),
        };
        self.format.seek(SeekMode::Accurate, seek_to)?;
        let codec_params = self.format.tracks().first()
            .ok_or_else(|| anyhow::anyhow!("No tracks after seek"))?
            .codec_params
            .clone();
        self.decoder = symphonia::default::get_codecs()
            .make(&codec_params, &Default::default())
            .context("Symphonia failed to create decoder after seek")?;
        if let Some(ref mut resampler) = self.resampler {
            resampler.reset();
        }
        self.pcm_queue.clear();
        self.reached_end = false;
        Ok(())
    }

    fn decode_packets_into_queue(&mut self) -> anyhow::Result<()> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(pkt) => pkt,
                Err(symphonia::core::errors::Error::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.reached_end = true;
                    return Ok(());
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => return Err(anyhow::anyhow!("Symphonia packet error: {e:?}")),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => return Err(anyhow::anyhow!("Symphonia decode error: {e:?}")),
            };

            let spec = *decoded.spec();
            let num_frames = decoded.frames();
            let num_channels = spec.channels.count() as usize;
            let mut sample_buf =
                SampleBuffer::<f32>::new(num_frames as u64 * num_channels as u64, spec);
            sample_buf.copy_interleaved_ref(decoded);

            if let Some(ref mut resampler) = self.resampler {
                resampler.push_input(sample_buf.samples());
                resampler.drain_resampled(&mut self.pcm_queue)?;
            } else {
                self.pcm_queue.extend_from_slice(sample_buf.samples());
            }
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn next_pcm_frame(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        let needed = OPUS_FRAME_SIZE * self.channels.max(2);
        while self.pcm_queue.len() < needed && !self.reached_end {
            self.decode_packets_into_queue()?;
        }

        if self.pcm_queue.len() < needed {
            return Ok(None);
        }

        let frame = self.pcm_queue[..needed].to_vec();
        self.pcm_queue.drain(0..needed);
        Ok(Some(frame))
    }
}
