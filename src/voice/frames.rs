use std::collections::HashSet;

pub const VOICE_FRAME_OPS: VoiceFrameOps = VoiceFrameOps {
    start: 1,
    stop: 2,
    data: 3,
};

pub struct VoiceFrameOps {
    pub start: u8,
    pub stop: u8,
    pub data: u8,
}

pub const VOICE_FORMATS: VoiceFormats = VoiceFormats {
    opus: 0,
    ogg: 1,
    pcm_s16le: 2,
};

pub struct VoiceFormats {
    pub opus: u8,
    pub ogg: u8,
    pub pcm_s16le: u8,
}

#[derive(Debug, Clone)]
pub struct ResolvedVoiceFormat {
    pub name: String,
    pub code: u8,
}

#[derive(Debug, Clone)]
pub struct ParsedVoiceFrameHeader {
    pub op: u8,
    pub format: u8,
    pub guild_id: String,
    pub user_id: String,
    pub ssrc: u32,
    pub timestamp: u32,
    pub payload_offset: usize,
}

lazy_static::lazy_static! {
    static ref SUPPORTED_FORMATS: HashSet<&'static str> = {
        let mut set = HashSet::new();
        set.insert("opus");
        set.insert("pcm_s16le");
        set
    };
}

fn voice_format_code(name: &str) -> u8 {
    match name {
        "opus" => 0,
        "ogg" => 1,
        "pcm_s16le" => 2,
        _ => 0,
    }
}

pub fn resolve_voice_format(
    format: Option<&str>,
) -> ResolvedVoiceFormat {
    let normalized = format.unwrap_or("opus").to_lowercase();
    if SUPPORTED_FORMATS.contains(normalized.as_str()) {
        return ResolvedVoiceFormat {
            name: normalized.clone(),
            code: voice_format_code(&normalized),
        };
    }
    ResolvedVoiceFormat {
        name: "opus".into(),
        code: 0,
    }
}

pub fn build_voice_frame(
    op: u8,
    format_code: u8,
    guild_id: &str,
    user_id: &str,
    ssrc: u32,
    timestamp: u32,
    payload: Option<&[u8]>,
) -> Vec<u8> {
    let guild_bytes = guild_id.as_bytes();
    let user_bytes = user_id.as_bytes();

    if guild_bytes.len() > 255 || user_bytes.len() > 255 {
        panic!("Voice frame id too long.");
    }

    let payload_data = payload.unwrap_or(&[]);
    let total_length = 1 + 1 + 1 + guild_bytes.len() + 1 + user_bytes.len() + 4 + 4 + payload_data.len();

    let mut buf = Vec::with_capacity(total_length);
    buf.push(op);
    buf.push(format_code);
    buf.push(guild_bytes.len() as u8);
    buf.extend_from_slice(guild_bytes);
    buf.push(user_bytes.len() as u8);
    buf.extend_from_slice(user_bytes);
    buf.extend_from_slice(&ssrc.to_be_bytes());
    buf.extend_from_slice(&timestamp.to_be_bytes());
    if !payload_data.is_empty() {
        buf.extend_from_slice(payload_data);
    }
    buf
}

pub fn parse_voice_frame_header(buf: &[u8]) -> Option<ParsedVoiceFrameHeader> {
    if buf.len() < 8 {
        return None;
    }

    let mut offset = 0;
    let op = buf[offset];
    offset += 1;
    let format = buf[offset];
    offset += 1;

    if offset >= buf.len() {
        return None;
    }
    let guild_len = buf[offset] as usize;
    offset += 1;
    if offset + guild_len > buf.len() {
        return None;
    }
    let guild_id = String::from_utf8_lossy(&buf[offset..offset + guild_len]).to_string();
    offset += guild_len;

    if offset >= buf.len() {
        return None;
    }
    let user_len = buf[offset] as usize;
    offset += 1;
    if offset + user_len > buf.len() {
        return None;
    }
    let user_id = String::from_utf8_lossy(&buf[offset..offset + user_len]).to_string();
    offset += user_len;

    if offset + 8 > buf.len() {
        return None;
    }
    let ssrc = u32::from_be_bytes(buf[offset..offset + 4].try_into().ok()?);
    offset += 4;
    let timestamp = u32::from_be_bytes(buf[offset..offset + 4].try_into().ok()?);
    offset += 4;

    Some(ParsedVoiceFrameHeader {
        op,
        format,
        guild_id,
        user_id,
        ssrc,
        timestamp,
        payload_offset: offset,
    })
}
