use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::error;

use crate::playback::processors::{
    FadeTransformer, FlowConfig, FlowController, ScratchSettings, ScratchTransformer,
    SilenceDetector, TapeDirection, TapeTransformer,
};
use crate::player::audio_pipeline::{AudioPipeline, ResampleQuality};
use crate::player::fading::{TapeAction, TapeFade, VolumeFade};
use crate::player::filter_chain::FilterChain;
use crate::player::loudness::LoudnessNormalizer;
use crate::player::volume::{FadeCurve, VolumeTransformer};

const OPUS_FRAME_MS: u64 = 20;
const TARGET_SAMPLE_RATE: u32 = 48000;

pub struct PCMFrameCounter {
    position: Arc<AtomicU64>,
    channels: usize,
}

impl PCMFrameCounter {
    pub fn new(position: Arc<AtomicU64>, channels: usize) -> Self {
        Self { position, channels }
    }

    pub fn process_frame(&mut self, frame_samples: usize) {
        let ms = (frame_samples as u64 * 1000) / (TARGET_SAMPLE_RATE as u64);
        self.position.fetch_add(ms.max(OPUS_FRAME_MS), Ordering::SeqCst);
    }

    pub fn position_ms(&self) -> u64 {
        self.position.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct AudioResourceOptions {
    pub resample_quality: String,
    pub loudness_enabled: bool,
    pub loudness_lookahead_ms: u64,
    pub loudness_gate_threshold: f64,
    pub fade_enabled: bool,
    pub track_start_kind: String,
    pub track_start_duration: f32,
    pub track_start_curve: String,
    pub track_end_kind: String,
    pub track_end_duration: f32,
    pub track_end_curve: String,
    pub pause_kind: String,
    pub pause_duration: f32,
    pub pause_curve: String,
    pub resume_kind: String,
    pub resume_duration: f32,
    pub resume_curve: String,
    pub crossfade_enabled: bool,
    pub crossfade_duration: f32,
    pub crossfade_curve: String,
    pub track_length_ms: u64,
}

impl Default for AudioResourceOptions {
    fn default() -> Self {
        Self {
            resample_quality: "best".to_string(),
            loudness_enabled: false,
            loudness_lookahead_ms: 0,
            loudness_gate_threshold: -30.0,
            fade_enabled: false,
            track_start_kind: String::new(),
            track_start_duration: 0.0,
            track_start_curve: String::new(),
            track_end_kind: String::new(),
            track_end_duration: 0.0,
            track_end_curve: String::new(),
            pause_kind: String::new(),
            pause_duration: 0.0,
            pause_curve: String::new(),
            resume_kind: String::new(),
            resume_duration: 0.0,
            resume_curve: String::new(),
            crossfade_enabled: false,
            crossfade_duration: 0.0,
            crossfade_curve: String::new(),
            track_length_ms: 0,
        }
    }
}

type EventCallback = Box<dyn Fn(&str, &serde_json::Value) + Send>;

pub struct StreamAudioResource {
    pipeline: AudioPipeline,
    channels: usize,
    sample_rate: u32,
    frame_samples: usize,
    position: Arc<AtomicU64>,
    frame_counter: PCMFrameCounter,
    loudness_normalizer: Option<LoudnessNormalizer>,
    filter_chain: Arc<Mutex<FilterChain>>,
    volume_fade: VolumeFade,
    volume_transformer: VolumeTransformer,
    tape_fade: TapeFade,
    tape_buffer: Vec<f32>,
    volume: u32,
    crossfade_buffer: Vec<f32>,
    options: AudioResourceOptions,
    track_end_fade_triggered: bool,
    silence_detector: Option<SilenceDetector>,
    flow_controller: Option<FlowController>,
    tape_transformer: Option<TapeTransformer>,
    scratch_transformer: Option<ScratchTransformer>,
    fade_transformer: Option<FadeTransformer>,
    rms_accumulator: f64,
    rms_sample_count: u64,
    stream_paused: bool,
    event_listeners: Vec<EventCallback>,
}

impl StreamAudioResource {
    pub async fn new(url: &str, options: AudioResourceOptions) -> anyhow::Result<Self> {
        let quality = ResampleQuality::from_str(&options.resample_quality);
        let pipeline = AudioPipeline::new(url, Some(quality)).await?;
        Ok(Self::from_pipeline_inner(pipeline, options))
    }

    pub fn from_pipeline(pipeline: AudioPipeline, options: AudioResourceOptions) -> Self {
        Self::from_pipeline_inner(pipeline, options)
    }

    fn from_pipeline_inner(pipeline: AudioPipeline, options: AudioResourceOptions) -> Self {
        let channels = pipeline.channels();
        let sample_rate = pipeline.sample_rate();
        let frame_samples = 960 * channels;
        let position = Arc::new(AtomicU64::new(0));
        let frame_counter = PCMFrameCounter::new(position.clone(), channels);

        let loudness = if options.loudness_enabled && options.loudness_lookahead_ms > 0 {
            Some(LoudnessNormalizer::new(
                channels,
                sample_rate,
                options.loudness_lookahead_ms,
                -14.0,
                options.loudness_gate_threshold,
            ))
        } else {
            None
        };

        let mut volume_fade = VolumeFade::new();
        let mut tape_fade = TapeFade::new();

        if options.fade_enabled && options.track_start_duration > 0.0 {
            match options.track_start_kind.as_str() {
                "tape" => tape_fade.trigger(
                    TapeAction::Start,
                    options.track_start_duration,
                    &options.track_start_curve,
                ),
                _ => volume_fade.trigger(
                    1.0,
                    options.track_start_duration,
                    &options.track_start_curve,
                ),
            }
        }

        let volume_transformer = VolumeTransformer::new(
            sample_rate,
            channels,
            1.0,
            0.0,
            FadeCurve::Sinusoidal,
            0.95,
            0.4,
            false,
            5.0,
            None,
        );

        Self {
            pipeline,
            channels,
            sample_rate,
            frame_samples,
            position,
            frame_counter,
            loudness_normalizer: loudness,
            filter_chain: Arc::new(Mutex::new(FilterChain::default())),
            volume_fade,
            volume_transformer,
            tape_fade,
            tape_buffer: Vec::new(),
            volume: 100,
            crossfade_buffer: Vec::new(),
            options,
            track_end_fade_triggered: false,
            silence_detector: Some(SilenceDetector::new(-40.0, 500, sample_rate)),
            flow_controller: Some(FlowController::new(
                FlowConfig {
                    target_bitrate: 128000,
                    max_buffered_ms: 3000,
                    min_buffered_ms: 500,
                },
                channels,
            )),
            tape_transformer: Some(TapeTransformer::new(channels)),
            scratch_transformer: Some(ScratchTransformer::new(500.0, channels)),
            fade_transformer: Some(FadeTransformer::new()),
            rms_accumulator: 0.0,
            rms_sample_count: 0,
            stream_paused: false,
            event_listeners: Vec::new(),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn position(&self) -> u64 {
        self.position.load(Ordering::SeqCst)
    }

    pub fn set_position(&self, pos: u64) {
        self.position.store(pos, Ordering::SeqCst);
    }

    pub fn position_arc(&self) -> Arc<AtomicU64> {
        self.position.clone()
    }

    pub fn filter_chain(&self) -> Arc<Mutex<FilterChain>> {
        self.filter_chain.clone()
    }

    pub fn set_volume(&mut self, vol: u32) {
        self.volume = vol.min(1000);
        self.volume_transformer.set_volume(self.volume as f32 / 100.0);
    }

    pub fn trigger_fade(&mut self, kind: &str, duration: f32, curve: &str) {
        match kind {
            "tape" => self.tape_fade.trigger(TapeAction::Start, duration, curve),
            _ => self.volume_fade.trigger(1.0, duration, curve),
        }
    }

    pub fn trigger_stop_fade(&mut self, kind: &str, duration: f32, curve: &str) {
        match kind {
            "tape" => self.tape_fade.trigger(TapeAction::Stop, duration, curve),
            _ => self.volume_fade.trigger(0.0, duration, curve),
        }
    }

    pub fn is_tape_active(&self) -> bool {
        self.tape_fade.is_active()
    }

    pub fn is_volume_fade_active(&self) -> bool {
        self.volume_fade.active
    }

    pub fn seek_to(&mut self, position_ms: u64) -> anyhow::Result<()> {
        self.pipeline.seek_to(position_ms)?;
        self.position.store(position_ms, Ordering::SeqCst);
        self.tape_buffer.clear();
        self.crossfade_buffer.clear();
        self.track_end_fade_triggered = false;
        self.rms_accumulator = 0.0;
        self.rms_sample_count = 0;
        Ok(())
    }

    pub fn next_raw_frame(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        self.pipeline.next_pcm_frame()
    }

    pub fn next_processed_frame(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        let pcm = match self.pipeline.next_pcm_frame()? {
            Some(f) => f,
            None => return Ok(None),
        };

        let channels = self.channels;
        let frame_samples = self.frame_samples;
        let position_ms = self.position.load(Ordering::SeqCst);

        // Crossfade buffer accumulation
        if self.options.crossfade_enabled && self.options.crossfade_duration > 0.0 {
            let max_samp = ((self.options.crossfade_duration / 20.0) as usize) * frame_samples;
            self.crossfade_buffer.extend_from_slice(&pcm);
            if self.crossfade_buffer.len() > max_samp {
                let drain = self.crossfade_buffer.len() - max_samp;
                self.crossfade_buffer.drain(..drain);
            }
        }

        // Scheduled track end fade
        let track_length_ms = self.options.track_length_ms;
        let track_end_duration = self.options.track_end_duration;
        let fade_enabled = self.options.fade_enabled;
        if !self.track_end_fade_triggered
            && fade_enabled
            && track_end_duration > 0.0
            && track_length_ms > 0
        {
            let fade_start_ms = track_length_ms.saturating_sub(track_end_duration as u64);
            if position_ms >= fade_start_ms {
                let kind = self.options.track_end_kind.clone();
                let curve = self.options.track_end_curve.clone();
                self.track_end_fade_triggered = true;
                self.trigger_stop_fade(&kind, track_end_duration, &curve);
            }
        }

        let mut frame = pcm;

        // Loudness normalizer
        if let Some(ref mut ln) = self.loudness_normalizer {
            ln.process(&mut frame);
        }

        // Filter chain
        {
            let mut fc = self.filter_chain.lock().unwrap();
            fc.process(&mut frame, channels);
        }

        // Tape fade (start/stop speed effect)
        let mut tape_out = Vec::new();
        self.tape_fade.process(&frame, &mut tape_out, channels, OPUS_FRAME_MS as f32);
        self.tape_buffer.extend(tape_out);

        // TapeTransformer (playback rate changes)
        if let Some(ref mut tt) = self.tape_transformer {
            tt.process(&mut self.tape_buffer, channels);
        }

        // ScratchTransformer (vinyl scratch effect)
        if let Some(ref mut st) = self.scratch_transformer {
            let scratch = ScratchSettings {
                frequency: 0.5,
                amplitude: 0.0,
                position: 0.0,
            };
            if scratch.amplitude > 0.0 {
                st.process(&mut self.tape_buffer, channels, &scratch);
            }
        }

        // Volume adjustment with soft-knee limiter and lookahead
        let process_len = self.tape_buffer.len().min(frame_samples);
        if process_len > 0 {
            self.volume_transformer.process(&mut self.tape_buffer[..process_len]);
        }

        if self.tape_buffer.len() < frame_samples {
            return Ok(None);
        }

        let mut output: Vec<f32> = self.tape_buffer.drain(..frame_samples).collect();

        // FadeTransformer (standalone fades)
        if let Some(ref mut ft) = self.fade_transformer {
            ft.process(&mut output, OPUS_FRAME_MS as f32);
        }

        // VolumeFade (existing fade mechanism)
        self.volume_fade.process(&mut output, OPUS_FRAME_MS as f32);

        // SilenceDetector
        if let Some(ref mut sd) = self.silence_detector {
            sd.process(&output);
        }

        // RMS computation
        for &s in &output {
            self.rms_accumulator += (s as f64) * (s as f64);
        }
        self.rms_sample_count += output.len() as u64;

        // FlowController
        if let Some(ref mut fc) = self.flow_controller {
            fc.push_pcm(&output);
            let metrics = fc.metrics();
            self.stream_paused = metrics.paused;
        }

        self.frame_counter.process_frame(frame_samples);
        Ok(Some(output))
    }

    pub fn drain_tape_buffer(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.tape_buffer)
    }

    pub fn crossfade_buffer(&self) -> &[f32] {
        &self.crossfade_buffer
    }

    // --- BaseAudioResource interface methods ---

    pub fn get_rms(&self) -> Option<f64> {
        if self.rms_sample_count == 0 {
            return None;
        }
        Some((self.rms_accumulator / self.rms_sample_count as f64).sqrt())
    }

    pub fn is_silent(&self) -> bool {
        self.silence_detector.as_ref().map_or(false, |sd| sd.is_silent())
    }

    pub fn get_effective_rate(&self) -> f32 {
        let mut rate = 1.0;
        if let Some(ref tt) = self.tape_transformer {
            rate *= tt.playback_rate;
        }
        rate
    }

    pub fn set_scratch(&mut self, frequency: f32, amplitude: f32, position: f32) {
        self.scratch_transformer = Some(ScratchTransformer::new(500.0, self.channels));
    }

    pub fn fade_to(&mut self, target_gain: f32, duration_ms: f32, curve: &str) {
        if let Some(ref mut ft) = self.fade_transformer {
            if target_gain >= 0.5 {
                ft.fade_in(duration_ms, curve);
            } else {
                ft.fade_out(duration_ms, curve);
            }
        }
    }

    pub fn set_tape_rate(&mut self, rate: f32) {
        if let Some(ref mut tt) = self.tape_transformer {
            tt.set_target_rate(rate);
        }
    }

    pub fn set_tape_direction(&mut self, dir: TapeDirection) {
        if let Some(ref mut tt) = self.tape_transformer {
            tt.set_direction(dir);
        }
    }

    pub fn resume_stream(&mut self) {
        self.stream_paused = false;
        if let Some(ref mut fc) = self.flow_controller {
            fc.clear();
        }
    }

    pub fn is_stream_paused(&self) -> bool {
        self.stream_paused
    }

    pub fn read(&mut self) -> Option<Vec<f32>> {
        self.next_processed_frame().ok().flatten()
    }

    pub fn on_event<F: Fn(&str, &serde_json::Value) + Send + 'static>(&mut self, callback: F) {
        self.event_listeners.push(Box::new(callback));
    }

    fn emit_event(&self, event_type: &str, data: &serde_json::Value) {
        for cb in &self.event_listeners {
            cb(event_type, data);
        }
    }
}

pub async fn create_audio_resource(
    url: &str,
    options: AudioResourceOptions,
) -> anyhow::Result<StreamAudioResource> {
    StreamAudioResource::new(url, options).await
}

pub async fn create_seekable_audio_resource(
    url: &str,
    options: AudioResourceOptions,
    start_position_ms: u64,
) -> anyhow::Result<StreamAudioResource> {
    let mut resource = StreamAudioResource::new(url, options).await?;
    if start_position_ms > 0 {
        resource.seek_to(start_position_ms)?;
    }
    Ok(resource)
}

pub async fn create_pcm_stream(
    url: &str,
) -> anyhow::Result<mpsc::UnboundedReceiver<Vec<f32>>> {
    let quality = ResampleQuality::Best;
    let mut pipeline = AudioPipeline::new(url, Some(quality)).await?;
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            match pipeline.next_pcm_frame() {
                Ok(Some(frame)) => {
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    error!(target: "Resource", "PCM stream error: {e}");
                    break;
                }
            }
        }
    });

    Ok(rx)
}

pub async fn crossfade_transition(
    crossfade_buffer: &[f32],
    new_url: &str,
    channels: usize,
    duration_ms: f32,
    curve: &str,
    resample_quality: &str,
) -> anyhow::Result<(AudioPipeline, Vec<f32>)> {
    let quality = ResampleQuality::from_str(resample_quality);
    let mut new_pipeline = AudioPipeline::new(new_url, Some(quality)).await?;

    let frame_samples = 960 * channels;
    let fade_frames = (duration_ms / 20.0) as usize;
    let avail_frames = crossfade_buffer.len() / frame_samples;
    let use_frames = fade_frames.min(avail_frames);

    let mut mixed_buffer = Vec::new();
    let mut old_fade = VolumeFade::new();
    old_fade.trigger(0.0, duration_ms, curve);
    let mut new_fade = VolumeFade::new();
    new_fade.trigger(1.0, duration_ms, curve);

    for i in 0..use_frames {
        let start = i * frame_samples;
        let end = start + frame_samples;
        let mut old_chunk = crossfade_buffer[start..end].to_vec();
        let mut new_chunk = new_pipeline
            .next_pcm_frame()
            .ok()
            .flatten()
            .unwrap_or_else(|| vec![0.0; frame_samples]);

        old_fade.process(&mut old_chunk, 20.0);
        new_fade.process(&mut new_chunk, 20.0);

        for s in 0..frame_samples {
            old_chunk[s] += new_chunk[s];
        }

        mixed_buffer.extend_from_slice(&old_chunk);
    }

    Ok((new_pipeline, mixed_buffer))
}