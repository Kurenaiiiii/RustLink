use base64::Engine;

pub struct ProtoWriter {
    chunks: Vec<Vec<u8>>,
    pub length: usize,
}

impl ProtoWriter {
    pub fn new() -> Self {
        Self { chunks: Vec::new(), length: 0 }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        self.length += bytes.len();
        self.chunks.push(bytes.to_vec());
    }

    fn push_byte(&mut self, byte: u8) {
        self.length += 1;
        self.chunks.push(vec![byte]);
    }

    pub fn write_varint(&mut self, value: u64) {
        let mut v = value;
        while v > 127 {
            self.push_byte((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        self.push_byte(v as u8);
    }

    pub fn write_tag(&mut self, field_number: u32, wire_type: u32) {
        self.write_varint(((field_number << 3) | wire_type) as u64);
    }

    pub fn write_string(&mut self, field_number: u32, value: Option<&str>) {
        let s = match value {
            Some(s) if !s.is_empty() => s,
            _ => return,
        };
        let buf = s.as_bytes();
        self.write_tag(field_number, 2);
        self.write_varint(buf.len() as u64);
        self.push_bytes(buf);
    }

    pub fn write_bytes(&mut self, field_number: u32, data: Option<&[u8]>) {
        let d = match data {
            Some(d) if !d.is_empty() => d,
            _ => return,
        };
        self.write_tag(field_number, 2);
        self.write_varint(d.len() as u64);
        self.push_bytes(d);
    }

    pub fn write_int32(&mut self, field_number: u32, value: Option<i32>) {
        let v = match value {
            Some(v) if v != 0 => v,
            _ => return,
        };
        self.write_tag(field_number, 0);
        self.write_varint(v as u64);
    }

    pub fn write_int64(&mut self, field_number: u32, value: Option<i64>) {
        let v = match value {
            Some(v) if v != 0 => v,
            _ => return,
        };
        self.write_tag(field_number, 0);
        self.write_varint(v as u64);
    }

    pub fn write_bool(&mut self, field_number: u32, value: Option<bool>) {
        match value {
            Some(true) => {}
            _ => return,
        }
        self.write_tag(field_number, 0);
        self.write_varint(1);
    }

    pub fn write_message(&mut self, field_number: u32, msg: &[u8]) {
        if msg.is_empty() {
            return;
        }
        self.write_tag(field_number, 2);
        self.write_varint(msg.len() as u64);
        self.push_bytes(msg);
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.length);
        for chunk in &self.chunks {
            buf.extend_from_slice(chunk);
        }
        buf
    }
}

pub struct ProtoReader<'a> {
    pub buffer: &'a [u8],
    pub pos: usize,
}

impl<'a> ProtoReader<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer, pos: 0 }
    }

    pub fn read_varint(&mut self) -> u64 {
        let mut result: u64 = 0;
        let mut shift: u64 = 0;
        loop {
            if self.pos >= self.buffer.len() {
                return result;
            }
            let b = self.buffer[self.pos];
            self.pos += 1;
            result |= ((b & 0x7f) as u64) << shift;
            shift += 7;
            if (b & 0x80) == 0 {
                break;
            }
        }
        result
    }

    pub fn read_i64(&mut self) -> i64 {
        self.read_varint() as i64
    }

    pub fn read_string(&mut self) -> String {
        let len = self.read_varint() as usize;
        let end = self.pos + len;
        if end > self.buffer.len() {
            return String::new();
        }
        let s = String::from_utf8_lossy(&self.buffer[self.pos..end]).to_string();
        self.pos = end;
        s
    }

    pub fn read_bytes(&mut self) -> &'a [u8] {
        let len = self.read_varint() as usize;
        let end = self.pos + len;
        if end > self.buffer.len() {
            return &[];
        }
        let bytes = &self.buffer[self.pos..end];
        self.pos = end;
        bytes
    }

    pub fn remaining(&self) -> &'a [u8] {
        &self.buffer[self.pos..]
    }

    pub fn skip(&mut self, wire_type: u32) {
        if self.pos >= self.buffer.len() {
            return;
        }
        match wire_type {
            0 => { self.read_varint(); }
            1 => { self.pos = std::cmp::min(self.pos + 8, self.buffer.len()); }
            2 => {
                let len = self.read_varint() as usize;
                self.pos = std::cmp::min(self.pos + len, self.buffer.len());
            }
            5 => { self.pos = std::cmp::min(self.pos + 4, self.buffer.len()); }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FormatIdMsg {
    pub itag: i32,
    pub last_modified: Option<String>,
    pub xtags: Option<String>,
}

pub fn encode_format_id(msg: &FormatIdMsg, w: &mut ProtoWriter) {
    w.write_int32(1, Some(msg.itag));
    if let Some(ref lm) = msg.last_modified {
        if let Ok(v) = lm.parse::<i64>() {
            w.write_int64(2, Some(v));
        }
    }
    w.write_string(3, msg.xtags.as_deref());
}

pub fn decode_format_id(reader: &mut ProtoReader, len: usize) -> FormatIdMsg {
    let end = reader.pos + len;
    let mut msg = FormatIdMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.itag = reader.read_varint() as i32;
        } else if field == 2 {
            msg.last_modified = Some(reader.read_varint().to_string());
        } else if field == 3 {
            msg.xtags = Some(reader.read_string());
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct ClientAbrStateMsg {
    pub last_manual_selected_resolution: Option<i32>,
    pub sticky_resolution: Option<i32>,
    pub client_viewport_is_flexible: Option<bool>,
    pub bandwidth_estimate: Option<i64>,
    pub player_time_ms: Option<i64>,
    pub visibility: Option<i32>,
    pub playback_rate: Option<f32>,
    pub time_since_last_action_ms: Option<i64>,
    pub enabled_track_types_bitfield: Option<i32>,
    pub player_state: Option<i64>,
    pub drc_enabled: Option<bool>,
    pub audio_track_id: Option<String>,
}

pub fn encode_client_abr_state(msg: &ClientAbrStateMsg, w: &mut ProtoWriter) {
    w.write_int32(16, msg.last_manual_selected_resolution);
    w.write_int32(21, msg.sticky_resolution);
    w.write_bool(22, msg.client_viewport_is_flexible);
    w.write_int64(23, msg.bandwidth_estimate);
    w.write_int64(28, msg.player_time_ms);
    w.write_int32(34, msg.visibility);
    w.write_int32(40, msg.enabled_track_types_bitfield);
    w.write_int64(44, msg.player_state);
    w.write_bool(46, msg.drc_enabled);
    w.write_string(69, msg.audio_track_id.as_deref());
}

#[derive(Debug, Clone, Default)]
pub struct ClientInfoMsg {
    pub client_name: i32,
    pub client_version: String,
}

pub fn encode_client_info(msg: &ClientInfoMsg, w: &mut ProtoWriter) {
    w.write_int32(16, Some(msg.client_name));
    w.write_string(17, Some(&msg.client_version));
}

#[derive(Debug, Clone, Default)]
pub struct TimeRangeMsg {
    pub start_ticks: String,
    pub duration_ticks: String,
    pub timescale: i32,
}

pub fn encode_time_range(msg: &TimeRangeMsg, w: &mut ProtoWriter) {
    if let Ok(v) = msg.start_ticks.parse::<i64>() {
        w.write_int64(1, Some(v));
    }
    if let Ok(v) = msg.duration_ticks.parse::<i64>() {
        w.write_int64(2, Some(v));
    }
    w.write_int32(3, Some(msg.timescale));
}

pub fn decode_time_range(reader: &mut ProtoReader, len: usize) -> TimeRangeMsg {
    let end = reader.pos + len;
    let mut msg = TimeRangeMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.start_ticks = reader.read_varint().to_string();
        } else if field == 2 {
            msg.duration_ticks = reader.read_varint().to_string();
        } else if field == 3 {
            msg.timescale = reader.read_varint() as i32;
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct BufferedRangeMsg {
    pub format_id: Option<FormatIdMsg>,
    pub start_time_ms: Option<String>,
    pub duration_ms: Option<String>,
    pub start_segment_index: Option<i32>,
    pub end_segment_index: Option<i32>,
    pub time_range: Option<TimeRangeMsg>,
}

pub fn encode_buffered_range(msg: &BufferedRangeMsg, w: &mut ProtoWriter) {
    if let Some(ref fid) = msg.format_id {
        let mut sw = ProtoWriter::new();
        encode_format_id(fid, &mut sw);
        w.write_message(1, &sw.finish());
    }
    if let Some(ref st) = msg.start_time_ms {
        if let Ok(v) = st.parse::<i64>() {
            w.write_int64(2, Some(v));
        }
    }
    if let Some(ref dm) = msg.duration_ms {
        if let Ok(v) = dm.parse::<i64>() {
            w.write_int64(3, Some(v));
        }
    }
    w.write_int32(4, msg.start_segment_index);
    w.write_int32(5, msg.end_segment_index);
    if let Some(ref tr) = msg.time_range {
        let mut sw = ProtoWriter::new();
        encode_time_range(tr, &mut sw);
        w.write_message(6, &sw.finish());
    }
}

#[derive(Debug, Clone, Default)]
pub struct MediaHeaderMsg {
    pub header_id: Option<i32>,
    pub itag: i32,
    pub lmt: Option<String>,
    pub xtags: Option<String>,
    pub is_init_seg: bool,
    pub sequence_number: i32,
    pub start_ms: String,
    pub duration_ms: String,
    pub format_id: Option<FormatIdMsg>,
    pub content_length: Option<String>,
    pub time_range: Option<TimeRangeMsg>,
}

pub fn decode_media_header(reader: &mut ProtoReader, len: usize) -> MediaHeaderMsg {
    let end = reader.pos + len;
    let mut msg = MediaHeaderMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.header_id = Some(reader.read_varint() as i32);
        } else if field == 3 {
            msg.itag = reader.read_varint() as i32;
        } else if field == 4 {
            msg.lmt = Some(reader.read_varint().to_string());
        } else if field == 5 {
            msg.xtags = Some(reader.read_string());
        } else if field == 8 {
            msg.is_init_seg = reader.read_varint() != 0;
        } else if field == 9 {
            msg.sequence_number = reader.read_varint() as i32;
        } else if field == 11 {
            msg.start_ms = reader.read_varint().to_string();
        } else if field == 12 {
            msg.duration_ms = reader.read_varint().to_string();
        } else if field == 13 {
            let sub_len = reader.read_varint() as usize;
            msg.format_id = Some(decode_format_id(reader, sub_len));
            if let Some(ref fid) = msg.format_id {
                if fid.itag != 0 {
                    msg.itag = fid.itag;
                }
                if fid.xtags.is_some() {
                    msg.xtags = fid.xtags.clone();
                }
            }
        } else if field == 14 {
            msg.content_length = Some(reader.read_varint().to_string());
        } else if field == 15 {
            let sub_len = reader.read_varint() as usize;
            msg.time_range = Some(decode_time_range(reader, sub_len));
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct FormatInitializationMetadataMsg {
    pub format_id: Option<FormatIdMsg>,
    pub itag: Option<i32>,
    pub end_segment_number: Option<String>,
    pub mime_type: Option<String>,
    pub duration_units: Option<String>,
    pub duration_timescale: Option<String>,
}

pub fn decode_format_initialization_metadata(
    reader: &mut ProtoReader,
    len: usize,
) -> FormatInitializationMetadataMsg {
    let end = reader.pos + len;
    let mut msg = FormatInitializationMetadataMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 2 {
            let sub_len = reader.read_varint() as usize;
            msg.format_id = Some(decode_format_id(reader, sub_len));
            msg.itag = msg.format_id.as_ref().map(|f| f.itag);
        } else if field == 4 {
            msg.end_segment_number = Some(reader.read_varint().to_string());
        } else if field == 5 {
            msg.mime_type = Some(reader.read_string());
        } else if field == 9 {
            msg.duration_units = Some(reader.read_varint().to_string());
        } else if field == 10 {
            msg.duration_timescale = Some(reader.read_varint().to_string());
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct StreamProtectionStatusMsg {
    pub status: Option<i32>,
}

pub fn decode_stream_protection_status(
    reader: &mut ProtoReader,
    len: usize,
) -> StreamProtectionStatusMsg {
    let end = reader.pos + len;
    let mut msg = StreamProtectionStatusMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.status = Some(reader.read_varint() as i32);
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct SabrRedirectMsg {
    pub url: Option<String>,
}

pub fn decode_sabr_redirect(reader: &mut ProtoReader, len: usize) -> SabrRedirectMsg {
    let end = reader.pos + len;
    let mut msg = SabrRedirectMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 && wire_type == 2 {
            msg.url = Some(reader.read_string());
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct SabrErrorMsg {
    pub err_type: Option<String>,
    pub code: Option<i32>,
}

pub fn decode_sabr_error(reader: &mut ProtoReader, len: usize) -> SabrErrorMsg {
    let end = reader.pos + len;
    let mut msg = SabrErrorMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 && wire_type == 2 {
            msg.err_type = Some(reader.read_string());
        } else if field == 2 {
            msg.code = Some(reader.read_varint() as i32);
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct SnackbarMessageMsg {
    pub id: Option<i32>,
}

pub fn decode_snackbar_message(reader: &mut ProtoReader, len: usize) -> SnackbarMessageMsg {
    let end = reader.pos + len;
    let mut msg = SnackbarMessageMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.id = Some(reader.read_varint() as i32);
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone)]
pub struct SabrContextUpdateMsg {
    pub ctx_type: Option<i32>,
    pub scope: Option<i32>,
    pub value: Option<Vec<u8>>,
    pub send_by_default: Option<bool>,
    pub write_policy: Option<i32>,
}

pub fn decode_sabr_context_update(reader: &mut ProtoReader, len: usize) -> SabrContextUpdateMsg {
    let end = reader.pos + len;
    let mut msg = SabrContextUpdateMsg {
        ctx_type: None,
        scope: None,
        value: None,
        send_by_default: None,
        write_policy: None,
    };
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.ctx_type = Some(reader.read_varint() as i32);
        } else if field == 2 {
            msg.scope = Some(reader.read_varint() as i32);
        } else if field == 3 && wire_type == 2 {
            msg.value = Some(reader.read_bytes().to_vec());
        } else if field == 4 {
            msg.send_by_default = Some(reader.read_varint() != 0);
        } else if field == 5 {
            msg.write_policy = Some(reader.read_varint() as i32);
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct SabrContextSendingPolicyMsg {
    pub start_policy: Vec<i32>,
    pub stop_policy: Vec<i32>,
    pub discard_policy: Vec<i32>,
}

pub fn decode_sabr_context_sending_policy(
    reader: &mut ProtoReader,
    len: usize,
) -> SabrContextSendingPolicyMsg {
    let end = reader.pos + len;
    let mut msg = SabrContextSendingPolicyMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.start_policy.push(reader.read_varint() as i32);
        } else if field == 2 {
            msg.stop_policy.push(reader.read_varint() as i32);
        } else if field == 3 {
            msg.discard_policy.push(reader.read_varint() as i32);
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct NextRequestPolicyMsg {
    pub target_audio_readahead_ms: Option<i32>,
    pub target_video_readahead_ms: Option<i32>,
    pub max_time_since_last_request_ms: Option<i32>,
    pub backoff_time_ms: Option<i32>,
    pub min_audio_readahead_ms: Option<i32>,
    pub min_video_readahead_ms: Option<i32>,
    pub playback_cookie: Option<Vec<u8>>,
}

pub fn decode_next_request_policy(
    reader: &mut ProtoReader,
    len: usize,
) -> NextRequestPolicyMsg {
    let end = reader.pos + len;
    let mut msg = NextRequestPolicyMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 {
            msg.target_audio_readahead_ms = Some(reader.read_varint() as i32);
        } else if field == 2 {
            msg.target_video_readahead_ms = Some(reader.read_varint() as i32);
        } else if field == 3 {
            msg.max_time_since_last_request_ms = Some(reader.read_varint() as i32);
        } else if field == 4 {
            msg.backoff_time_ms = Some(reader.read_varint() as i32);
        } else if field == 5 {
            msg.min_audio_readahead_ms = Some(reader.read_varint() as i32);
        } else if field == 6 {
            msg.min_video_readahead_ms = Some(reader.read_varint() as i32);
        } else if field == 7 && wire_type == 2 {
            msg.playback_cookie = Some(reader.read_bytes().to_vec());
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

#[derive(Debug, Clone, Default)]
pub struct RequestIdentifierMsg {
    pub id: Option<String>,
}

pub fn decode_request_identifier(reader: &mut ProtoReader, len: usize) -> RequestIdentifierMsg {
    let end = reader.pos + len;
    let mut msg = RequestIdentifierMsg::default();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        let field = tag >> 3;
        let wire_type = tag & 7;
        if field == 1 && wire_type == 2 {
            msg.id = Some(reader.read_string());
        } else {
            reader.skip(wire_type);
        }
    }
    msg
}

pub fn decode_generic_message(reader: &mut ProtoReader, len: usize) -> serde_json::Value {
    let end = reader.pos + len;
    let mut map = serde_json::Map::new();
    while reader.pos < end {
        let tag = reader.read_varint() as u32;
        if tag == 0 {
            break;
        }
        let field = tag >> 3;
        let wire_type = tag & 7;
        let key = field.to_string();
        if wire_type == 0 {
            let v = reader.read_varint();
            map.insert(key, serde_json::Value::from(v as i64));
        } else if wire_type == 2 {
            let bytes = reader.read_bytes().to_vec();
            let entry = serde_json::json!({
                "len": bytes.len(),
                "utf8": String::from_utf8_lossy(&bytes).to_string()
            });
            map.insert(key, entry);
        } else {
            reader.skip(wire_type);
        }
    }
    serde_json::Value::Object(map)
}

#[derive(Debug, Clone, Default)]
pub struct StreamerContextMsg {
    pub client_info: Option<ClientInfoMsg>,
    pub po_token: Option<Vec<u8>>,
    pub playback_cookie: Option<Vec<u8>>,
    pub sabr_contexts: Vec<SabrContextEntry>,
    pub unsent_sabr_contexts: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct SabrContextEntry {
    pub ctx_type: i32,
    pub value: Vec<u8>,
}

pub fn encode_streamer_context(msg: &StreamerContextMsg, w: &mut ProtoWriter) {
    if let Some(ref ci) = msg.client_info {
        let mut sw = ProtoWriter::new();
        encode_client_info(ci, &mut sw);
        w.write_message(1, &sw.finish());
    }
    w.write_bytes(2, msg.po_token.as_deref());
    w.write_bytes(3, msg.playback_cookie.as_deref());
    for ctx in &msg.sabr_contexts {
        let mut sw = ProtoWriter::new();
        sw.write_int32(1, Some(ctx.ctx_type));
        sw.write_bytes(2, Some(&ctx.value));
        w.write_message(5, &sw.finish());
    }
    for &typ in &msg.unsent_sabr_contexts {
        w.write_int32(6, Some(typ));
    }
}

pub fn encode_video_playback_abr_request(
    client_abr_state: Option<&ClientAbrStateMsg>,
    selected_format_ids: &[FormatIdMsg],
    buffered_ranges: &[BufferedRangeMsg],
    video_playback_ustreamer_config: Option<&[u8]>,
    preferred_audio_format_ids: &[FormatIdMsg],
    preferred_video_format_ids: &[FormatIdMsg],
    streamer_context: Option<&StreamerContextMsg>,
) -> Vec<u8> {
    let mut w = ProtoWriter::new();
    if let Some(ref cas) = client_abr_state {
        let mut sw = ProtoWriter::new();
        encode_client_abr_state(cas, &mut sw);
        w.write_message(1, &sw.finish());
    }
    for fid in selected_format_ids {
        let mut sw = ProtoWriter::new();
        encode_format_id(fid, &mut sw);
        w.write_message(2, &sw.finish());
    }
    for br in buffered_ranges {
        let mut sw = ProtoWriter::new();
        encode_buffered_range(br, &mut sw);
        w.write_message(3, &sw.finish());
    }
    w.write_bytes(5, video_playback_ustreamer_config);
    for fid in preferred_audio_format_ids {
        let mut sw = ProtoWriter::new();
        encode_format_id(fid, &mut sw);
        w.write_message(16, &sw.finish());
    }
    for fid in preferred_video_format_ids {
        let mut sw = ProtoWriter::new();
        encode_format_id(fid, &mut sw);
        w.write_message(17, &sw.finish());
    }
    if let Some(ref sc) = streamer_context {
        let mut sw = ProtoWriter::new();
        encode_streamer_context(sc, &mut sw);
        w.write_message(19, &sw.finish());
    }
    w.finish()
}

pub struct UMPPartId;

impl UMPPartId {
    pub const FORMAT_INITIALIZATION_METADATA: u32 = 42;
    pub const NEXT_REQUEST_POLICY: u32 = 35;
    pub const SABR_ERROR: u32 = 44;
    pub const SABR_REDIRECT: u32 = 43;
    pub const PLAYBACK_START_POLICY: u32 = 47;
    pub const REQUEST_IDENTIFIER: u32 = 52;
    pub const REQUEST_CANCELLATION_POLICY: u32 = 53;
    pub const SABR_CONTEXT_UPDATE: u32 = 57;
    pub const SABR_CONTEXT_SENDING_POLICY: u32 = 59;
    pub const STREAM_PROTECTION_STATUS: u32 = 58;
    pub const RELOAD_PLAYER_RESPONSE: u32 = 46;
    pub const MEDIA_HEADER: u32 = 20;
    pub const MEDIA: u32 = 21;
    pub const MEDIA_END: u32 = 22;
    pub const SNACKBAR_MESSAGE: u32 = 67;
}

pub fn ump_part_name(typ: u32) -> &'static str {
    match typ {
        UMPPartId::FORMAT_INITIALIZATION_METADATA => "FORMAT_INITIALIZATION_METADATA",
        UMPPartId::NEXT_REQUEST_POLICY => "NEXT_REQUEST_POLICY",
        UMPPartId::SABR_ERROR => "SABR_ERROR",
        UMPPartId::SABR_REDIRECT => "SABR_REDIRECT",
        UMPPartId::PLAYBACK_START_POLICY => "PLAYBACK_START_POLICY",
        UMPPartId::REQUEST_IDENTIFIER => "REQUEST_IDENTIFIER",
        UMPPartId::REQUEST_CANCELLATION_POLICY => "REQUEST_CANCELLATION_POLICY",
        UMPPartId::SABR_CONTEXT_UPDATE => "SABR_CONTEXT_UPDATE",
        UMPPartId::SABR_CONTEXT_SENDING_POLICY => "SABR_CONTEXT_SENDING_POLICY",
        UMPPartId::STREAM_PROTECTION_STATUS => "STREAM_PROTECTION_STATUS",
        UMPPartId::RELOAD_PLAYER_RESPONSE => "RELOAD_PLAYER_RESPONSE",
        UMPPartId::MEDIA_HEADER => "MEDIA_HEADER",
        UMPPartId::MEDIA => "MEDIA",
        UMPPartId::MEDIA_END => "MEDIA_END",
        UMPPartId::SNACKBAR_MESSAGE => "SNACKBAR_MESSAGE",
        _ => "UNKNOWN",
    }
}

pub struct EnabledTrackTypes;

impl EnabledTrackTypes {
    pub const VIDEO_AND_AUDIO: i32 = 0;
    pub const AUDIO_ONLY: i32 = 1;
    pub const VIDEO_ONLY: i32 = 2;
}

pub fn base64_to_u8(input: &str) -> Vec<u8> {
    let s = input.replace('-', "+").replace('_', "/");
    let mod_len = s.len() & 3;
    let padded = if mod_len != 0 {
        format!("{}{}", s, "=".repeat(4 - mod_len))
    } else {
        s
    };
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .unwrap_or_default()
}

pub fn u8_to_base64(data: &[u8]) -> String {
    use base64::Engine;
    let s = base64::engine::general_purpose::STANDARD.encode(data);
    s.replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
}

pub fn concatenate_chunks(chunks: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    let mut result = Vec::with_capacity(total);
    for chunk in chunks {
        result.extend_from_slice(chunk);
    }
    result
}

pub struct UMPWriter {
    chunks: Vec<Vec<u8>>,
}

impl UMPWriter {
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    pub fn write(&mut self, part_type: u32, part_data: &[u8]) {
        self.write_varint(part_type);
        self.write_varint(part_data.len() as u32);
        self.chunks.push(part_data.to_vec());
    }

    fn write_varint(&mut self, value: u32) {
        if value < 128 {
            self.chunks.push(vec![value as u8]);
        } else if value < 16384 {
            self.chunks.push(vec![
                ((value & 0x3f) | 0x80) as u8,
                (value >> 6) as u8,
            ]);
        } else if value < 2097152 {
            self.chunks.push(vec![
                ((value & 0x1f) | 0xc0) as u8,
                ((value >> 5) & 0xff) as u8,
                (value >> 13) as u8,
            ]);
        } else if value < 268435456 {
            self.chunks.push(vec![
                ((value & 0x0f) | 0xe0) as u8,
                ((value >> 4) & 0xff) as u8,
                ((value >> 12) & 0xff) as u8,
                (value >> 20) as u8,
            ]);
        } else {
            let mut data = vec![0u8; 5];
            data[0] = 0xf0;
            let bytes = (value as u32).to_le_bytes();
            data[1..5].copy_from_slice(&bytes);
            self.chunks.push(data);
        }
    }

    pub fn finish(&mut self) -> Vec<u8> {
        concatenate_chunks(&self.chunks)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        let values = vec![0u64, 1, 127, 128, 255, 1000, 1_000_000, u64::MAX];
        for v in &values {
            let mut w = ProtoWriter::new();
            w.write_varint(*v);
            let bytes = w.finish();
            let mut r = ProtoReader::new(&bytes);
            let decoded = r.read_varint();
            assert_eq!(decoded, *v, "varint roundtrip failed for {}", v);
            assert_eq!(r.pos, bytes.len(), "not all bytes consumed for {}", v);
        }
    }

    #[test]
    fn test_bytes_write_read() {
        let data = b"hello protobuf";
        let mut w = ProtoWriter::new();
        w.write_bytes(1, Some(data));
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let tag = r.read_varint();
        assert_eq!(tag >> 3, 1);
        assert_eq!(tag & 7, 2);
        let decoded = r.read_bytes();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_write_tag() {
        let mut w = ProtoWriter::new();
        w.write_tag(5, 2);
        w.write_varint(42);
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let tag = r.read_varint();
        assert_eq!(tag >> 3, 5);
        assert_eq!(tag & 7, 2);
        let val = r.read_varint();
        assert_eq!(val, 42);
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"test base64 data with \0 bytes \xFF\xFE";
        let encoded = u8_to_base64(data);
        let decoded = base64_to_u8(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_base64_url_safe() {
        let data = b"\xFB\xFF\xFE\xFD";
        let encoded = u8_to_base64(data);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
        let decoded = base64_to_u8(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_sha256() {
        let result = sha256_hex(b"hello");
        assert_eq!(result.len(), 64);
        assert_eq!(
            result,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_concatenate_chunks() {
        let chunks = vec![vec![1, 2, 3], vec![4, 5], vec![6]];
        let result = concatenate_chunks(&chunks);
        assert_eq!(result, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_format_id_encode_decode() {
        let original = FormatIdMsg {
            itag: 251,
            last_modified: Some("12345".into()),
            xtags: None,
        };
        let mut w = ProtoWriter::new();
        encode_format_id(&original, &mut w);
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let decoded = decode_format_id(&mut r, bytes.len());
        assert_eq!(decoded.itag, original.itag);
        assert_eq!(decoded.last_modified, original.last_modified);
    }

    #[test]
    fn test_client_info_encode() {
        let msg = ClientInfoMsg {
            client_name: 3,
            client_version: "1.0.0".into(),
        };
        let mut w = ProtoWriter::new();
        encode_client_info(&msg, &mut w);
        let bytes = w.finish();
        assert!(!bytes.is_empty());
        let as_str = String::from_utf8_lossy(&bytes);
        assert!(as_str.contains("1.0.0"));
    }

    #[test]
    fn test_time_range_encode_decode() {
        let original = TimeRangeMsg {
            start_ticks: "1000".into(),
            duration_ticks: "4000".into(),
            timescale: 1000,
        };
        let mut w = ProtoWriter::new();
        encode_time_range(&original, &mut w);
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let decoded = decode_time_range(&mut r, bytes.len());
        assert_eq!(decoded.start_ticks, original.start_ticks);
        assert_eq!(decoded.duration_ticks, original.duration_ticks);
        assert_eq!(decoded.timescale, original.timescale);
    }

    #[test]
    fn test_ump_writer() {
        let mut uw = UMPWriter::new();
        uw.write(0x01, b"data1");
        uw.write(0x02, b"data2");
        let result = uw.finish();
        assert!(!result.is_empty());
        assert!(result.len() >= 14);
    }

    #[test]
    fn test_ump_part_id_constants() {
        assert_eq!(UMPPartId::PLAYBACK_START_POLICY, 47);
        assert_eq!(UMPPartId::REQUEST_IDENTIFIER, 52);
        assert_eq!(UMPPartId::MEDIA_HEADER, 20);
        assert_eq!(UMPPartId::SABR_ERROR, 44);
        assert_eq!(UMPPartId::SNACKBAR_MESSAGE, 67);
    }

    #[test]
    fn test_enabled_track_types() {
        assert_eq!(EnabledTrackTypes::VIDEO_AND_AUDIO, 0);
        assert_eq!(EnabledTrackTypes::AUDIO_ONLY, 1);
        assert_eq!(EnabledTrackTypes::VIDEO_ONLY, 2);
    }

    #[test]
    fn test_streamer_context_encode() {
        let msg = StreamerContextMsg {
            client_info: None,
            po_token: None,
            playback_cookie: None,
            sabr_contexts: vec![],
            unsent_sabr_contexts: vec![],
        };
        let mut w = ProtoWriter::new();
        encode_streamer_context(&msg, &mut w);
        let bytes = w.finish();
        assert!(bytes.is_empty() || bytes.len() < 5);
    }

    #[test]
    fn test_proto_reader_skip() {
        let bytes = vec![0x64, 0xC8, 0x01, 0xAC, 0x02];
        let mut r = ProtoReader::new(&bytes);
        let first = r.read_varint();
        assert_eq!(first, 100);
        // skip remaining bytes by wire type
        r.skip(0);
        assert!(r.pos <= bytes.len());
    }

    #[test]
    fn test_empty_proto_writer() {
        let mut w = ProtoWriter::new();
        let bytes = w.finish();
        assert!(bytes.is_empty());
        assert_eq!(w.length, 0);
    }

    #[test]
    fn test_proto_reader_empty() {
        let mut r = ProtoReader::new(b"");
        let val = r.read_varint();
        assert_eq!(val, 0);
    }

    #[test]
    fn test_large_varint() {
        let val = 0xFFFFFFFFFFFFFFFFu64;
        let mut w = ProtoWriter::new();
        w.write_varint(val);
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let decoded = r.read_varint();
        assert_eq!(decoded, val);
        assert!(bytes.len() <= 10);
    }

    #[test]
    fn test_write_string() {
        let mut w = ProtoWriter::new();
        w.write_string(1, Some("hello"));
        let bytes = w.finish();
        assert!(!bytes.is_empty());
        let mut r = ProtoReader::new(&bytes);
        let _tag = r.read_varint();
        let s = r.read_string();
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_write_int32() {
        let mut w = ProtoWriter::new();
        w.write_int32(1, Some(42));
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let tag = r.read_varint();
        assert_eq!(tag >> 3, 1);
        assert_eq!(r.read_varint(), 42);
    }

    #[test]
    fn test_write_int64() {
        let mut w = ProtoWriter::new();
        w.write_int64(1, Some(9999999999));
        let bytes = w.finish();
        let mut r = ProtoReader::new(&bytes);
        let tag = r.read_varint();
        assert_eq!(tag >> 3, 1);
        assert_eq!(r.read_varint(), 9999999999);
    }
}
