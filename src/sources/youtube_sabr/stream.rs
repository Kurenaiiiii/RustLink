use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use reqwest::Client as HttpClient;
use tracing::{debug, warn, error, info};

use super::protor::*;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const MAX_BUFFER_BYTES: usize = 512 * 1024;
const MIN_REQUEST_INTERVAL_MS: u64 = 500;

#[derive(Debug)]
pub struct CompositeBuffer {
    pub chunks: Vec<Vec<u8>>,
    current_chunk_offset: usize,
    current_chunk_index: usize,
    pub total_length: usize,
}

impl CompositeBuffer {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            current_chunk_offset: 0,
            current_chunk_index: 0,
            total_length: 0,
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.chunks.push(data.to_vec());
            self.total_length += data.len();
        }
    }

    pub fn append_buffer(&mut self, other: &CompositeBuffer) {
        for chunk in &other.chunks {
            self.append(chunk);
        }
    }

    pub fn split(&self, position: usize) -> (CompositeBuffer, CompositeBuffer) {
        let mut extracted = CompositeBuffer::new();
        let mut remaining = CompositeBuffer::new();
        let mut remaining_pos = position;

        for chunk in &self.chunks {
            if remaining_pos >= chunk.len() {
                extracted.append(chunk);
                remaining_pos -= chunk.len();
            } else if remaining_pos > 0 {
                extracted.append(&chunk[..remaining_pos]);
                remaining.append(&chunk[remaining_pos..]);
                remaining_pos = 0;
            } else {
                remaining.append(chunk);
            }
        }
        (extracted, remaining)
    }

    pub fn can_read_bytes(&self, position: usize, length: usize) -> bool {
        position + length <= self.total_length
    }

    pub fn get_uint8(&self, position: usize) -> u8 {
        let (ci, co) = self.focus(position);
        self.chunks.get(ci).and_then(|c| c.get(position - co)).copied().unwrap_or(0)
    }

    pub fn get_length(&self) -> usize {
        self.total_length
    }

    pub fn is_empty(&self) -> bool {
        self.total_length == 0
    }

    fn focus(&self, position: usize) -> (usize, usize) {
        if self.chunks.is_empty() {
            return (0, 0);
        }
        let mut offset = 0usize;
        for (i, chunk) in self.chunks.iter().enumerate() {
            if position < offset + chunk.len() {
                return (i, offset);
            }
            offset += chunk.len();
        }
        (self.chunks.len().saturating_sub(1), offset.saturating_sub(self.chunks.last().map(|c| c.len()).unwrap_or(0)))
    }
}

#[derive(Debug)]
pub struct UmpPart {
    pub part_type: u32,
    pub size: usize,
    pub data: CompositeBuffer,
}

#[derive(Debug)]
pub struct IncompleteUmpPart {
    pub part_type: u32,
    pub size: usize,
    pub header_size: usize,
    pub data: CompositeBuffer,
    pub incomplete: bool,
}

pub struct UmpReader {
    pub composite_buffer: CompositeBuffer,
}

impl UmpReader {
    pub fn new(buffer: CompositeBuffer) -> Self {
        Self { composite_buffer: buffer }
    }

    fn read_varint(&self, offset: usize) -> (i32, usize) {
        let byte_length = if self.composite_buffer.can_read_bytes(offset, 1) {
            let first_byte = self.composite_buffer.get_uint8(offset);
            if first_byte < 128 { 1 }
            else if first_byte < 192 { 2 }
            else if first_byte < 224 { 3 }
            else if first_byte < 240 { 4 }
            else { 5 }
        } else { 0 };

        if byte_length < 1 || !self.composite_buffer.can_read_bytes(offset, byte_length) {
            return (-1, offset);
        }

        let mut off = offset;
        let value = match byte_length {
            1 => self.composite_buffer.get_uint8(off) as i32,
            2 => {
                let b1 = self.composite_buffer.get_uint8(off); off += 1;
                let b2 = self.composite_buffer.get_uint8(off); off += 1;
                (b1 as i32 & 0x3f) + 64 * b2 as i32
            }
            3 => {
                let b1 = self.composite_buffer.get_uint8(off); off += 1;
                let b2 = self.composite_buffer.get_uint8(off); off += 1;
                let b3 = self.composite_buffer.get_uint8(off); off += 1;
                (b1 as i32 & 0x1f) + 32 * (b2 as i32 + 256 * b3 as i32)
            }
            4 => {
                let b1 = self.composite_buffer.get_uint8(off); off += 1;
                let b2 = self.composite_buffer.get_uint8(off); off += 1;
                let b3 = self.composite_buffer.get_uint8(off); off += 1;
                let b4 = self.composite_buffer.get_uint8(off); off += 1;
                (b1 as i32 & 0x0f) + 16 * (b2 as i32 + 256 * (b3 as i32 + 256 * b4 as i32))
            }
            _ => {
                let _ = self.composite_buffer.get_uint8(off); off += 1;
                let b1 = self.composite_buffer.get_uint8(off); off += 1;
                let b2 = self.composite_buffer.get_uint8(off); off += 1;
                let b3 = self.composite_buffer.get_uint8(off); off += 1;
                let b4 = self.composite_buffer.get_uint8(off); off += 1;
                b1 as i32 + 256 * (b2 as i32 + 256 * (b3 as i32 + 256 * b4 as i32))
            }
        };
        (value, off)
    }

    pub fn read<F>(&mut self, handle_part: &mut F) -> Option<IncompleteUmpPart>
    where
        F: FnMut(UmpPart),
    {
        loop {
            let offset = 0usize;
            let (part_type, next_offset) = self.read_varint(offset);
            if part_type < 0 {
                break;
            }
            let (part_size, final_offset) = self.read_varint(next_offset);
            if part_size < 0 {
                break;
            }

            if !self.composite_buffer.can_read_bytes(final_offset, part_size as usize) {
                let (_extracted, remaining) = self.composite_buffer.split(final_offset);
                return Some(IncompleteUmpPart {
                    part_type: part_type as u32,
                    size: part_size as usize,
                    header_size: final_offset,
                    data: remaining,
                    incomplete: true,
                });
            }

            let cb = std::mem::replace(&mut self.composite_buffer, CompositeBuffer::new());
            let (_, remaining) = cb.split(final_offset);
            let (payload, next_remaining) = remaining.split(part_size as usize);

            handle_part(UmpPart {
                part_type: part_type as u32,
                size: part_size as usize,
                data: payload,
            });
            self.composite_buffer = next_remaining;
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct FormatEntry {
    pub itag: i32,
    pub mime_type: Option<String>,
    pub xtags: Option<String>,
    pub last_modified: Option<String>,
    pub audio_track_id: Option<String>,
    pub bitrate: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct AbrState {
    pub player_time_ms: i64,
    pub bandwidth_estimate: i64,
    pub enabled_track_types_bitfield: i32,
    pub audio_track_id: String,
    pub player_state: i64,
    pub visibility: i32,
    pub playback_rate: f32,
    pub sticky_resolution: i32,
    pub last_manual_selected_resolution: i32,
    pub client_viewport_is_flexible: bool,
}

#[derive(Debug, Clone)]
pub struct PartialSegmentQueueEntry {
    pub format_id_key: String,
    pub segment_number: i32,
    pub media_header: MediaHeaderMsg,
    pub duration_ms: String,
    pub loaded_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DownloadedSegment {
    pub segment_number: i32,
    pub duration_ms: i64,
    pub byte_length: usize,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone)]
pub struct FormatInitializationEntry {
    pub format_initialization_metadata: FormatInitializationMetadataMsg,
}

#[derive(Debug, Clone, Default)]
pub struct PreviousSessionState {
    pub request_number: u64,
    pub bandwidth_estimate: i64,
    pub next_request_policy: Option<NextRequestPolicyMsg>,
}

#[derive(Debug, Clone)]
pub struct SabrStreamConfig {
    pub video_id: String,
    pub server_abr_streaming_url: Option<String>,
    pub video_playback_ustreamer_config: Option<Vec<u8>>,
    pub client_info: Option<ClientInfoMsg>,
    pub formats: Vec<FormatEntry>,
    pub po_token: Option<Vec<u8>>,
    pub visitor_data: Option<String>,
    pub start_time: i64,
    pub user_agent: Option<String>,
    pub previous_session: Option<PreviousSessionState>,
    pub access_token: Option<String>,
}

pub type PartHandler = Box<dyn FnMut(UmpPart) + Send>;

pub struct SabrStream {
    pub video_id: String,
    config: SabrStreamConfig,
    http_client: HttpClient,

    // UMP part handlers
    ump_part_handlers: HashMap<u32, PartHandler>,

    // Format state
    initialized_formats_map: HashMap<String, FormatInitializationEntry>,
    partial_segment_queue: HashMap<i32, PartialSegmentQueueEntry>,
    format_sequence_counters: HashMap<i32, i32>,
    downloaded_segments_by_itag: HashMap<i32, HashMap<i32, DownloadedSegment>>,
    end_segment_numbers: HashMap<i32, i32>,

    // SABR context state
    sabr_contexts: HashMap<i32, SabrContextValue>,
    active_sabr_context_types: HashSet<i32>,

    // Request state
    pub request_number: u64,
    media_headers_processed: bool,
    aborted: bool,
    pub stream_finished: bool,

    // PO token
    po_token: Option<Vec<u8>>,
    visitor_data: Option<String>,

    // Server config
    pub server_abr_streaming_url: Option<String>,
    video_playback_ustreamer_config: Option<Vec<u8>>,
    client_info: Option<ClientInfoMsg>,
    format_ids: Vec<FormatEntry>,

    // Timeline
    pub start_time: i64,
    pub total_downloaded_ms: i64,
    virtual_player_time_ms: i64,
    last_virtual_advance_at: i64,
    pub cumulative_downloaded_ms: i64,

    // Buffered range tracking
    pending_ranges_headers: HashMap<String, Vec<MediaHeaderMsg>>,
    cached_buffered_ranges: Option<Vec<BufferedRangeSummary>>,
    last_reported_ranges: HashSet<String>,

    // Bandwidth estimation
    pub bandwidth_estimate: i64,
    last_bandwidth_log_at: i64,
    no_media_streak: i32,

    // Request timing
    last_request_at: i64,
    next_request_policy: Option<NextRequestPolicyMsg>,
    last_stream_protection_status: Option<i32>,
    last_stream_protection_log_at: i64,
    last_detailed_log_at: i64,
    _last_policy_backoff: Option<i32>,
    _last_policy_cookie_len: Option<usize>,
    _last_policy_log_at: i64,

    // Recovery
    recovery_pending: bool,
    stall_emitted: bool,

    // Audio output channel
    audio_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub audio_rx: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
}

#[derive(Debug, Clone)]
pub struct SabrContextValue {
    pub ctx_type: i32,
    pub value: Vec<u8>,
    pub send_by_default: bool,
}

#[derive(Debug, Clone, Default)]
pub struct BufferedRangeSummary {
    pub format_id: Option<FormatIdMsg>,
    pub start_time_ms: String,
    pub duration_ms: String,
    pub start_segment_index: i32,
    pub end_segment_index: i32,
    pub time_range: Option<TimeRangeMsg>,
}

pub struct SawFlags {
    pub media: bool,
    pub media_header: bool,
    pub media_end: bool,
    pub next_request_policy: bool,
    pub playback_start_policy: bool,
    pub request_identifier: bool,
    pub request_cancellation_policy: bool,
    pub sabr_error: bool,
    pub sabr_redirect: bool,
    pub sabr_context_update: bool,
    pub stream_protection_status: bool,
}

impl SabrStream {
    pub fn new(http_client: HttpClient, config: SabrStreamConfig) -> Self {
        let mut url = config.server_abr_streaming_url.clone();
        if let Some(ref u) = url {
            if let Ok(mut parsed) = url::Url::parse(u) {
                parsed.query_pairs_mut()
                    .append_pair("alr", "yes")
                    .append_pair("ump", "1")
                    .append_pair("srfvp", "1");
                url = Some(parsed.to_string());
            }
        }

        let po_token = config.po_token.clone();
        let visitor_data = config.visitor_data.clone();
        let ustreamer_config = config.video_playback_ustreamer_config.clone();
        let ci = config.client_info.clone();
        let fmt_ids = config.formats.clone();
        let previous_session = config.previous_session.clone();

        let (audio_tx, audio_rx) = mpsc::unbounded_channel();

        let mut stream = SabrStream {
            video_id: config.video_id.clone(),
            config,
            http_client,
            ump_part_handlers: HashMap::new(),
            initialized_formats_map: HashMap::new(),
            partial_segment_queue: HashMap::new(),
            format_sequence_counters: HashMap::new(),
            downloaded_segments_by_itag: HashMap::new(),
            end_segment_numbers: HashMap::new(),
            sabr_contexts: HashMap::new(),
            active_sabr_context_types: HashSet::new(),
            request_number: 0,
            media_headers_processed: false,
            aborted: false,
            stream_finished: false,
            po_token,
            visitor_data,
            server_abr_streaming_url: url,
            video_playback_ustreamer_config: ustreamer_config,
            client_info: ci,
            format_ids: fmt_ids,
            start_time: 0,
            total_downloaded_ms: 0,
            virtual_player_time_ms: 0,
            last_virtual_advance_at: 0,
            cumulative_downloaded_ms: 0,
            pending_ranges_headers: HashMap::new(),
            cached_buffered_ranges: None,
            last_reported_ranges: HashSet::new(),
            bandwidth_estimate: 5_000_000,
            last_bandwidth_log_at: 0,
            no_media_streak: 0,
            last_request_at: 0,
            next_request_policy: None,
            last_stream_protection_status: None,
            last_stream_protection_log_at: 0,
            last_detailed_log_at: 0,
            _last_policy_backoff: None,
            _last_policy_cookie_len: None,
            _last_policy_log_at: 0,
            recovery_pending: false,
            stall_emitted: false,
            audio_tx,
            audio_rx: Some(audio_rx),
        };

        stream.start_time = stream.config.start_time;
        stream.cumulative_downloaded_ms = stream.start_time;

        if let Some(ps) = previous_session {
            stream.request_number = ps.request_number;
            stream.bandwidth_estimate = ps.bandwidth_estimate;
            stream.next_request_policy = ps.next_request_policy;
            info!(
                "SABR Session state transferred: rn={}, bw={:.2}Mbps, hasCookie={}",
                stream.request_number,
                stream.bandwidth_estimate as f64 / 1_000_000.0,
                stream.next_request_policy.as_ref().and_then(|p| p.playback_cookie.as_ref()).is_some()
            );
        }

        stream.register_handlers();
        stream
    }

    fn register_handlers(&mut self) {
        // We'll handle parts manually in the read loop
    }

    pub fn get_session_state(&self) -> PreviousSessionState {
        PreviousSessionState {
            request_number: self.request_number,
            bandwidth_estimate: self.bandwidth_estimate,
            next_request_policy: self.next_request_policy.clone(),
        }
    }

    fn update_bandwidth_estimate(&mut self, bytes: usize, duration_ms: u64) {
        if bytes == 0 || duration_ms == 0 {
            return;
        }
        let bits = (bytes as f64) * 8.0;
        let throughput = (bits / duration_ms as f64) * 1000.0;
        let alpha = 0.15;
        self.bandwidth_estimate = (alpha * throughput + (1.0 - alpha) * self.bandwidth_estimate as f64) as i64;
    }

    fn create_key(itag: i32, xtags: Option<&str>) -> String {
        format!("{}:{}", itag, xtags.unwrap_or(""))
    }

    fn format_key_from_init_meta(meta: &FormatInitializationMetadataMsg) -> String {
        let itag = meta.format_id.as_ref().map(|f| f.itag).or(meta.itag).unwrap_or(0);
        let xtags = meta.format_id.as_ref().and_then(|f| f.xtags.as_deref());
        Self::create_key(itag, xtags)
    }

    fn format_key_from_media_header(h: &MediaHeaderMsg) -> String {
        let itag = h.format_id.as_ref().map(|f| f.itag).unwrap_or(h.itag);
        let xtags = h.format_id.as_ref().and_then(|f| f.xtags.as_deref()).or(h.xtags.as_deref());
        Self::create_key(itag, xtags)
    }

    fn resolve_format_id_for_request(&self, format: &FormatEntry) -> FormatIdMsg {
        if format.xtags.is_some() {
            return FormatIdMsg {
                itag: format.itag,
                last_modified: format.last_modified.clone(),
                xtags: format.xtags.clone(),
            };
        }
        let prefix = format!("{}:", format.itag);
        for (k, v) in &self.initialized_formats_map {
            if !k.starts_with(&prefix) { continue; }
            if let Some(ref fid) = v.format_initialization_metadata.format_id {
                if fid.itag != 0 {
                    return FormatIdMsg {
                        itag: fid.itag,
                        last_modified: fid.last_modified.clone().or_else(|| format.last_modified.clone()),
                        xtags: fid.xtags.clone(),
                    };
                }
            }
        }
        FormatIdMsg {
            itag: format.itag,
            last_modified: format.last_modified.clone(),
            xtags: format.xtags.clone(),
        }
    }

    fn decode_part<T>(part: &UmpPart, decoder: fn(&mut ProtoReader, usize) -> T) -> Option<T> {
        let data = if part.data.chunks.len() == 1 {
            part.data.chunks[0].clone()
        } else {
            concatenate_chunks(&part.data.chunks)
        };
        let mut reader = ProtoReader::new(&data);
        Some(decoder(&mut reader, data.len()))
    }

    fn handle_format_initialization_metadata(&mut self, part: UmpPart) {
        let m = match Self::decode_part(&part, decode_format_initialization_metadata) {
            Some(m) => m,
            None => return,
        };
        let key = Self::format_key_from_init_meta(&m);
        if !self.initialized_formats_map.contains_key(&key) {
            let itag = m.format_id.as_ref().map(|f| f.itag).or(m.itag);
            if let Some(ens) = &m.end_segment_number {
                if let Ok(n) = ens.parse::<i32>() {
                    if n > 0 {
                        self.end_segment_numbers.insert(itag.unwrap_or(0), n);
                        debug!("SABR Tracking completion: itag={:?} will finish at segment {}", itag, n);
                    }
                }
            }
            let mime = m.mime_type.clone();
            let end_seg = m.end_segment_number.clone();
            self.initialized_formats_map.insert(key.clone(), FormatInitializationEntry {
                format_initialization_metadata: m,
            });
            debug!("SABR Format init: key={} mime={:?} endSeg={:?}", key, mime, end_seg);
        }
    }

    fn handle_sabr_error(&mut self, part: UmpPart) -> Result<(), String> {
        let err = match Self::decode_part(&part, decode_sabr_error) {
            Some(e) => e,
            None => return Ok(()),
        };
        Err(format!("SABR Error: {:?} {:?}", err.code, err.err_type))
    }

    fn handle_sabr_redirect(&mut self, part: UmpPart) {
        let red = match Self::decode_part(&part, decode_sabr_redirect) {
            Some(r) => r,
            None => return,
        };
        if let Some(url) = red.url {
            self.server_abr_streaming_url = Some(url);
        }
    }

    fn handle_stream_protection_status(&mut self, part: UmpPart) {
        let status = match Self::decode_part(&part, decode_stream_protection_status) {
            Some(s) => s,
            None => return,
        };
        let now = chrono_now_ms();
        let changed = self.last_stream_protection_status != status.status;
        let should_log = changed || self.last_stream_protection_log_at == 0 || now - self.last_stream_protection_log_at > 5000;
        self.last_stream_protection_status = status.status;
        if !should_log { return; }
        self.last_stream_protection_log_at = now;

        if status.status == Some(3) {
            debug!("SABR Stream Protection Status: 3 (Attestation pending/required)");
            return;
        }
        if status.status == Some(2) {
            if self.stall_emitted { return; }
            self.stall_emitted = true;
            warn!("SABR Stream Protection Status: 2 (Limited Playback). Triggering token refresh...");
            self.recovery_pending = true;
            return;
        }
        warn!("SABR Stream Protection Status: {:?}", status.status);
    }

    fn handle_media_partial(&mut self, buffer: &CompositeBuffer, header_id: i32, is_first_chunk: bool) {
        if let Some(s) = self.partial_segment_queue.get_mut(&header_id) {
            let data_to_process = if is_first_chunk && buffer.get_length() > 1 {
                let (_, remaining) = buffer.split(1);
                remaining
            } else if is_first_chunk && buffer.get_length() == 1 {
                return;
            } else {
                let mut cb = CompositeBuffer::new();
                cb.append_buffer(buffer);
                cb
            };

            let bytes = data_to_process.total_length;
            s.loaded_bytes += bytes;

            // Push audio chunks to output channel
            for chunk in &data_to_process.chunks {
                let _ = self.audio_tx.send(chunk.clone());
            }
        }
    }

    fn handle_media(&mut self, part: UmpPart) {
        let header_id = part.data.get_uint8(0) as i32;
        if let Some(s) = self.partial_segment_queue.get_mut(&header_id) {
            let (_, remaining) = part.data.split(1);
            let bytes = remaining.total_length;
            s.loaded_bytes += bytes;

            // Push audio chunks to output channel
            for chunk in &remaining.chunks {
                let _ = self.audio_tx.send(chunk.clone());
            }

            if bytes > 0 {
                debug!("SABR Media data: id={} bytes={} total={}/{:?}", header_id, bytes, s.loaded_bytes, s.media_header.content_length);
            }
        } else {
            debug!("SABR Media data for unknown headerId: {}", header_id);
        }
    }

    fn handle_media_header(&mut self, part: UmpPart) {
        let h = match Self::decode_part(&part, decode_media_header) {
            Some(h) => h,
            None => {
                warn!("SABR Failed to decode MediaHeader");
                return;
            }
        };
        let key = Self::format_key_from_media_header(&h);
        let header_id = h.header_id.unwrap_or(0);

        let mut segment_number = h.sequence_number;
        if h.is_init_seg {
            segment_number = 0;
        } else if segment_number == 0 {
            let count = self.format_sequence_counters.get(&h.itag).copied().unwrap_or(0) + 1;
            self.format_sequence_counters.insert(h.itag, count);
            segment_number = count;
        } else {
            self.format_sequence_counters.insert(h.itag, segment_number);
        }

        let mut h = h;
        if h.duration_ms == "0" || h.duration_ms.is_empty() {
            if let Some(ref tr) = h.time_range {
                if tr.timescale > 0 {
                    let dur_ticks = tr.duration_ticks.parse::<f64>().unwrap_or(0.0);
                    h.duration_ms = ((dur_ticks / tr.timescale as f64) * 1000.0).ceil().to_string();
                }
            }
        }

        let format_id_key = key;
        self.pending_ranges_headers.entry(format_id_key.clone())
            .or_default()
            .push(h.clone());

        let dur_ms = h.duration_ms.clone();
        debug!("SABR MediaHeader: id={} itag={} seq={} dur={}ms", header_id, h.itag, segment_number, dur_ms);

        self.partial_segment_queue.insert(header_id, PartialSegmentQueueEntry {
            format_id_key,
            segment_number,
            media_header: h,
            duration_ms: dur_ms,
            loaded_bytes: 0,
        });
    }

    fn handle_media_end(&mut self, part: UmpPart) {
        let id = part.data.get_uint8(0) as i32;
        if let Some(s) = self.partial_segment_queue.remove(&id) {
            debug!("SABR MediaEnd: id={} seq={} totalBytes={}", id, s.segment_number, s.loaded_bytes);

            let itag = s.media_header.format_id.as_ref().map(|f| f.itag).unwrap_or(s.media_header.itag);
            let mut segment_duration = 0i64;
            if let Ok(d) = s.duration_ms.parse::<i64>() {
                segment_duration = d;
            } else if let Some(ref tr) = s.media_header.time_range {
                if tr.timescale > 0 {
                    let dur_ticks = tr.duration_ticks.parse::<f64>().unwrap_or(0.0);
                    segment_duration = ((dur_ticks / tr.timescale as f64) * 1000.0).ceil() as i64;
                }
            }

            if segment_duration > 0 {
                self.total_downloaded_ms += segment_duration;
                self.media_headers_processed = true;
                debug!("SABR Segment received: itag={} seq={} dur={}ms totalDownloaded={}ms", itag, s.segment_number, segment_duration, self.total_downloaded_ms);
            }

            if itag > 0 {
                let seg_map = self.downloaded_segments_by_itag.entry(itag).or_default();
                if seg_map.contains_key(&s.segment_number) {
                    warn!("SABR Ignoring duplicate segment {} for itag {}", s.segment_number, itag);
                } else {
                    let start_ms_str = &s.media_header.start_ms;
                    let mut start_ms = start_ms_str.parse::<i64>().unwrap_or(0);
                    if start_ms == 0 {
                        if let Some(ref tr) = s.media_header.time_range {
                            if tr.timescale > 0 {
                                let ticks = tr.start_ticks.parse::<i64>().unwrap_or(0);
                                start_ms = (ticks * 1000) / tr.timescale as i64;
                            }
                        }
                    }
                    let end_ms = start_ms + segment_duration;
                    seg_map.insert(s.segment_number, DownloadedSegment {
                        segment_number: s.segment_number,
                        duration_ms: segment_duration,
                        byte_length: s.loaded_bytes,
                        start_ms,
                        end_ms,
                    });
                    if end_ms > self.cumulative_downloaded_ms {
                        self.cumulative_downloaded_ms = end_ms;
                    }
                    if let Some(&end_seg) = self.end_segment_numbers.get(&itag) {
                        if s.segment_number >= end_seg && !self.stream_finished {
                            self.stream_finished = true;
                            info!("SABR Stream complete: received final segment {}/{} for itag {}", s.segment_number, end_seg, itag);
                        }
                    }
                }
            }
        }
    }

    fn handle_next_request_policy(&mut self, part: UmpPart) {
        let policy = match Self::decode_part(&part, decode_next_request_policy) {
            Some(p) => p,
            None => return,
        };
        self.next_request_policy = Some(policy.clone());
        let cookie_len = policy.playback_cookie.as_ref().map(|c| c.len()).unwrap_or(0);
        let backoff = policy.backoff_time_ms.unwrap_or(0);
        let now = chrono_now_ms();
        let changed = self._last_policy_backoff != Some(backoff) || self._last_policy_cookie_len != Some(cookie_len);
        let should_log = changed || self._last_policy_log_at == 0 || now - self._last_policy_log_at > 2000;
        self._last_policy_backoff = Some(backoff);
        self._last_policy_cookie_len = Some(cookie_len);
        if !should_log { return; }
        self._last_policy_log_at = now;
        debug!("SABR NextRequestPolicy: backoff={}ms cookieLen={}", backoff, cookie_len);
    }

    fn handle_sabr_context_update(&mut self, part: UmpPart) {
        let ctx = match Self::decode_part(&part, decode_sabr_context_update) {
            Some(c) => c,
            None => return,
        };
        if let (Some(typ), Some(ref value)) = (ctx.ctx_type, ctx.value) {
            if !value.is_empty() {
                self.sabr_contexts.insert(typ, SabrContextValue {
                    ctx_type: typ,
                    value: value.clone(),
                    send_by_default: ctx.send_by_default.unwrap_or(false),
                });
                if ctx.send_by_default.unwrap_or(false) {
                    self.active_sabr_context_types.insert(typ);
                }
                debug!("SABR Received context update type={} len={} sendByDefault={:?}", typ, value.len(), ctx.send_by_default);
            }
        }
    }

    fn handle_sabr_context_sending_policy(&mut self, part: UmpPart) {
        let policy = match Self::decode_part(&part, decode_sabr_context_sending_policy) {
            Some(p) => p,
            None => return,
        };
        for typ in policy.start_policy { self.active_sabr_context_types.insert(typ); }
        for typ in policy.stop_policy { self.active_sabr_context_types.remove(&typ); }
        for typ in policy.discard_policy { self.sabr_contexts.remove(&typ); }
    }

    fn handle_snackbar_message(&mut self, _part: UmpPart) {}

    fn handle_reload_player_response(&mut self, part: UmpPart) {
        let data = if part.data.chunks.len() == 1 {
            part.data.chunks[0].clone()
        } else {
            concatenate_chunks(&part.data.chunks)
        };
        let mut reader = ProtoReader::new(&data);
        let decoded = decode_generic_message(&mut reader, data.len());
        let reason = decoded.get("1").and_then(|v| v.get("utf8")).and_then(|v| v.as_str()).unwrap_or("unknown");
        warn!("SABR Reload requested by server. Reason: {}", reason);
        self.recovery_pending = true;
    }

    fn build_buffered_ranges(&mut self) -> Vec<BufferedRangeSummary> {
        let mut ranges = Vec::new();
        let format_ids: Vec<(String, FormatIdMsg)> = self.format_ids.iter()
            .map(|f| {
                let key = Self::create_key(f.itag, f.xtags.as_deref());
                let fid = self.resolve_format_id_for_request(f);
                (key, fid)
            })
            .collect();
        for (format_id_key, format_id) in format_ids {
            let headers = self.pending_ranges_headers.entry(format_id_key).or_default();
            if headers.is_empty() { continue; }

            let duration_ms: i64 = headers.iter()
                .filter_map(|h| h.duration_ms.parse::<i64>().ok())
                .sum();
            let start_h = headers[0].clone();
            let end_h = headers[headers.len() - 1].clone();
            let timescale = start_h.time_range.as_ref().map(|tr| tr.timescale).unwrap_or(1000);
            let dur_ticks = if duration_ms > 0 {
                ((duration_ms as u128 * timescale as u128) / 1000).to_string()
            } else {
                "0".to_string()
            };

            ranges.push(BufferedRangeSummary {
                format_id: Some(format_id),
                start_time_ms: start_h.start_ms.clone(),
                duration_ms: duration_ms.to_string(),
                start_segment_index: start_h.sequence_number,
                end_segment_index: end_h.sequence_number,
                time_range: Some(TimeRangeMsg {
                    start_ticks: start_h.start_ms.clone(),
                    duration_ticks: dur_ticks,
                    timescale,
                }),
            });
            *headers = Vec::new();
        }
        ranges
    }

    pub fn seek_to(&mut self, position_ms: i64) -> bool {
        if self.aborted {
            warn!("SABR Cannot seek: stream is aborted");
            return false;
        }
        info!("SABR Seeking to {}ms (from startTime={}ms)", position_ms, self.start_time);
        self.start_time = position_ms;
        self.downloaded_segments_by_itag.clear();
        self.format_sequence_counters.clear();
        self.partial_segment_queue.clear();
        self.initialized_formats_map.clear();
        self.total_downloaded_ms = 0;
        self.virtual_player_time_ms = 0;
        self.cumulative_downloaded_ms = position_ms;
        self.last_virtual_advance_at = chrono_now_ms();
        self.pending_ranges_headers.clear();
        self.cached_buffered_ranges = None;
        self.last_reported_ranges.clear();
        self.media_headers_processed = false;
        self.stream_finished = false;
        self.no_media_streak = 0;
        debug!("SABR Seek to {}ms complete. Session preserved (rn={})", position_ms, self.request_number);
        true
    }

    pub fn clear_buffers(&mut self) {
        self.initialized_formats_map.clear();
        self.downloaded_segments_by_itag.clear();
        self.format_sequence_counters.clear();
        self.partial_segment_queue.clear();
        self.media_headers_processed = false;
        self.pending_ranges_headers.clear();
        self.cached_buffered_ranges = None;
        self.last_reported_ranges.clear();
        self.sabr_contexts.clear();
        self.active_sabr_context_types.clear();
        info!("SABR Buffers cleared for recovery. Preserving timeline position: {}ms, totalDownloaded: {}ms", self.cumulative_downloaded_ms, self.total_downloaded_ms);
    }

    pub fn update_session(&mut self, new_config: SabrStreamConfig) {
        if let Some(ref url) = new_config.server_abr_streaming_url {
            if let Ok(mut parsed) = url::Url::parse(url) {
                parsed.query_pairs_mut()
                    .append_pair("alr", "yes")
                    .append_pair("ump", "1")
                    .append_pair("srfvp", "1");
                self.server_abr_streaming_url = Some(parsed.to_string());
            }
        }
        if let Some(ref uc) = new_config.video_playback_ustreamer_config {
            self.video_playback_ustreamer_config = Some(uc.clone());
        }
        if let Some(ref pt) = new_config.po_token {
            self.po_token = Some(pt.clone());
        }
        if let Some(ref vd) = new_config.visitor_data {
            self.visitor_data = Some(vd.clone());
        }
        if let Some(ref ci) = new_config.client_info {
            self.client_info = Some(ci.clone());
        }
        if !new_config.formats.is_empty() {
            self.format_ids = new_config.formats;
        }
        if let Some(ref ua) = new_config.user_agent {
            // user_agent handled in HTTP client config
            let _ = ua;
        }

        self.request_number = 0;
        self.no_media_streak = 0;
        self.pending_ranges_headers.clear();
        self.recovery_pending = false;
        self.stall_emitted = false;

        info!("SABR Session updated. Continuing with RN={}", self.request_number);
    }

    pub fn abort(&mut self) {
        self.aborted = true;
    }

    async fn fetch_and_process_segments(
        &mut self,
        state: &AbrState,
        audio_format: &FormatEntry,
    ) -> Result<(), String> {
        if self.video_playback_ustreamer_config.is_none() || self.client_info.is_none() {
            return Err("Missing config".to_string());
        }

        // Handle backoff
        if let Some(ref policy) = self.next_request_policy {
            if let Some(backoff) = policy.backoff_time_ms {
                if backoff > 0 {
                    warn!("SABR Waiting for backoff: {}ms", backoff);
                    sleep(Duration::from_millis(backoff as u64)).await;
                    // Can't mutate through shared ref, need separate handling
                }
            }
        }

        let formats_initialized = !self.initialized_formats_map.is_empty();
        let request_format_ids: Vec<FormatIdMsg> = if formats_initialized {
            self.format_ids.iter()
                .map(|f| self.resolve_format_id_for_request(f))
                .collect()
        } else {
            Vec::new()
        };

        if self.cached_buffered_ranges.is_none() {
            self.cached_buffered_ranges = Some(self.build_buffered_ranges());
        }

        let mut contexts: Vec<(i32, Vec<u8>)> = Vec::new();
        let mut unsent: Vec<i32> = Vec::new();
        for (_key, ctx) in &self.sabr_contexts {
            if self.active_sabr_context_types.contains(&ctx.ctx_type) {
                contexts.push((ctx.ctx_type, ctx.value.clone()));
            } else {
                unsent.push(ctx.ctx_type);
            }
        }

        let pref_audio = vec![self.resolve_format_id_for_request(audio_format)];
        let buffered_ranges = self.cached_buffered_ranges.as_ref().map(|v| v.clone()).unwrap_or_default();

        let abr_state_msg = ClientAbrStateMsg {
            last_manual_selected_resolution: Some(state.last_manual_selected_resolution),
            sticky_resolution: Some(state.sticky_resolution),
            client_viewport_is_flexible: Some(state.client_viewport_is_flexible),
            bandwidth_estimate: Some(state.bandwidth_estimate),
            player_time_ms: Some(state.player_time_ms),
            visibility: Some(state.visibility),
            playback_rate: Some(state.playback_rate),
            time_since_last_action_ms: Some(0),
            enabled_track_types_bitfield: Some(state.enabled_track_types_bitfield),
            player_state: Some(state.player_state),
            drc_enabled: None,
            audio_track_id: if state.audio_track_id.is_empty() { None } else { Some(state.audio_track_id.clone()) },
        };

        let sabr_context_entries: Vec<SabrContextEntry> = contexts.into_iter()
            .map(|(t, v)| SabrContextEntry { ctx_type: t, value: v })
            .collect();

        let streamer_ctx = StreamerContextMsg {
            client_info: self.client_info.clone(),
            po_token: self.po_token.clone(),
            playback_cookie: self.next_request_policy.as_ref().and_then(|p| p.playback_cookie.clone()),
            sabr_contexts: sabr_context_entries,
            unsent_sabr_contexts: unsent,
        };

        let br_msgs: Vec<BufferedRangeMsg> = buffered_ranges.iter().map(|br| {
            let mut st = "0".to_string();
            let mut dur = "0".to_string();
            if !br.start_time_ms.is_empty() { st = br.start_time_ms.clone(); }
            if !br.duration_ms.is_empty() { dur = br.duration_ms.clone(); }
            BufferedRangeMsg {
                format_id: br.format_id.clone(),
                start_time_ms: Some(st),
                duration_ms: Some(dur),
                start_segment_index: Some(br.start_segment_index),
                end_segment_index: Some(br.end_segment_index),
                time_range: br.time_range.clone(),
            }
        }).collect();

        let request_body = encode_video_playback_abr_request(
            Some(&abr_state_msg),
            &request_format_ids,
            &br_msgs,
            self.video_playback_ustreamer_config.as_deref(),
            &pref_audio,
            &[],
            Some(&streamer_ctx),
        );

        let rn = self.request_number;
        self.request_number += 1;

        let url = match self.server_abr_streaming_url.as_ref() {
            Some(u) => {
                let mut parsed = url::Url::parse(u).map_err(|e| format!("URL parse error: {e}"))?;
                parsed.query_pairs_mut().append_pair("rn", &rn.to_string());
                parsed.to_string()
            }
            None => return Err("No streaming URL".to_string()),
        };

        let visitor_data = self.visitor_data.as_deref().unwrap_or("");
        let client_name = self.client_info.as_ref().map(|c| c.client_name).unwrap_or(1);
        let client_version = self.client_info.as_ref().map(|c| c.client_version.as_str()).unwrap_or("");

        let t0 = std::time::Instant::now();

        let mut req = self.http_client.post(&url)
            .header("content-type", "application/x-protobuf")
            .header("accept", "application/vnd.yt-ump")
            .header("x-goog-visitor-id", visitor_data)
            .header("x-youtube-client-name", client_name.to_string())
            .header("x-youtube-client-version", client_version)
            .header("origin", "https://www.youtube.com")
            .header("referer", format!("https://www.youtube.com/watch?v={}", self.video_id))
            .header("user-agent", USER_AGENT)
            .body(request_body);

        if let Some(ref token) = self.config.access_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let response = req.send().await.map_err(|e| format!("HTTP error: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            error!("SABR Fetch failed: {} - {}", status, error_text);
            return Err(format!("HTTP {}: {}", status, error_text));
        }

        let duration_ms = t0.elapsed().as_millis() as u64;
        let body = response.bytes().await.map_err(|e| format!("Body read error: {e}"))?;
        let response_bytes = body.len();

        let mut buffer = CompositeBuffer::new();
        buffer.append(&body);
        let mut ump = UmpReader::new(CompositeBuffer::new());
        ump.composite_buffer.append_buffer(&buffer);

        let mut saw = SawFlags {
            media: false, media_header: false, media_end: false,
            next_request_policy: false, playback_start_policy: false,
            request_identifier: false, request_cancellation_policy: false,
            sabr_error: false, sabr_redirect: false, sabr_context_update: false,
            stream_protection_status: false,
        };

        ump.read(&mut |part| {
            if part.part_type == UMPPartId::MEDIA { saw.media = true; }
            else if part.part_type == UMPPartId::MEDIA_HEADER { saw.media_header = true; }
            else if part.part_type == UMPPartId::MEDIA_END { saw.media_end = true; }
            else if part.part_type == UMPPartId::NEXT_REQUEST_POLICY { saw.next_request_policy = true; }
            else if part.part_type == UMPPartId::SABR_ERROR { saw.sabr_error = true; }
            else if part.part_type == UMPPartId::SABR_REDIRECT { saw.sabr_redirect = true; }
            else if part.part_type == UMPPartId::SABR_CONTEXT_UPDATE { saw.sabr_context_update = true; }
            else if part.part_type == UMPPartId::STREAM_PROTECTION_STATUS { saw.stream_protection_status = true; }

            match part.part_type {
                UMPPartId::FORMAT_INITIALIZATION_METADATA => self.handle_format_initialization_metadata(part),
                UMPPartId::SABR_REDIRECT => self.handle_sabr_redirect(part),
                UMPPartId::STREAM_PROTECTION_STATUS => self.handle_stream_protection_status(part),
                UMPPartId::MEDIA_HEADER => self.handle_media_header(part),
                UMPPartId::MEDIA => self.handle_media(part),
                UMPPartId::MEDIA_END => self.handle_media_end(part),
                UMPPartId::NEXT_REQUEST_POLICY => self.handle_next_request_policy(part),
                UMPPartId::SABR_CONTEXT_UPDATE => self.handle_sabr_context_update(part),
                UMPPartId::SABR_CONTEXT_SENDING_POLICY => self.handle_sabr_context_sending_policy(part),
                UMPPartId::SNACKBAR_MESSAGE => self.handle_snackbar_message(part),
                UMPPartId::RELOAD_PLAYER_RESPONSE => self.handle_reload_player_response(part),
                _ => {}
            }
        });

        // Update bandwidth
        if response_bytes > 0 && duration_ms > 0 {
            if saw.media || response_bytes > 5000 {
                self.update_bandwidth_estimate(response_bytes, duration_ms);
            }
        }

        if saw.media {
            self.media_headers_processed = true;
            self.cached_buffered_ranges = None;
            self.no_media_streak = 0;
        } else if let Some(ref policy) = self.next_request_policy {
            if policy.backoff_time_ms.unwrap_or(0) > 0 {
                self.no_media_streak += 1;
                self.cached_buffered_ranges = None;
                if self.no_media_streak >= 12 {
                    warn!("SABR Stall detected (noMediaStreak={}). Signaling for re-resolution.", self.no_media_streak);
                    self.no_media_streak = 0;
                }
            }
        }

        Ok(())
    }

    pub async fn start(&mut self, audio_itag: i32) {
        let audio_format = match self.format_ids.iter().find(|f| f.itag == audio_itag) {
            Some(f) => f.clone(),
            None => {
                error!("SABR Audio format not found: itag={}", audio_itag);
                return;
            }
        };

        if self.last_virtual_advance_at == 0 {
            self.last_virtual_advance_at = chrono_now_ms();
        }

        while !self.aborted && !self.stream_finished {
            if self.recovery_pending {
                sleep(Duration::from_millis(500)).await;
                continue;
            }

            let now = chrono_now_ms();
            let prev_player_time = self.virtual_player_time_ms;

            if self.total_downloaded_ms > self.virtual_player_time_ms as i64 {
                if self.last_virtual_advance_at > 0 {
                    self.virtual_player_time_ms += now - self.last_virtual_advance_at;
                }
                self.last_virtual_advance_at = now;
            } else if self.total_downloaded_ms > 0 {
                if self.last_virtual_advance_at > 0 {
                    let advance = now - self.last_virtual_advance_at;
                    self.virtual_player_time_ms = std::cmp::min(
                        self.virtual_player_time_ms + advance,
                        self.total_downloaded_ms,
                    );
                }
                self.last_virtual_advance_at = now;
            }

            if self.virtual_player_time_ms / 1000 != prev_player_time / 1000 {
                debug!("SABR Tracking: downloaded={}ms virtualPlayerTime={}ms->{}ms",
                    self.total_downloaded_ms, prev_player_time, self.virtual_player_time_ms);
            }

            // Rate limit requests
            if self.last_request_at > 0 {
                let since = now - self.last_request_at;
                if since < MIN_REQUEST_INTERVAL_MS as i64 {
                    sleep(Duration::from_millis((MIN_REQUEST_INTERVAL_MS as i64 - since) as u64)).await;
                }
            }
            self.last_request_at = chrono_now_ms();

            let state = AbrState {
                player_time_ms: self.total_downloaded_ms + self.start_time,
                bandwidth_estimate: std::cmp::max(self.bandwidth_estimate, 500_000),
                enabled_track_types_bitfield: 1,
                audio_track_id: audio_format.audio_track_id.clone().unwrap_or_default(),
                player_state: 1,
                visibility: 1,
                playback_rate: 1.0,
                sticky_resolution: 1080,
                last_manual_selected_resolution: 1080,
                client_viewport_is_flexible: false,
            };

            match self.fetch_and_process_segments(&state, &audio_format).await {
                Ok(_) => {}
                Err(e) => {
                    if self.aborted { break; }
                    warn!("SABR Fetch error: {}", e);
                    if e.contains("sabr.malformed_config") || e.contains("sabr.media_serving_enforcement_id_error") {
                        if e.contains("media_serving_enforcement_id_error") {
                            self.sabr_contexts.clear();
                            self.active_sabr_context_types.clear();
                        }
                        self.recovery_pending = true;
                        let current_rn = self.request_number;
                        while self.request_number == current_rn && !self.aborted {
                            sleep(Duration::from_millis(500)).await;
                        }
                        continue;
                    }
                    break;
                }
            }

            let no_backoff = self.next_request_policy.as_ref()
                .and_then(|p| p.backoff_time_ms)
                .unwrap_or(0) == 0;
            if no_backoff && self.initialized_formats_map.is_empty() {
                sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
