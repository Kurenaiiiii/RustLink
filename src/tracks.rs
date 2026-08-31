use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub title: String,
    pub author: String,
    pub length: i64,
    pub identifier: String,
    pub is_seekable: bool,
    pub is_stream: bool,
    pub uri: Option<String>,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
    pub source_name: String,
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<Chapter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub title: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackData {
    pub encoded: Option<String>,
    pub info: TrackInfo,
    #[serde(default)]
    pub plugin_info: serde_json::Value,
    #[serde(default)]
    pub user_data: serde_json::Value,
    #[serde(default)]
    pub details: Vec<Option<String>>,
    #[serde(default)]
    pub message_flags: i32,
}

struct BufWriter {
    chunks: Vec<Vec<u8>>,
}

impl BufWriter {
    fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    fn write_byte(&mut self, v: u8) {
        self.chunks.push(vec![v]);
    }

    fn write_i64(&mut self, v: i64) {
        self.chunks.push(v.to_be_bytes().to_vec());
    }

    fn write_u16(&mut self, v: u16) {
        self.chunks.push(v.to_be_bytes().to_vec());
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.chunks.push(bytes.to_vec());
    }

    fn into_vec(self) -> Vec<u8> {
        let mut out = Vec::new();
        for c in self.chunks {
            out.extend_from_slice(&c);
        }
        out
    }
}

fn encode_modified_utf8(value: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for ch in value.chars() {
        let code = ch as u32;
        if code >= 0x0001 && code <= 0x007F {
            bytes.push(code as u8);
        } else if code == 0x0000 || (code >= 0x0080 && code <= 0x07FF) {
            bytes.push(0xC0 | ((code >> 6) as u8 & 0x1F));
            bytes.push(0x80 | (code as u8 & 0x3F));
        } else if code >= 0x0800 && code <= 0xFFFF {
            bytes.push(0xE0 | ((code >> 12) as u8 & 0x0F));
            bytes.push(0x80 | ((code >> 6) as u8 & 0x3F));
            bytes.push(0x80 | (code as u8 & 0x3F));
        }
    }
    bytes
}

fn write_utf(writer: &mut BufWriter, value: &str) {
    let encoded = encode_modified_utf8(value);
    if encoded.len() > 65535 {
        panic!("Encode Error: UTF string too long");
    }
    writer.write_u16(encoded.len() as u16);
    writer.write_bytes(&encoded);
}

fn write_nullable_text(writer: &mut BufWriter, value: Option<&str>) {
    match value {
        None | Some("") => {
            writer.write_byte(0);
        }
        Some(s) => {
            writer.write_byte(1);
            write_utf(writer, s);
        }
    }
}

struct BufReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BufReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn ensure(&self, n: usize) {
        if self.pos + n > self.buf.len() {
            panic!("Unexpected end of buffer (need {} bytes)", n);
        }
    }

    fn read_byte(&mut self) -> u8 {
        self.ensure(1);
        let v = self.buf[self.pos];
        self.pos += 1;
        v
    }

    fn read_u16(&mut self) -> u16 {
        self.ensure(2);
        let v = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        v
    }

    fn read_u32(&mut self) -> u32 {
        self.ensure(4);
        let v = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }

    fn read_i64(&mut self) -> i64 {
        self.ensure(8);
        let v = i64::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
            self.buf[self.pos + 4],
            self.buf[self.pos + 5],
            self.buf[self.pos + 6],
            self.buf[self.pos + 7],
        ]);
        self.pos += 8;
        v
    }

    fn read_modified_utf8(&mut self) -> String {
        let utflen = self.read_u16() as usize;
        self.ensure(utflen);
        let end = self.pos + utflen;
        let mut chars: Vec<char> = Vec::new();
        let mut i = self.pos;
        while i < end {
            let c = self.buf[i];
            i += 1;
            if c < 0x80 {
                chars.push(char::from(c));
            } else if (c & 0xE0) == 0xC0 {
                if i >= end {
                    panic!("Malformed utf");
                }
                let c2 = self.buf[i];
                i += 1;
                if (c2 & 0xC0) != 0x80 {
                    panic!("Malformed utf");
                }
                let ch = (((c & 0x1F) as u32) << 6) | ((c2 & 0x3F) as u32);
                chars.push(char::from_u32(ch).unwrap());
            } else if (c & 0xF0) == 0xE0 {
                if i + 1 >= end {
                    panic!("Malformed utf");
                }
                let c2 = self.buf[i];
                let c3 = self.buf[i + 1];
                i += 2;
                if (c2 & 0xC0) != 0x80 || (c3 & 0xC0) != 0x80 {
                    panic!("Malformed utf");
                }
                let ch = (((c & 0x0F) as u32) << 12)
                    | (((c2 & 0x3F) as u32) << 6)
                    | ((c3 & 0x3F) as u32);
                chars.push(char::from_u32(ch).unwrap());
            } else {
                panic!("Malformed utf");
            }
        }
        self.pos = end;
        chars.into_iter().collect()
    }

    fn read_nullable_text(&mut self) -> Option<String> {
        let present = self.read_byte() != 0;
        if present {
            Some(self.read_modified_utf8())
        } else {
            None
        }
    }
}

fn try_parse_legacy_seekable_trailer(buf: &[u8]) -> bool {
    let mut p = 0usize;
    if buf.len() < 1 {
        return false;
    }
    let present = buf[p] != 0;
    if !present {
        return false;
    }
    p += 1;
    if p + 2 > buf.len() {
        return false;
    }
    let utflen = u16::from_be_bytes([buf[p], buf[p + 1]]) as usize;
    p += 2;
    if p + utflen > buf.len() {
        return false;
    }
    if p + utflen != buf.len() {
        return false;
    }
    let s = std::str::from_utf8(&buf[p..p + utflen]).ok();
    s == Some("NLK:seekableY") || s == Some("NLK:seekableN")
}

pub fn encode_track(input: &TrackData) -> String {
    encode_track_info(&input.info, &input.details)
}

pub fn encode_track_info(input: &TrackInfo, details: &[Option<String>]) -> String {
    let mut writer = BufWriter::new();

    let has_uri = input.uri.as_deref().filter(|s| !s.is_empty());
    let has_artwork = input.artwork_url.as_deref().filter(|s| !s.is_empty());
    let has_isrc = input.isrc.as_deref().filter(|s| !s.is_empty());

    let version: u8 = if has_artwork.is_some() || has_isrc.is_some() {
        3
    } else if has_uri.is_some() {
        2
    } else {
        1
    };
    let flags = 1u8;

    writer.write_byte(version);
    write_utf(&mut writer, &input.title);
    write_utf(&mut writer, &input.author);
    writer.write_i64(input.length);
    write_utf(&mut writer, &input.identifier);
    writer.write_byte(if input.is_stream { 1 } else { 0 });

    if version >= 2 {
        write_nullable_text(&mut writer, has_uri);
    }
    if version >= 3 {
        write_nullable_text(&mut writer, has_artwork);
        write_nullable_text(&mut writer, has_isrc);
    }

    write_utf(&mut writer, &input.source_name);

    for detail in details {
        match detail {
            Some(s) => write_nullable_text(&mut writer, Some(s)),
            None => write_nullable_text(&mut writer, None),
        }
    }

    writer.write_i64(input.position);

    let message_buf = writer.into_vec();
    let header =
        ((message_buf.len() as u32) & 0x3FFFFFFF) | ((flags as u32 & 0x3) << 30);

    let mut out = Vec::with_capacity(4 + message_buf.len());
    out.extend_from_slice(&header.to_be_bytes());
    out.extend_from_slice(&message_buf);

    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&out)
}

pub fn decode_track(encoded: &str) -> Result<TrackData, String> {
    if encoded.is_empty() {
        return Err("Decode Error: Input string is null or empty".to_string());
    }

    use base64::Engine;
    let buffer = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("Decode Error: Invalid base64: {e}"))?;

    let mut reader = BufReader::new(&buffer);

    let header = reader.read_u32();
    let _flags = ((header >> 30) & 0x3) as i32;
    let message_size = (header & 0x3FFFFFFF) as usize;

    if message_size == 0 {
        return Err("Decode Error: message size: 0".to_string());
    }

    let message_end = reader.pos + message_size;
    if message_end > buffer.len() {
        return Err(format!(
            "Decode Error: message size {} exceeds buffer length {}",
            message_size,
            buffer.len()
        ));
    }

    let mut message_buf = &buffer[reader.pos..message_end];

    let tail_try_max = std::cmp::min(message_buf.len(), 512);
    for cut in 1..=tail_try_max {
        let tail = &message_buf[message_buf.len() - cut..];
        if try_parse_legacy_seekable_trailer(tail) {
            message_buf = &message_buf[..message_buf.len() - cut];
            break;
        }
    }

    let mut reader = BufReader::new(message_buf);

    let version = reader.read_byte();
    let title = reader.read_modified_utf8();
    let author = reader.read_modified_utf8();
    let length = reader.read_i64();
    let identifier = reader.read_modified_utf8();
    let is_stream = reader.read_byte() != 0;

    let uri = if version >= 2 {
        reader.read_nullable_text()
    } else {
        None
    };

    let artwork_url = if version >= 3 {
        reader.read_nullable_text()
    } else {
        None
    };

    let isrc = if version >= 3 {
        reader.read_nullable_text()
    } else {
        None
    };

    let source_name = reader.read_modified_utf8();

    let position_offset = message_buf.len() - 8;

    let details_buf = &message_buf[reader.pos..position_offset];
    let track_position = i64::from_be_bytes([
        message_buf[position_offset],
        message_buf[position_offset + 1],
        message_buf[position_offset + 2],
        message_buf[position_offset + 3],
        message_buf[position_offset + 4],
        message_buf[position_offset + 5],
        message_buf[position_offset + 6],
        message_buf[position_offset + 7],
    ]);

    let mut details: Vec<Option<String>> = Vec::new();
    if !details_buf.is_empty() {
        let mut d_reader = BufReader::new(details_buf);
        while d_reader.pos < details_buf.len() {
            match d_reader.read_nullable_text() {
                Some(s) => details.push(Some(s)),
                None => details.push(None),
            }
        }
    }

    Ok(TrackData {
        encoded: Some(encoded.to_string()),
        info: TrackInfo {
            title,
            author,
            length,
            identifier,
            is_seekable: !is_stream,
            is_stream,
            uri,
            artwork_url,
            isrc,
            source_name,
            position: track_position,
            chapters: None,
        },
        details,
        plugin_info: serde_json::Value::Object(serde_json::Map::new()),
        user_data: serde_json::Value::Object(serde_json::Map::new()),
        message_flags: 0,
    })
}
