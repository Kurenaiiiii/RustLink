use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FALLBACK_TITLE: &str = "Unknown Title";
pub const FALLBACK_AUTHOR: &str = "Unknown Artist";

/// Patterns for extracting info from YouTube URLs
pub struct UrlPatterns;
impl UrlPatterns {
    pub const VIDEO: &'static str = r"^https?://(?:music\.)?(?:www\.)?youtube\.com/watch\?v=[\w-]+";
    pub const PLAYLIST: &'static str = r"^https?://(?:music\.)?(?:www\.)?youtube\.com/playlist\?list=[\w-]+";
    pub const SHORT: &'static str = r"^https?://youtu\.be/[\w-]+";
    pub const SHORTS: &'static str = r"^https?://(?:www\.)?youtube\.com/shorts/[\w-]+";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedAtInfo {
    pub original: String,
    pub timestamp: u64,
    pub date: String,
    pub readable: String,
    pub compact: String,
    pub ago: TimeUnits,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimeUnits {
    pub years: u64,
    pub months: u64,
    pub weeks: u64,
    pub days: u64,
    pub hours: u64,
    pub minutes: u64,
    pub seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoQuality {
    pub quality: String,
    pub bitrate: u64,
    pub fps: Option<u64>,
    pub mime_type: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub codec: String,
    pub itag: u64,
    pub container: Option<String>,
    pub average_bitrate: Option<u64>,
    pub content_length: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormat {
    pub itag: u64,
    pub mime_type: String,
    pub bitrate: u64,
    pub average_bitrate: Option<u64>,
    pub audio_quality: Option<String>,
    pub audio_sample_rate: Option<String>,
    pub audio_channels: Option<u64>,
    pub codec: String,
    pub container: Option<String>,
    pub content_length: Option<serde_json::Value>,
    pub loudness_db: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTrack {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_auto_dubbed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionTrack {
    pub language_code: String,
    pub name: Option<String>,
    pub is_translatable: bool,
    pub base_url: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalLinks {
    pub spotify: Option<String>,
    pub apple_music: Option<String>,
    pub soundcloud: Option<String>,
    pub bandcamp: Option<String>,
    pub deezer: Option<String>,
    pub tidal: Option<String>,
    pub amazon_music: Option<String>,
    pub youtube_music: Option<String>,
    pub website: Option<String>,
    pub other: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct YouTubeChannelInfo {
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub subscribers: Option<SubscriberInfo>,
    pub video_count: Option<VideoCountInfo>,
    pub verified: bool,
    pub description: Option<String>,
    pub links: Vec<String>,
    pub featured_video: Option<FeaturedVideo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriberInfo {
    pub original: String,
    pub count: Option<u64>,
    pub formatted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoCountInfo {
    pub original: String,
    pub count: Option<u64>,
    pub formatted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturedVideo {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

pub fn format_duration(ms: i64) -> DurationParts {
    if ms <= 0 {
        return DurationParts { ms: 0, formatted: "🔴 LIVE".into(), hms: "🔴 LIVE".into() };
    }
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    let formatted = if hours > 0 {
        format!("{hours}:{:02}:{:02}", minutes, secs)
    } else {
        format!("{minutes}:{:02}", secs)
    };
    let hms = format!("{hours}h {:02}m {:02}s", minutes, secs);
    DurationParts { ms, formatted, hms }
}

pub struct DurationParts {
    pub ms: i64,
    pub formatted: String,
    pub hms: String,
}

pub fn format_number(num: f64) -> String {
    if !num.is_finite() { return "0".into(); }
    if num >= 1_000_000_000.0 { format!("{:.1}B", num / 1_000_000_000.0) }
    else if num >= 1_000_000.0 { format!("{:.1}M", num / 1_000_000.0) }
    else if num >= 1_000.0 { format!("{:.1}K", num / 1_000.0) }
    else { format!("{}", num as u64) }
}

fn time_unit_multipliers() -> [(u64, u64); 7] {
    [
        (36525, 864_000_00),  // year (365.25 days)
        (3044, 864_000_00),   // month (30.44 days)
        (7, 864_000_00),      // week
        (1, 864_000_00),      // day
        (1, 3_600_000),       // hour
        (1, 60_000),          // minute
        (1, 1_000),           // second
    ]
}

fn parse_iso_date(text: &str) -> Option<u64> {
    // Simple ISO 8601 parsing (YYYY-MM-DDTHH:MM:SS format)
    let re = regex::Regex::new(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})").ok()?;
    let caps = re.captures(text)?;
    let year: u64 = caps[1].parse().ok()?;
    let month: u64 = caps[2].parse().ok()?;
    let day: u64 = caps[3].parse().ok()?;
    let hour: u64 = caps[4].parse().ok()?;
    let min: u64 = caps[5].parse().ok()?;
    let sec: u64 = caps[6].parse().ok()?;
    // Naive approximation: days since epoch + time
    let days_since_epoch = (year.saturating_sub(1970)) * 365
        + (year.saturating_sub(1970) + 3) / 4 // leap years approx
        + (month as u64) * 30 // approximate
        + day as u64;
    let total_secs = days_since_epoch * 86400 + hour * 3600 + min * 60 + sec;
    Some(total_secs * 1000)
}

pub fn parse_published_at(text: &str) -> Option<PublishedAtInfo> {
    if text.is_empty() { return None; }
    fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
    }
    // Try parsing as ISO date (simple approach)
    if let Some(timestamp) = parse_iso_date(text) {
        return Some(build_published_at(timestamp, text));
    }
    // Try parsing relative time like "2 years ago"
    let lower = text.to_lowercase();
    let re = regex::Regex::new(r"(\d+)\s*(year|month|week|day|hour|minute|second)").ok()?;
    let mut units = TimeUnits::default();
    for cap in re.captures_iter(&lower) {
        let val: u64 = cap[1].parse().ok()?;
        match &cap[2] {
            s if s.starts_with("year") => units.years = val,
            s if s.starts_with("month") => units.months = val,
            s if s.starts_with("week") => units.weeks = val,
            s if s.starts_with("day") => units.days = val,
            s if s.starts_with("hour") => units.hours = val,
            s if s.starts_with("minute") => units.minutes = val,
            s if s.starts_with("second") => units.seconds = val,
            _ => {}
        }
    }
    let ms_ago = units.years * 36525 * 864_000_00
        + units.months * 3044 * 864_000_00 / 100
        + units.weeks * 7 * 864_000_00
        + units.days * 864_000_00
        + units.hours * 3_600_000
        + units.minutes * 60_000
        + units.seconds * 1_000;
    let timestamp = now_ms().saturating_sub(ms_ago);
    Some(build_published_at(timestamp, text))
}

fn build_published_at(timestamp: u64, original: &str) -> PublishedAtInfo {
    fn readable(units: &TimeUnits) -> String {
        if units.years > 0 { format!("{} year{} ago", units.years, if units.years > 1 { "s" } else { "" }) }
        else if units.months > 0 { format!("{} month{} ago", units.months, if units.months > 1 { "s" } else { "" }) }
        else if units.weeks > 0 { format!("{} week{} ago", units.weeks, if units.weeks > 1 { "s" } else { "" }) }
        else if units.days > 0 { format!("{} day{} ago", units.days, if units.days > 1 { "s" } else { "" }) }
        else if units.hours > 0 { format!("{} hour{} ago", units.hours, if units.hours > 1 { "s" } else { "" }) }
        else if units.minutes > 0 { format!("{} minute{} ago", units.minutes, if units.minutes > 1 { "s" } else { "" }) }
        else if units.seconds > 0 { format!("{} second{} ago", units.seconds, if units.seconds > 1 { "s" } else { "" }) }
        else { "just now".into() }
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let ts = timestamp as i64;
    let diff = (now - ts).max(0) as u64;
    let units = TimeUnits {
        years: diff / (36525 * 864_000_00 / 100),
        months: diff / (3044 * 864_000_00 / 100) % 12,
        weeks: diff / (7 * 864_000_00) % 4,
        days: diff / 864_000_00 % 7,
        hours: diff / 3_600_000 % 24,
        minutes: diff / 60_000 % 60,
        seconds: diff / 1_000 % 60,
    };
    PublishedAtInfo {
        original: original.to_string(),
        timestamp,
        date: "1970-01-01T00:00:00Z".into(), // simplified
        readable: readable(&units),
        compact: format!("{}y {}mo {}w {}d {}h {}m {}s", units.years, units.months, units.weeks, units.days, units.hours, units.minutes, units.seconds),
        ago: units,
    }
}

pub fn extract_external_links(description: &str) -> Option<ExternalLinks> {
    if description.is_empty() { return None; }
    let url_re = regex::Regex::new(r"https?://[^\s]+").ok()?;
    let mut links = ExternalLinks::default();
    for m in url_re.find_iter(description) {
        let url = m.as_str().trim_end_matches(|c: char| ",;)".contains(c));
        if url.contains("spotify.com") || url.contains("open.spotify.com") {
            links.spotify = Some(url.to_string());
        } else if url.contains("apple.com") || url.contains("music.apple.com") {
            links.apple_music = Some(url.to_string());
        } else if url.contains("soundcloud.com") {
            links.soundcloud = Some(url.to_string());
        } else if url.contains("bandcamp.com") {
            links.bandcamp = Some(url.to_string());
        } else if url.contains("deezer.com") {
            links.deezer = Some(url.to_string());
        } else if url.contains("tidal.com") {
            links.tidal = Some(url.to_string());
        } else if url.contains("amazon.com/music") || url.contains("music.amazon") {
            links.amazon_music = Some(url.to_string());
        } else if url.contains("music.youtube.com") {
            links.youtube_music = Some(url.to_string());
        } else if !url.contains("youtube.com") && !url.contains("youtu.be") {
            if links.website.is_none() && (url.contains(".com") || url.contains(".net") || url.contains(".org") || url.contains(".io")) {
                links.website = Some(url.to_string());
            } else {
                links.other.get_or_insert_with(Vec::new).push(url.to_string());
            }
        }
    }
    if links.other.as_ref().map_or(true, |v| v.is_empty()) { links.other = None; }
    let has_links = links.spotify.is_some() || links.apple_music.is_some() || links.soundcloud.is_some()
        || links.bandcamp.is_some() || links.deezer.is_some() || links.tidal.is_some()
        || links.amazon_music.is_some() || links.youtube_music.is_some() || links.website.is_some()
        || links.other.is_some();
    if has_links { Some(links) } else { None }
}

pub fn extract_video_qualities(streaming_data: &serde_json::Value) -> Vec<VideoQuality> {
    let mut qualities = Vec::new();
    let all = streaming_data["formats"].as_array().into_iter()
        .chain(streaming_data["adaptiveFormats"].as_array().into_iter())
        .flatten();
    for fmt in all {
        let quality_label = fmt["qualityLabel"].as_str().unwrap_or("");
        let bitrate = fmt["bitrate"].as_u64().unwrap_or(0);
        let mime = fmt["mimeType"].as_str().unwrap_or("");
        if quality_label.is_empty() || bitrate == 0 || !mime.starts_with("video/") { continue; }
        let codec = mime.split("codecs=\"").nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("unknown")
            .split('.')
            .next()
            .unwrap_or("unknown")
            .to_string();
        qualities.push(VideoQuality {
            quality: quality_label.to_string(),
            bitrate,
            fps: fmt["fps"].as_u64(),
            mime_type: Some(mime.to_string()),
            width: fmt["width"].as_u64(),
            height: fmt["height"].as_u64(),
            codec,
            itag: fmt["itag"].as_u64().unwrap_or(0),
            container: mime.split(';').next().and_then(|s| s.split('/').nth(1)).map(|s| s.to_string()),
            average_bitrate: fmt["averageBitrate"].as_u64(),
            content_length: fmt.get("contentLength").cloned(),
        });
    }
    qualities
}

pub fn extract_audio_formats(streaming_data: &serde_json::Value) -> Vec<AudioFormat> {
    let mut formats = Vec::new();
    let all = streaming_data["formats"].as_array().into_iter()
        .chain(streaming_data["adaptiveFormats"].as_array().into_iter())
        .flatten();
    for fmt in all {
        let mime = fmt["mimeType"].as_str().unwrap_or("");
        let bitrate = fmt["bitrate"].as_u64().unwrap_or(0);
        if !mime.starts_with("audio/") || bitrate == 0 { continue; }
        let codec = mime.split("codecs=\"").nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("unknown")
            .to_string();
        formats.push(AudioFormat {
            itag: fmt["itag"].as_u64().unwrap_or(0),
            mime_type: mime.to_string(),
            bitrate,
            average_bitrate: fmt["averageBitrate"].as_u64(),
            audio_quality: fmt["audioQuality"].as_str().map(|s| s.to_string()),
            audio_sample_rate: fmt["audioSampleRate"].as_str().map(|s| s.to_string()),
            audio_channels: fmt["audioChannels"].as_u64(),
            codec,
            container: mime.split(';').next().and_then(|s| s.split('/').nth(1)).map(|s| s.to_string()),
            content_length: fmt.get("contentLength").cloned(),
            loudness_db: fmt["loudnessDb"].as_f64(),
        });
    }
    formats
}

pub fn extract_audio_tracks(streaming_data: &serde_json::Value) -> Vec<AudioTrack> {
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let all = streaming_data["formats"].as_array().into_iter()
        .chain(streaming_data["adaptiveFormats"].as_array().into_iter())
        .flatten();
    for fmt in all {
        let track = &fmt["audioTrack"];
        if track.is_null() { continue; }
        let id = track["id"].as_str().unwrap_or("");
        if id.is_empty() || !seen.insert(id.to_string()) { continue; }
        tracks.push(AudioTrack {
            id: id.to_string(),
            name: track["displayName"].as_str().unwrap_or("").to_string(),
            is_default: track["audioIsDefault"].as_bool().unwrap_or(false),
            is_auto_dubbed: track["isAutoDubbed"].as_bool().unwrap_or(false),
        });
    }
    tracks
}

pub fn extract_captions(data: &serde_json::Value) -> Option<Vec<CaptionTrack>> {
    let captions_root = data["captions"]["playerCaptionsTracklistRenderer"].as_object()?;
    let caption_tracks = captions_root["captionTracks"].as_array()?;
    let tracks: Vec<CaptionTrack> = caption_tracks.iter().map(|c| CaptionTrack {
        language_code: c["languageCode"].as_str().unwrap_or("").to_string(),
        name: c["name"]["simpleText"].as_str().map(|s| s.to_string()),
        is_translatable: c["isTranslatable"].as_bool().unwrap_or(false),
        base_url: c["baseUrl"].as_str().unwrap_or("").to_string(),
        kind: c["kind"].as_str().map(|s| s.to_string()),
    }).collect();
    if tracks.is_empty() { None } else { Some(tracks) }
}

pub fn extract_title(renderer: &serde_json::Value, full_response: Option<&serde_json::Value>) -> Option<String> {
    if let Some(resp) = full_response {
        if let Some(vd) = resp["videoDetails"].as_object() {
            if let Some(t) = vd["title"].as_str() {
                if t != "undefined" { return Some(t.to_string()); }
            }
        }
        if let Some(mf) = resp["microformat"]["playerMicroformatRenderer"].as_object() {
            if let Some(t) = mf["title"]["simpleText"].as_str() {
                if t != "undefined" { return Some(t.to_string()); }
            }
            if let Some(runs) = mf["title"]["runs"].as_array() {
                let t: String = runs.iter().filter_map(|r| r["text"].as_str()).collect();
                if !t.is_empty() { return Some(t); }
            }
        }
    }
    if let Some(t) = renderer["title"].as_str() {
        if t != "undefined" { return Some(t.to_string()); }
    }
    if let Some(runs) = renderer["title"]["runs"].as_array() {
        let t: String = runs.iter().filter_map(|r| r["text"].as_str()).collect();
        if !t.is_empty() { return Some(t); }
    }
    if let Some(t) = renderer["title"]["simpleText"].as_str() {
        return Some(t.to_string());
    }
    None
}

pub fn extract_author(renderer: &serde_json::Value, full_response: Option<&serde_json::Value>) -> Option<String> {
    if let Some(resp) = full_response {
        if let Some(vd) = resp["videoDetails"].as_object() {
            if let Some(a) = vd["author"].as_str() {
                if a != "undefined" { return Some(a.to_string()); }
            }
        }
        if let Some(mf) = resp["microformat"]["playerMicroformatRenderer"].as_object() {
            if let Some(name) = mf["ownerChannelName"].as_str() {
                return Some(name.to_string());
            }
        }
    }
    if let Some(a) = renderer["author"].as_str() {
        if a != "undefined" { return Some(a.to_string()); }
    }
    for path in &["longBylineText", "shortBylineText", "ownerText"] {
        if let Some(runs) = renderer[path]["runs"].as_array() {
            let a: String = runs.iter().filter_map(|r| r["text"].as_str()).collect();
            if !a.is_empty() { return Some(a); }
        }
    }
    None
}

pub fn extract_thumbnail(renderer: &serde_json::Value, video_id: Option<&str>) -> Option<String> {
    let thumbs = renderer["thumbnail"]["thumbnails"].as_array()
        .or_else(|| renderer["thumbnail"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"].as_array());
    if let Some(t) = thumbs {
        if let Some(last) = t.last() {
            if let Some(url) = last["url"].as_str() {
                return Some(url.split('?').next().unwrap_or(url).to_string());
            }
        }
    }
    video_id.map(|id| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
}
