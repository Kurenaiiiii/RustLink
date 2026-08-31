use std::collections::VecDeque;

const SAMPLE_RATE: u32 = 48000;

// --- FLV Demuxer ---

#[derive(Debug, Clone)]
pub struct FlvAudioTag {
    pub codec_id: u8,
    pub sample_rate: u32,
    pub sample_size: u8,
    pub channels: u8,
    pub aac_packet_type: Option<u8>,
    pub data: Vec<u8>,
    pub timestamp: u32,
}

pub struct FlvDemuxer {
    buffer: Vec<u8>,
    pos: usize,
    has_audio: bool,
    tags: VecDeque<FlvAudioTag>,
    has_parsed_header: bool,
}

impl FlvDemuxer {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            pos: 0,
            has_audio: false,
            tags: VecDeque::new(),
            has_parsed_header: false,
        }
    }

    pub fn push_data(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        if !self.has_parsed_header && self.buffer.len() >= 9 {
            self.parse_header();
        }
        self.parse_tags();
    }

    fn parse_header(&mut self) {
        if self.buffer.len() < 9 {
            return;
        }
        if &self.buffer[..3] != b"FLV" {
            return;
        }
        let flags = self.buffer[4];
        self.has_audio = (flags & 4) != 0;
        let header_size = u32::from_be_bytes([
            0,
            self.buffer[5],
            self.buffer[6],
            self.buffer[7],
        ]) as usize;
        self.pos = header_size.max(9);
        self.has_parsed_header = true;
    }

    fn parse_tags(&mut self) {
        loop {
            if self.pos + 11 > self.buffer.len() {
                break;
            }
            let tag_type = self.buffer[self.pos];
            let data_size = u32::from_be_bytes([
                0,
                self.buffer[self.pos + 1],
                self.buffer[self.pos + 2],
                self.buffer[self.pos + 3],
            ]) as usize;
            let timestamp = u32::from_be_bytes([
                self.buffer[self.pos + 4],
                self.buffer[self.pos + 5],
                self.buffer[self.pos + 6],
                self.buffer[self.pos + 7],
            ]);
            let _stream_id = u32::from_be_bytes([
                0,
                self.buffer[self.pos + 8],
                self.buffer[self.pos + 9],
                self.buffer[self.pos + 10],
            ]);
            let tag_end = self.pos + 11 + data_size + 4;

            if tag_end > self.buffer.len() {
                break;
            }

            if tag_type == 8 {
                let tag_data = &self.buffer[self.pos + 11..self.pos + 11 + data_size];
                if let Some(audio_tag) = self.parse_audio_tag(tag_data, timestamp) {
                    self.tags.push_back(audio_tag);
                }
            }

            self.pos = tag_end;
        }

        if self.pos > 0 {
            self.buffer.drain(..self.pos);
            self.pos = 0;
        }
    }

    fn parse_audio_tag(&self, data: &[u8], timestamp: u32) -> Option<FlvAudioTag> {
        if data.is_empty() {
            return None;
        }
        let first = data[0];
        let codec_id = (first >> 4) & 0x0f;
        let sample_rate_code = (first >> 2) & 0x03;
        let sample_size = if ((first >> 1) & 0x01) != 0 { 16 } else { 8 };
        let channels = if (first & 0x01) != 0 { 2 } else { 1 };

        let sample_rate = match sample_rate_code {
            0 => 5500,
            1 => 11000,
            2 => 22000,
            3 => 44100,
            _ => 44100,
        };

        let (aac_packet_type, audio_data) = if codec_id == 10 && data.len() > 1 {
            let pkt_type = data[1];
            (Some(pkt_type), data[2..].to_vec())
        } else {
            (None, data[1..].to_vec())
        };

        Some(FlvAudioTag {
            codec_id,
            sample_rate,
            sample_size,
            channels,
            aac_packet_type,
            data: audio_data,
            timestamp,
        })
    }

    pub fn has_audio(&self) -> bool {
        self.has_audio
    }

    pub fn next_audio_tag(&mut self) -> Option<FlvAudioTag> {
        self.tags.pop_front()
    }

    pub fn is_exhausted(&self) -> bool {
        !self.has_parsed_header || self.buffer.is_empty()
    }
}

// --- WebM/Opus Demuxer ---

#[derive(Debug, Clone)]
pub struct WebmPacket {
    pub data: Vec<u8>,
    pub timestamp: u64,
    pub duration: u64,
    pub keyframe: bool,
}

#[derive(Debug, Clone)]
pub struct WebmTrackInfo {
    pub track_number: u64,
    pub codec_id: String,
    pub sample_rate: f64,
    pub channels: u64,
}

pub struct WebmDemuxer {
    buffer: Vec<u8>,
    pos: usize,
    tracks: Vec<WebmTrackInfo>,
    packets: VecDeque<WebmPacket>,
    cluster_time: u64,
    parsed: bool,
    timecode_scale: u64,
}

impl WebmDemuxer {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            pos: 0,
            tracks: Vec::new(),
            packets: VecDeque::new(),
            cluster_time: 0,
            parsed: false,
            timecode_scale: 1000000,
        }
    }

    pub fn push_data(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        if !self.parsed && self.buffer.len() >= 1024 {
            self.parse_headers();
        }
        if self.parsed {
            self.parse_clusters();
        }
    }

    fn read_varint(data: &[u8], pos: &mut usize) -> Option<(u64, usize)> {
        if *pos >= data.len() {
            return None;
        }
        let first = data[*pos];
        if first == 0 {
            return None;
        }
        let mut mask: u8 = 0x80;
        let mut len = 1;
        while (first & mask) == 0 && len <= 8 {
            mask >>= 1;
            len += 1;
        }
        if len > 8 {
            return None;
        }
        let mut value = (first & (mask - 1)) as u64;
        for i in 1..len {
            if *pos + i >= data.len() {
                return None;
            }
            value = (value << 8) | data[*pos + i] as u64;
        }
        *pos += len;
        Some((value, len))
    }

    fn read_ebml_element(data: &[u8], pos: &mut usize) -> Option<(u64, u64)> {
        let id = Self::read_varint(data, pos)?;
        let (size, _) = Self::read_varint(data, pos)?;
        Some((id.0, size))
    }

    fn parse_headers(&mut self) {
        if self.buffer.len() < 9 {
            return;
        }
        let header_buf = self.buffer.clone();
        let mut pos = 0;

        // EBML header (first 4 bytes identify EBML)
        if pos + 4 > header_buf.len() {
            return;
        }

        // Skip EBML header element
        let _ = Self::read_ebml_element(&header_buf, &mut pos);

        // Look for Segment
        loop {
            if pos + 2 > header_buf.len() {
                return;
            }

            let element = Self::read_ebml_element(&header_buf, &mut pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let end = pos + size as usize;

            match id {
                0x18538067 => {
                    self.parse_segment_headers(&header_buf, &mut pos, end);
                    break;
                }
                _ => {
                    pos = end;
                }
            }
        }
        self.parsed = true;
    }

    fn parse_segment_headers(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        while *pos + 2 <= end && *pos < data.len() {
            let element = Self::read_ebml_element(data, pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let elem_end = *pos + size as usize;

            match id {
                0x1549a966 => {
                    self.parse_info(data, pos, elem_end);
                }
                0x1654ae6b => {
                    self.parse_tracks(data, pos, elem_end);
                }
                0x1f43b675 => {
                    // First cluster found - stop header parsing
                    return;
                }
                _ => {}
            }
            *pos = elem_end;
        }
    }

    fn parse_info(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        while *pos + 2 <= end && *pos < data.len() {
            let element = Self::read_ebml_element(data, pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let elem_end = *pos + size as usize;

                if id == 0x2ad7b1 {
                    // TimecodeScale — 4-byte unsigned int
                    if *pos + 4 <= data.len() {
                        self.timecode_scale = u32::from_be_bytes([
                            data[*pos],
                            data[*pos + 1],
                            data[*pos + 2],
                            data[*pos + 3],
                        ]) as u64;
                    }
                }
            *pos = elem_end;
        }
    }

    fn parse_tracks(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        while *pos + 2 <= end && *pos < data.len() {
            let element = Self::read_ebml_element(data, pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let elem_end = *pos + size as usize;

            if id == 0xae {
                // TrackEntry
                self.parse_track_entry(data, pos, elem_end);
            }
            *pos = elem_end;
        }
    }

    fn parse_track_entry(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        let mut track_number = 0;
        let mut codec_id = String::new();
        let mut sample_rate = 8000.0;
        let mut channels = 1;

        while *pos + 2 <= end && *pos < data.len() {
            let element = Self::read_ebml_element(data, pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let elem_end = *pos + size as usize;

            match id {
                0xd7 => {
                    // TrackNumber
                    if let Some((v, _)) = Self::read_varint(data, pos) {
                        track_number = v;
                    }
                }
                0x86 => {
                    // CodecID
                    if *pos + size as usize <= data.len() {
                        codec_id = String::from_utf8_lossy(&data[*pos..elem_end]).to_string();
                    }
                    *pos = elem_end;
                }
                0xb5 => {
                    sample_rate = 0.0;
                    if *pos + 4 <= data.len() {
                        let bits = u64::from_be_bytes([
                            0,
                            0,
                            0,
                            0,
                            data[*pos],
                            data[*pos + 1],
                            data[*pos + 2],
                            data[*pos + 3],
                        ]);
                        sample_rate = f64::from_bits(bits);
                    }
                    *pos = elem_end;
                }
                0x9f => {
                    if *pos + 1 <= data.len() {
                        channels = data[*pos] as u64;
                    }
                    *pos = elem_end;
                }
                _ => {
                    *pos = elem_end;
                }
            }
        }

        self.tracks.push(WebmTrackInfo {
            track_number,
            codec_id,
            sample_rate,
            channels,
        });
    }

    fn parse_clusters(&mut self) {
        let data = self.buffer.clone();
        let mut pos = self.pos;
        let len = data.len();

        loop {
            if pos + 2 > len {
                break;
            }
            let element = Self::read_ebml_element(&data, &mut pos);
            if element.is_none() {
                break;
            }
            let (id, size) = element.unwrap();
            let elem_end = pos + size as usize;

            if elem_end > len {
                break;
            }

            if id == 0x1f43b675 {
                self.parse_cluster(&data, &mut pos, elem_end);
            }

            pos = elem_end;
        }

        self.pos = pos;
    }

    fn parse_cluster(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        // Read Cluster Timecode
        let saved = *pos;
        while *pos + 2 <= end && *pos < data.len() {
            let element = Self::read_ebml_element(data, pos);
            if element.is_none() {
                return;
            }
            let (id, size) = element.unwrap();
            let elem_end = *pos + size as usize;

            match id {
                0xe7 => {
                    if *pos + 4 <= data.len() {
                        self.cluster_time = u64::from_be_bytes([
                            0,
                            0,
                            0,
                            0,
                            data[*pos],
                            data[*pos + 1],
                            data[*pos + 2],
                            data[*pos + 3],
                        ]);
                    }
                    *pos = elem_end;
                }
                0xa3 => {
                    // SimpleBlock
                    self.parse_simple_block(data, pos, elem_end);
                }
                _ => {
                    *pos = elem_end;
                }
            }
        }
        *pos = saved;
        *pos = end;
    }

    fn parse_simple_block(&mut self, data: &[u8], pos: &mut usize, end: usize) {
        if let Some((_track_number, _)) = Self::read_varint(data, pos) {
            if *pos + 3 > end {
                return;
            }
            let timecode = i16::from_be_bytes([data[*pos], data[*pos + 1]]);
            *pos += 2;
            let flags = data[*pos];
            *pos += 1;
            let keyframe = (flags & 0x80) != 0;

            let absolute_time = (self.cluster_time as i64 + timecode as i64) as u64;
            let timestamp_ns = absolute_time * self.timecode_scale;

            if *pos < end {
                let packet_data = data[*pos..end].to_vec();
                self.packets.push_back(WebmPacket {
                    data: packet_data,
                    timestamp: timestamp_ns / 1000000,
                    duration: 0,
                    keyframe,
                });
            }
            *pos = end;
        }
    }

    pub fn tracks(&self) -> &[WebmTrackInfo] {
        &self.tracks
    }

    pub fn next_packet(&mut self) -> Option<WebmPacket> {
        self.packets.pop_front()
    }

    pub fn is_parsed(&self) -> bool {
        self.parsed
    }
}
