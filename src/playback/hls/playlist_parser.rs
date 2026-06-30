use crate::playback::hls::types::*;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;

lazy_static::lazy_static! {
    static ref EXTINF_RE: Regex = Regex::new(r#"#EXTINF:([^,]+),?(.*)"#).unwrap();
    static ref EXT_X_KEY_RE: Regex = Regex::new(r#"#EXT-X-KEY:METHOD=([^,]+)(?:,URI="([^"]+)")?(?:,IV=([^,]+))?"#).unwrap();
    static ref EXT_X_MAP_RE: Regex = Regex::new(r#"#EXT-X-MAP:URI="([^"]+)"(?:,BYTERANGE="([^"]+)")?"#).unwrap();
    static ref EXT_X_STREAM_INF_RE: Regex = Regex::new(r#"#EXT-X-STREAM-INF:(.+)"#).unwrap();
    static ref EXT_X_MEDIA_RE: Regex = Regex::new(r#"#EXT-X-MEDIA:TYPE=([^,]+),GROUP-ID="([^"]+)"(?:,NAME="([^"]+)")?(?:,DEFAULT=([^,]+))?(?:,AUTOSELECT=([^,]+))?(?:,URI="([^"]+)")?"#).unwrap();
    static ref EXT_X_TARGETDURATION_RE: Regex = Regex::new(r"#EXT-X-TARGETDURATION:(\d+)").unwrap();
    static ref EXT_X_MEDIA_SEQUENCE_RE: Regex = Regex::new(r"#EXT-X-MEDIA-SEQUENCE:(\d+)").unwrap();
    static ref EXT_X_DISCONTINUITY_RE: Regex = Regex::new(r"#EXT-X-DISCONTINUITY").unwrap();
    static ref EXT_X_ENDLIST_RE: Regex = Regex::new(r"#EXT-X-ENDLIST").unwrap();
    static ref EXT_X_VERSION_RE: Regex = Regex::new(r"#EXT-X-VERSION:(\d+)").unwrap();
    static ref EXT_X_PLAYLIST_TYPE_RE: Regex = Regex::new(r"#EXT-X-PLAYLIST-TYPE:(.+)").unwrap();
    static ref EXT_X_INDEPENDENT_SEGMENTS_RE: Regex = Regex::new(r"#EXT-X-INDEPENDENT-SEGMENTS").unwrap();
}

pub fn parse_playlist(content: &str, base_url: &str) -> Result<HLSPlaylist> {
    let mut playlist = HLSPlaylist {
        is_master: false,
        is_live: true,
        media_sequence: 0,
        target_duration: 6.0,
        segments: Vec::new(),
        variants: Vec::new(),
        audio_groups: HashMap::new(),
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut current_key: Option<HLSKey> = None;
    let mut current_map: Option<HLSMap> = None;
    let mut current_sequence: i64 = 0;
    let mut discontinuity = false;

    for (i, line) in lines.iter().enumerate() {
        let line = line.trim();

        if line.starts_with("#EXTM3U") {
            continue;
        }

        if let Some(_caps) = EXT_X_VERSION_RE.captures(line) {
            continue;
        }

        if let Some(caps) = EXT_X_TARGETDURATION_RE.captures(line) {
            playlist.target_duration = caps[1].parse().unwrap_or(6.0);
            continue;
        }

        if let Some(caps) = EXT_X_MEDIA_SEQUENCE_RE.captures(line) {
            playlist.media_sequence = caps[1].parse().unwrap_or(0);
            current_sequence = playlist.media_sequence;
            continue;
        }

        if let Some(caps) = EXT_X_PLAYLIST_TYPE_RE.captures(line) {
            let pl_type = caps[1].trim();
            playlist.is_live = pl_type != "VOD";
            continue;
        }

        if EXT_X_ENDLIST_RE.is_match(line) {
            playlist.is_live = false;
            continue;
        }

        if EXT_X_DISCONTINUITY_RE.is_match(line) {
            discontinuity = true;
            continue;
        }

        if EXT_X_INDEPENDENT_SEGMENTS_RE.is_match(line) {
            continue;
        }

        if let Some(caps) = EXT_X_KEY_RE.captures(line) {
            current_key = Some(HLSKey {
                method: caps[1].to_string(),
                uri: caps.get(2).map(|m| m.as_str().to_string()),
                iv: caps.get(3).map(|m| m.as_str().to_string()),
            });
            continue;
        }

        if let Some(caps) = EXT_X_MAP_RE.captures(line) {
            let uri = resolve_url(base_url, &caps[1]);
            current_map = Some(HLSMap {
                uri,
                byterange: caps.get(2).map(|m| m.as_str().to_string()),
            });
            continue;
        }

        if let Some(caps) = EXT_X_STREAM_INF_RE.captures(line) {
            let attrs = parse_attributes(&caps[1]);
            let bandwidth = attrs.get("BANDWIDTH").and_then(|v| v.parse().ok()).unwrap_or(0);
            let codecs = attrs.get("CODECS").map(|v| v.trim_matches('"').to_string());
            let resolution = attrs.get("RESOLUTION").map(|v| v.trim_matches('"').to_string());
            let audio = attrs.get("AUDIO").map(|v| v.trim_matches('"').to_string());

            let next_line = lines.get(i + 1).map(|s| s.trim());
            if let Some(url) = next_line {
                if !url.starts_with('#') && !url.is_empty() {
                    let variant_url = resolve_url(base_url, url);
                    playlist.variants.push(HLSVariant {
                        bandwidth,
                        codecs,
                        url: variant_url,
                        audio,
                        resolution,
                    });
                    playlist.is_master = true;
                }
            }
            continue;
        }

        if let Some(caps) = EXT_X_MEDIA_RE.captures(line) {
            let media_type = &caps[1];
            let group_id = &caps[2];
            let name = caps.get(3).map(|m| m.as_str().to_string());
            let default = caps.get(4).map(|m| m.as_str().to_string());
            let autoselect = caps.get(5).map(|m| m.as_str().to_string());
            let uri = caps.get(6).map(|m| resolve_url(base_url, m.as_str()));

            if media_type == "AUDIO" {
                let rendition = HLSAudioRendition {
                    default,
                    autoselect,
                    uri,
                    name,
                };
                playlist.audio_groups
                    .entry(group_id.to_string())
                    .or_default()
                    .push(rendition);
            }
            continue;
        }

        if let Some(caps) = EXTINF_RE.captures(line) {
            let duration = caps[1].parse().unwrap_or(0.0);
            let title = if caps[2].is_empty() { None } else { Some(caps[2].trim().to_string()) };

            let next_line = lines.get(i + 1).map(|s| s.trim());
            if let Some(url) = next_line {
                if !url.starts_with('#') && !url.is_empty() {
                    let segment_url = resolve_url(base_url, url);
                    let segment = HLSSegment {
                        url: segment_url,
                        duration,
                        sequence: current_sequence,
                        discontinuity,
                        key: current_key.clone(),
                        map: current_map.clone(),
                        title,
                    };
                    playlist.segments.push(segment);
                    current_sequence += 1;
                    discontinuity = false;
                }
            }
            continue;
        }
    }

    Ok(playlist)
}

fn parse_attributes(s: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let mut current_key = String::new();
    let mut current_value = String::new();
    let mut in_quotes = false;
    let mut reading_key = true;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '=' if !in_quotes => {
                reading_key = false;
            }
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                if !current_key.is_empty() {
                    attrs.insert(current_key.trim().to_string(), current_value.trim().to_string());
                }
                current_key.clear();
                current_value.clear();
                reading_key = true;
            }
            c if reading_key => {
                current_key.push(c);
            }
            c => {
                current_value.push(c);
            }
        }
    }

    if !current_key.is_empty() {
        attrs.insert(current_key.trim().to_string(), current_value.trim().to_string());
    }

    attrs
}

fn resolve_url(base_url: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }

    let base = base_url.rsplit_once('/').map(|(b, _)| b).unwrap_or("");
    if base.is_empty() {
        return relative.to_string();
    }

    if relative.starts_with('/') {
        if let Some(proto_end) = base.find("://") {
            let host_start = proto_end + 3;
            if let Some(path_start) = base[host_start..].find('/') {
                return format!("{}{}", &base[..host_start + path_start], relative);
            }
            return format!("{}{}", base, relative);
        }
        return format!("{}{}", base, relative);
    }

    format!("{}/{}", base, relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_master_playlist() {
        let content = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS="mp4a.40.2",RESOLUTION=1280x720
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=256000,CODECS="mp4a.40.2",RESOLUTION=1920x1080
high.m3u8
"#;
        let playlist = parse_playlist(content, "https://example.com/").unwrap();
        assert!(playlist.is_master);
        assert_eq!(playlist.variants.len(), 2);
        assert_eq!(playlist.variants[0].bandwidth, 128000);
        assert_eq!(playlist.variants[1].bandwidth, 256000);
    }

    #[test]
    fn test_parse_media_playlist() {
        let content = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:10.0,
segment0.ts
#EXTINF:10.0,
segment1.ts
#EXT-X-ENDLIST
"#;
        let playlist = parse_playlist(content, "https://example.com/").unwrap();
        assert!(!playlist.is_master);
        assert!(!playlist.is_live);
        assert_eq!(playlist.segments.len(), 2);
        assert_eq!(playlist.segments[0].url, "https://example.com/segment0.ts");
    }
}