use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

const PREFETCH_COUNT: usize = 4;
const MAX_BUFFERED: usize = 16 * 1024;
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DASHHandlerOptions {
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub local_address: Option<String>,
    pub start_time: Option<f64>,
}

impl Default for DASHHandlerOptions {
    fn default() -> Self {
        Self {
            headers: None,
            local_address: None,
            start_time: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DASHRepresentation {
    id: String,
    codecs: String,
    bandwidth: u32,
    #[serde(rename = "audioSamplingRate")]
    audio_sampling_rate: u32,
    #[serde(rename = "initUrl")]
    init_url: String,
    #[serde(rename = "mediaTemplate")]
    media_template: String,
    #[serde(rename = "startNumber")]
    start_number: u32,
    segments: Vec<SegmentGroup>,
}

#[derive(Debug, Clone, Deserialize)]
struct SegmentGroup {
    d: f64,
    #[serde(rename = "r")]
    repeat: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct MPDPeriod {
    #[serde(rename = "AdaptationSet")]
    adaptation_sets: Vec<MPDAdaptationSet>,
}

#[derive(Debug, Clone, Deserialize)]
struct MPDAdaptationSet {
    #[serde(rename = "Representation")]
    representations: Vec<DASHRepresentation>,
}

#[derive(Debug, Clone, Deserialize)]
struct MPD {
    #[serde(rename = "Period")]
    periods: Vec<MPDPeriod>,
}

struct SegmentUrlGenerator {
    template: String,
    start_number: u32,
    segments: Vec<SegmentGroup>,
    current_group: usize,
    current_repeat: u32,
    current_number: u32,
}

impl SegmentUrlGenerator {
    fn new(rep: &DASHRepresentation) -> Self {
        Self {
            template: rep.media_template.clone(),
            start_number: rep.start_number,
            segments: rep.segments.clone(),
            current_group: 0,
            current_repeat: 0,
            current_number: rep.start_number,
        }
    }

    fn total_segments(&self) -> usize {
        self.segments.iter().map(|sg| sg.repeat as usize + 1).sum()
    }

    fn segment_duration(&self) -> f64 {
        if self.segments.is_empty() {
            return 0.0;
        }
        let total_duration: f64 = self
            .segments
            .iter()
            .map(|sg| sg.d * (sg.repeat as f64 + 1.0))
            .sum();
        let count = self.total_segments() as f64;
        if count > 0.0 {
            total_duration / count
        } else {
            0.0
        }
    }
}

impl Iterator for SegmentUrlGenerator {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_group >= self.segments.len() {
            return None;
        }

        let seg = &self.segments[self.current_group];
        if self.current_repeat > seg.repeat {
            self.current_group += 1;
            self.current_repeat = 0;
            if self.current_group >= self.segments.len() {
                return None;
            }
        }

        let url = self.template.replace("$Number$", &self.current_number.to_string());
        self.current_number += 1;
        self.current_repeat += 1;
        Some(url)
    }
}

pub struct DASHHandler {
    mpd_url: String,
    options: DASHHandlerOptions,
    client: Client,
    stopped: bool,
    sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
}

impl DASHHandler {
    pub fn new(mpd_url: String, options: DASHHandlerOptions) -> Self {
        let client = Client::builder()
            .local_address(options.local_address.as_deref().and_then(|s| s.parse().ok()))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            mpd_url,
            options,
            client,
            stopped: false,
            sender: None,
        }
    }

    pub fn start(&mut self) -> UnboundedReceiverStream<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sender = Some(tx);

        let mpd_url = self.mpd_url.clone();
        let options = self.options.clone();
        let client = self.client.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_stream(mpd_url, options, client, sender).await {
                tracing::error!("DASH stream error: {}", e);
            }
        });

        UnboundedReceiverStream::new(rx)
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.sender.take();
    }

    async fn run_stream(
        mpd_url: String,
        options: DASHHandlerOptions,
        client: Client,
        sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
    ) -> anyhow::Result<()> {
        let response = client.get(&mpd_url).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("MPD fetch failed: {}", response.status());
        }
        let mpd_content = response.text().await?;

        let mpd: MPD = quick_xml::de::from_str(&mpd_content)?;

        let mut representations = Vec::new();
        for period in &mpd.periods {
            for adapt in &period.adaptation_sets {
                for rep in &adapt.representations {
                    if rep.codecs != "flac" {
                        representations.push(rep.clone());
                    }
                }
            }
        }

        let selected = representations
            .iter()
            .max_by_key(|r| r.bandwidth)
            .ok_or_else(|| anyhow::anyhow!("No suitable audio representation found in MPD"))?;

        tracing::debug!(
            "DASH: Selected id={}, codecs={}, bandwidth={}",
            selected.id,
            selected.codecs,
            selected.bandwidth
        );

        let init_url = if selected.init_url.starts_with("http") {
            selected.init_url.clone()
        } else {
            let base = mpd_url.rsplit_once('/').map(|(b, _)| b).unwrap_or("");
            format!("{}/{}", base, selected.init_url)
        };

        let init_resp = client.get(&init_url).send().await?;
        if !init_resp.status().is_success() {
            anyhow::bail!("Init segment fetch failed: {}", init_resp.status());
        }
        let init_data = init_resp.bytes().await?;

        if let Some(tx) = &sender {
            let _ = tx.send(init_data.to_vec());
        }

        let mut url_gen = SegmentUrlGenerator::new(selected);
        let total_segments = url_gen.total_segments();
        let segment_duration = url_gen.segment_duration();

        let mut skip_segments = 0;
        if let Some(start_time) = options.start_time {
            if start_time > 0.0 && segment_duration > 0.0 {
                skip_segments = (start_time / (segment_duration * 1000.0)).floor() as usize;
                skip_segments = skip_segments.min(total_segments.saturating_sub(1));
            }
        }

        tracing::debug!(
            "DASH: Total segments: {}, skip: {}, prefetch: {}",
            total_segments,
            skip_segments,
            PREFETCH_COUNT
        );

        for _ in 0..skip_segments {
            url_gen.next();
        }

        let mut fetch_queue: VecDeque<tokio::task::JoinHandle<Option<Vec<u8>>>> = VecDeque::new();
        let mut fetch_index = skip_segments;

        while fetch_queue.len() < PREFETCH_COUNT && fetch_index < total_segments {
            if let Some(url) = url_gen.next() {
                let client = client.clone();
                let headers = options.headers.clone();
                fetch_queue.push_back(tokio::spawn(async move {
                    Self::fetch_segment_with_retry(&client, &url, headers).await
                }));
                fetch_index += 1;
            } else {
                break;
            }
        }

        let mut push_index = skip_segments;

        while push_index < total_segments {
            if fetch_queue.is_empty() {
                break;
            }

            let handle = fetch_queue.pop_front().unwrap();
            let data = handle.await.unwrap_or(None);

            if let Some(data) = data {
                if let Some(tx) = &sender {
                    if tx.send(data).is_err() {
                        break;
                    }
                }
            }

            if fetch_index < total_segments {
                if let Some(url) = url_gen.next() {
                    let client = client.clone();
                    let headers = options.headers.clone();
                    fetch_queue.push_back(tokio::spawn(async move {
                        Self::fetch_segment_with_retry(&client, &url, headers).await
                    }));
                    fetch_index += 1;
                }
            }

            push_index += 1;

            if segment_duration > 0.0 && push_index < total_segments {
                let pace_ms = ((segment_duration * 1000.0) * 0.8).min(5000.0) as u64;
                tokio::time::sleep(tokio::time::Duration::from_millis(pace_ms)).await;
            }
        }

        tracing::debug!("DASH: All {} segments pushed", push_index);
        Ok(())
    }

    async fn fetch_segment_with_retry(
        client: &Client,
        url: &str,
        headers: Option<std::collections::HashMap<String, String>>,
    ) -> Option<Vec<u8>> {
        for attempt in 1..=MAX_RETRIES {
            let mut req = client.get(url);
            if let Some(h) = &headers {
                for (k, v) in h {
                    req = req.header(k, v);
                }
            }

            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.bytes().await {
                        Ok(bytes) => return Some(bytes.to_vec()),
                        Err(_) => {}
                    }
                }
                Ok(resp) => {
                    tracing::warn!("DASH: Segment fetch failed: HTTP {}", resp.status());
                }
                Err(e) => {
                    tracing::warn!("DASH: Segment fetch error (attempt {}/{}): {}", attempt, MAX_RETRIES, e);
                }
            }

            if attempt < MAX_RETRIES {
                let delay = 2_u64.pow(attempt) * 500;
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_generator() {
        let rep = DASHRepresentation {
            id: "test".to_string(),
            codecs: "mp4a.40.2".to_string(),
            bandwidth: 128000,
            audio_sampling_rate: 44100,
            init_url: "init.mp4".to_string(),
            media_template: "seg_$Number$.m4s".to_string(),
            start_number: 1,
            segments: vec![
                SegmentGroup { d: 4.0, repeat: 2 },
                SegmentGroup { d: 4.0, repeat: 0 },
            ],
        };

        let mut gen = SegmentUrlGenerator::new(&rep);
        assert_eq!(gen.next(), Some("seg_1.m4s".to_string()));
        assert_eq!(gen.next(), Some("seg_2.m4s".to_string()));
        assert_eq!(gen.next(), Some("seg_3.m4s".to_string()));
        assert_eq!(gen.next(), Some("seg_4.m4s".to_string()));
        assert_eq!(gen.total_segments(), 4);
    }
}