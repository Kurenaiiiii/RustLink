use crate::playback::hls::types::*;
use crate::playback::hls::playlist_parser::parse_playlist;
use crate::playback::hls::segment_fetcher::{SegmentFetcher, SegmentFetcherOptions, SegmentFetchResult};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use reqwest::Client;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, warn};

const MAX_HISTORY: usize = 200;
const MAX_GAP: i64 = 30;
const MASTER_REFRESH_INTERVAL: u32 = 3;
const LIVE_PRE_ROLL_SEGMENTS: usize = 12;
const STUCK_THRESHOLD: u32 = 10;

pub struct HLSHandler {
    master_url: String,
    current_url: String,
    headers: HashMap<String, String>,
    local_address: Option<String>,
    proxy: Option<String>,
    on_resolve_url: Option<Arc<dyn Fn(String) -> Option<String> + Send + Sync>>,
    strategy: HLSFetchStrategy,
    start_time: f64,
    fetcher: SegmentFetcher,
    processed_segments: HashSet<String>,
    processed_order: VecDeque<String>,
    segment_queue: VecDeque<HLSSegment>,
    max_parallel_fetches: usize,
    is_fetching: bool,
    stop: bool,
    last_map_uri: Option<String>,
    is_live: bool,
    last_media_sequence: i64,
    highest_sequence: i64,
    stuck_count: u32,
    pre_rolled: bool,
    just_resynced: bool,
    master_refresh_counter: u32,
    sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
    receiver: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl HLSHandler {
    pub fn new(master_url: String, options: HLSHandlerOptions) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        let options_headers = options.headers.clone();
        let options_local_address = options.local_address.clone();
        let options_proxy = options.proxy.clone();
        let options_strategy = options.strategy;
        let options_start_time = options.start_time;
        let on_resolve_url: Option<Arc<dyn Fn(String) -> Option<String> + Send + Sync>> =
            options.on_resolve_url.map(|f| Arc::from(f) as Arc<dyn Fn(String) -> Option<String> + Send + Sync>);

        let fetcher = SegmentFetcher::new(SegmentFetcherOptions {
            headers: options_headers.clone(),
            local_address: options_local_address.clone(),
            proxy: options_proxy.clone(),
            on_resolve_url: on_resolve_url.clone(),
        });

        let strategy = options_strategy.unwrap_or(HLSFetchStrategy::Auto);
        let resolved_strategy = match strategy {
            HLSFetchStrategy::Auto => {
                if master_url.contains("fmp4") || master_url.contains(".mp4") {
                    HLSFetchStrategy::Segmented
                } else {
                    HLSFetchStrategy::Streaming
                }
            }
            s => s,
        };

        let max_parallel_fetches = match resolved_strategy {
            HLSFetchStrategy::Segmented => 3,
            HLSFetchStrategy::Streaming => 2,
            HLSFetchStrategy::Auto => 2,
        };

        Self {
            master_url: master_url.clone(),
            current_url: master_url,
            headers: options_headers.unwrap_or_default(),
            local_address: options_local_address,
            proxy: options_proxy,
            on_resolve_url,
            strategy: resolved_strategy,
            start_time: options_start_time.unwrap_or(0.0) / 1000.0,
            fetcher,
            processed_segments: HashSet::new(),
            processed_order: VecDeque::new(),
            segment_queue: VecDeque::new(),
            max_parallel_fetches,
            is_fetching: false,
            stop: false,
            last_map_uri: None,
            is_live: false,
            last_media_sequence: -1,
            highest_sequence: -1,
            stuck_count: 0,
            pre_rolled: false,
            just_resynced: false,
            master_refresh_counter: 0,
            sender: Some(tx),
            receiver: Some(rx),
        }
    }

    pub fn into_stream(self) -> UnboundedReceiverStream<Vec<u8>> {
        UnboundedReceiverStream::new(self.receiver.unwrap())
    }

    pub fn stop(&mut self) {
        self.stop = true;
        self.sender.take();
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            if self.stop {
                return Ok(());
            }
            if let Err(e) = self._playlist_loop().await {
                if !self.stop {
                    error!("HLS playlist loop error: {}", e);
                }
                if self.is_live {
                    warn!("HLS: Playlist error (retrying): {}", e);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                } else {
                    return Err(e);
                }
            }
            if !self.is_live {
                return Ok(());
            }
        }
    }

    async fn _playlist_loop(&mut self) -> Result<()> {
        if self.stop {
            return Ok(());
        }

        let client = self._build_client()?;
        let resp = client.get(&self.current_url).send().await?;
        let status = resp.status();
        let content = resp.text().await?;

        if !status.is_success() {
            if status.as_u16() == 403 || status.as_u16() == 410 {
                if self.current_url != self.master_url {
                    self.current_url = self.master_url.clone();
                    self.just_resynced = true;
                    return Box::pin(self._playlist_loop()).await;
                }
            }
            return Err(anyhow!("Playlist fetch failed: {}", status));
        }

        let mut parsed = parse_playlist(&content, &self.current_url)?;

        if parsed.is_master {
            self._handle_master_playlist(&mut parsed).await?;
            return Ok(());
        }

        self._handle_media_playlist(&mut parsed, &content).await?;
        Ok(())
    }

    fn _build_client(&self) -> Result<Client> {
        let mut builder = Client::builder();
        if let Some(local) = &self.local_address {
            if let Ok(addr) = local.parse::<std::net::IpAddr>() {
                builder = builder.local_address(addr);
            }
        }
        if let Some(proxy) = &self.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        Ok(builder.build()?)
    }

    async fn _handle_master_playlist(&mut self, parsed: &mut HLSMediaPlaylist) -> Result<()> {
        if !parsed.is_master {
            return Ok(());
        }

        let mut sorted_variants = parsed.variants.clone();
        sorted_variants.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));

        let best_variant = sorted_variants.iter().find(|v| {
            (v.codecs.as_ref().map_or(false, |c| c.contains("mp4a") || c.contains("opus")))
                && !v.codecs.as_ref().map_or(false, |c| c.contains("avc1"))
        }).or_else(|| sorted_variants.iter().find(|v| {
            v.codecs.as_ref().map_or(false, |c| c.contains("mp4a") || c.contains("opus"))
        })).or_else(|| sorted_variants.first());

        let Some(variant) = best_variant else {
            return Err(anyhow!("No suitable variant found in master playlist"));
        };

        debug!("HLS: Selected variant bandwidth: {}, codecs: {:?}", variant.bandwidth, variant.codecs);

        if let Some(audio_group_id) = &variant.audio {
            if let Some(group) = parsed.audio_groups.get(audio_group_id) {
                let audio_rendition = group.iter().find(|r| r.default.as_deref() == Some("YES"))
                    .or_else(|| group.iter().find(|r| r.autoselect.as_deref() == Some("YES")))
                    .or_else(|| group.first());

                if let Some(rendition) = audio_rendition {
                    if let Some(uri) = &rendition.uri {
                        self.current_url = uri.clone();
                        return Box::pin(self._playlist_loop()).await;
                    }
                }
            }
        }

        self.current_url = variant.url.clone();
        Box::pin(self._playlist_loop()).await
    }

    async fn _handle_media_playlist(&mut self, parsed: &mut HLSMediaPlaylist, playlist_content: &str) -> Result<()> {
        self.is_live = parsed.is_live;

        debug!(
            "HLS: Processing playlist. Live: {}, Segments: {}, startTime: {}s",
            self.is_live, parsed.segments.len(), self.start_time
        );

        if self.start_time > 0.0 && !self.is_live && self.processed_segments.is_empty() {
            self._handle_start_time(parsed);
        }

        if self.last_media_sequence != -1
            && (parsed.media_sequence < self.last_media_sequence
                || parsed.media_sequence > self.last_media_sequence + MAX_GAP)
        {
            if self.is_live {
                warn!(
                    "HLS: Playlist sequence discontinuity ({} -> {}). Resetting to live edge.",
                    self.last_media_sequence, parsed.media_sequence
                );
                self.segment_queue.clear();
                self.processed_segments.clear();
                self.processed_order.clear();
                self.highest_sequence = -1;
                self.pre_rolled = false;
                self.just_resynced = true;
            }
        }
        self.last_media_sequence = parsed.media_sequence;

        if self.is_live {
            self.master_refresh_counter += 1;
            if self.master_refresh_counter >= MASTER_REFRESH_INTERVAL {
                self.master_refresh_counter = 0;
                self.current_url = self.master_url.clone();
                return Box::pin(self._playlist_loop()).await;
            }
        }

        self._handle_live_pre_roll(parsed);

        let new_segments: Vec<HLSSegment> = parsed.segments.iter()
            .filter(|s| {
                if s.sequence != -1 && s.sequence <= self.highest_sequence {
                    return false;
                }
                let key = if s.sequence != -1 {
                    s.sequence.to_string()
                } else {
                    s.url.clone()
                };
                !self.processed_segments.contains(&key)
            })
            .cloned()
            .collect();

        if !new_segments.is_empty() {
            self.stuck_count = 0;
            for segment in &new_segments {
                if segment.discontinuity && self.is_live {
                    debug!("HLS: Discontinuity detected. Clearing queue and re-syncing.");
                    self.segment_queue.clear();
                    self.processed_segments.clear();
                    self.processed_order.clear();
                    self.highest_sequence = -1;
                    self.pre_rolled = false;
                    self.just_resynced = true;
                    return Box::pin(self._playlist_loop()).await;
                }

                let key = if segment.sequence != -1 {
                    segment.sequence.to_string()
                } else {
                    segment.url.clone()
                };
                self._remember_segment(&key);
                self.segment_queue.push_back(segment.clone());
                if segment.sequence != -1 && segment.sequence > self.highest_sequence {
                    self.highest_sequence = segment.sequence;
                }
            }
        } else if self.is_live {
            self.stuck_count += 1;
            if self.stuck_count >= STUCK_THRESHOLD {
                warn!("HLS: No new segments for {} reloads. Refreshing master playlist.", STUCK_THRESHOLD);
                self.stuck_count = 0;
                self.current_url = self.master_url.clone();
                self.just_resynced = true;
                return Box::pin(self._playlist_loop()).await;
            }
        }

        if !self.segment_queue.is_empty() && !self.is_fetching {
            self._fetch_segments().await?;
        }

        if self.is_live && !playlist_content.contains("#EXT-X-ENDLIST") {
            let delay = (parsed.target_duration / 2.0).max(0.5) * 1000.0;
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
            return Box::pin(self._playlist_loop()).await;
        }

        if !self.is_live && self.segment_queue.is_empty() && !self.is_fetching {
            if let Some(tx) = &self.sender {
                let _ = tx.send(Vec::new());
            }
        }

        Ok(())
    }

    fn _remember_segment(&mut self, key: &str) {
        if self.processed_segments.insert(key.to_string()) {
            self.processed_order.push_back(key.to_string());
            if self.processed_order.len() > MAX_HISTORY {
                if let Some(oldest) = self.processed_order.pop_front() {
                    self.processed_segments.remove(&oldest);
                }
            }
        }
    }

    fn _handle_start_time(&mut self, parsed: &HLSMediaPlaylist) {
        let mut elapsed = 0.0;
        let mut skipped = 0;

        for seg in &parsed.segments {
            if elapsed + seg.duration <= self.start_time {
                elapsed += seg.duration;
                let key = if seg.sequence != -1 {
                    seg.sequence.to_string()
                } else {
                    seg.url.clone()
                };
                self._remember_segment(&key);
                if seg.sequence != -1 && seg.sequence > self.highest_sequence {
                    self.highest_sequence = seg.sequence;
                }
                skipped += 1;
            } else {
                break;
            }
        }

        debug!("HLS: Skipped {} segments. New elapsed: {}s, Target: {}s", skipped, elapsed, self.start_time);
        self.start_time = 0.0;
    }

    fn _handle_live_pre_roll(&mut self, parsed: &HLSMediaPlaylist) {
        let is_first_load = self.processed_segments.is_empty();

        if self.is_live && (is_first_load || self.just_resynced) {
            if self.just_resynced {
                self.processed_segments.clear();
                self.processed_order.clear();
                self.highest_sequence = -1;
            }

            let start_idx = parsed.segments.len().saturating_sub(LIVE_PRE_ROLL_SEGMENTS);
            for seg in parsed.segments.iter().take(start_idx) {
                let key = if seg.sequence != -1 {
                    seg.sequence.to_string()
                } else {
                    seg.url.clone()
                };
                self._remember_segment(&key);
                if seg.sequence != -1 && seg.sequence > self.highest_sequence {
                    self.highest_sequence = seg.sequence;
                }
            }
            self.just_resynced = false;
        } else {
            self.just_resynced = false;
        }
    }

    async fn _fetch_segments(&mut self) -> Result<()> {
        if self.is_fetching || self.stop {
            return Ok(());
        }

        self.is_fetching = true;

        let mut fetch_futures: VecDeque<Pin<Box<dyn Future<Output = Option<SegmentFetchResult>> + Send>>> = VecDeque::new();

        while (!self.segment_queue.is_empty() || !fetch_futures.is_empty()) && !self.stop {
            while fetch_futures.len() < self.max_parallel_fetches && !self.segment_queue.is_empty() {
                let segment = self.segment_queue.pop_front().unwrap();
                let should_stream = self.strategy != HLSFetchStrategy::Segmented;
                let fetcher = self.fetcher.clone();
                fetch_futures.push_back(Box::pin(async move {
                    fetcher.fetch_segment(&segment, should_stream).await.ok()
                }));
            }

            if self.is_live && fetch_futures.is_empty() && self.segment_queue.is_empty() && !self.pre_rolled {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if self.segment_queue.is_empty() && fetch_futures.is_empty() {
                    break;
                }
                continue;
            }

            if fetch_futures.is_empty() {
                break;
            }

            if let Some(handle) = fetch_futures.pop_front() {
                let result = handle.await;
                if let Some(fetch_result) = result {
                    self.pre_rolled = true;

                    if let Some(map) = &fetch_result.segment.map {
                        if map.uri != self.last_map_uri.as_deref().unwrap_or("") {
                            let key_for_map = fetch_result.segment.key.as_ref().filter(|k| k.iv.is_some());
                            if let Ok(Some(map_data)) = self.fetcher.fetch_map(map, key_for_map).await {
                                if let Some(tx) = &self.sender {
                                    if tx.send(map_data).is_err() {
                                        return Ok(());
                                    }
                                }
                                self.last_map_uri = Some(map.uri.clone());
                            }
                        }
                    }

                    if self.strategy == HLSFetchStrategy::Segmented {
                        if let Some(data) = fetch_result.data {
                            if let Some(tx) = &self.sender {
                                if tx.send(data).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    } else if let Some(mut stream) = fetch_result.stream {
                        while let Some(chunk_result) = stream.next().await {
                            if self.stop {
                                break;
                            }
                            match chunk_result {
                                Ok(chunk) => {
                                    if let Some(tx) = &self.sender {
                                        if tx.send(chunk.to_vec()).is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("HLS: Stream error: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        self.is_fetching = false;

        if !self.is_live && self.segment_queue.is_empty() && fetch_futures.is_empty() && !self.stop {
            if let Some(tx) = &self.sender {
                let _ = tx.send(Vec::new());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hls_handler_creation() {
        let handler = HLSHandler::new("https://example.com/master.m3u8".to_string(), HLSHandlerOptions::default());
        assert_eq!(handler.master_url, "https://example.com/master.m3u8");
    }
}