use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::sources::{PlaylistData, PlaylistInfo, SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const API_KEY: &str = "AIzaSyDVQw45DwoYh632gvsP5vPDqEKvb-Ywnb8";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

pub struct GoogleDriveSource {
    client: Client,
}

impl GoogleDriveSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn make_sid_auth_header(cookies: &str, origin: &str) -> Option<String> {
        let mut sapisid = None;
        for part in cookies.split(';') {
            let trimmed = part.trim();
            if let Some((name, value)) = trimmed.split_once('=') {
                let name = name.trim();
                let value = value.trim();
                if name == "SAPISID" || name == "__Secure-3PAPISID" {
                    sapisid = Some(value);
                }
            }
        }
        let sid = sapisid?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let data = format!("{} {} {}", timestamp, sid, origin);
        let mut hasher = Sha1::new();
        hasher.update(data.as_bytes());
        let hash = hex::encode(hasher.finalize());
        Some(format!("SAPISIDHASH {}_{}", timestamp, hash))
    }

    async fn get_cookies_and_auth(
        &self,
        url: &str,
    ) -> (String, Option<String>) {
        let resp = self
            .client
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await;

        let cookies_str = match resp {
            Ok(r) => {
                let set_cookie = r.headers().get_all("set-cookie");
                let cookies: Vec<&str> = set_cookie
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .map(|c| c.split(';').next().unwrap_or(""))
                    .collect();
                cookies.join("; ")
            }
            Err(_) => String::new(),
        };

        let auth = Self::make_sid_auth_header(&cookies_str, "https://drive.google.com");
        (cookies_str, auth)
    }

    fn normalize_drive_title(title: &str) -> String {
        let normalized = title.replace(" - Google Drive", "").trim().to_string();
        if normalized.is_empty() || normalized.to_lowercase().contains("virus scan warning") {
            String::new()
        } else {
            normalized
        }
    }

    fn decode_html_entities(text: &str) -> String {
        text.replace("&#39;", "'")
            .replace("&quot;", "\"")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .trim()
            .to_string()
    }

    fn get_format(_mime: Option<&str>, title: Option<&str>) -> String {
        if let Some(mime) = _mime {
            if mime.contains("audio/mpeg") {
                return "mp3".into();
            }
            if let Some(ext) = mime.split('/').nth(1) {
                if !ext.is_empty() {
                    return ext.to_lowercase();
                }
            }
        }
        if let Some(title) = title {
            if let Some(ext) = title.rsplit('.').next() {
                if ext.len() <= 5 {
                    return ext.to_lowercase();
                }
            }
        }
        "mp3".into()
    }

    async fn get_file_info(&self, file_id: &str, url: &str) -> Option<TrackInfo> {
        let origin = "https://drive.google.com";
        let (cookie_header, auth_header) = self.get_cookies_and_auth(url).await;

        let api_url = format!(
            "https://content-workspacevideo-pa.googleapis.com/v1/drive/media/{}/playback?key={}",
            file_id, API_KEY
        );

        let mut req = self
            .client
            .get(&api_url)
            .header("Referer", "https://drive.google.com/")
            .header("Origin", origin)
            .header("User-Agent", USER_AGENT);

        if let Some(ref auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        if !cookie_header.is_empty() {
            req = req.header("Cookie", &cookie_header);
        }

        let mut title = "Unknown Drive File".to_string();
        let mut duration_ms = 0i64;

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or_default();
                if let Some(meta) = data.get("mediaMetadata") {
                    if let Some(t) = meta.get("title").and_then(|v| v.as_str()) {
                        let clean = Self::normalize_drive_title(t);
                        if !clean.is_empty() {
                            title = clean;
                        }
                    }
                    if let Some(d) = meta.get("duration").and_then(|v| v.as_str()) {
                        if let Ok(secs) = d.parse::<f64>() {
                            duration_ms = (secs * 1000.0) as i64;
                        }
                    }
                }
            }
            _ => {}
        }

        if title == "Unknown Drive File" {
            let probe_url = format!(
                "https://drive.google.com/uc?id={}&export=download&authuser=0",
                file_id
            );
            let probe_resp = self
                .client
                .get(&probe_url)
                .header("User-Agent", USER_AGENT)
                .header("Referer", "https://drive.google.com/")
                .send()
                .await;

            if let Ok(resp) = probe_resp {
                if let Some(disposition) = resp.headers().get("content-disposition") {
                    if let Ok(d) = disposition.to_str() {
                        if let Some(name) = Self::extract_filename(d) {
                            title = name;
                        }
                    }
                }
            }
        }

        if title == "Unknown Drive File" {
            let page_url = format!(
                "https://drive.google.com/uc?id={}&export=download",
                file_id
            );
            if let Ok(resp) = self
                .client
                .get(&page_url)
                .header("User-Agent", USER_AGENT)
                .send()
                .await
            {
                if let Ok(html) = resp.text().await {
                    let title_re = Regex::new(r"<title>(.*?)</title>").ok();
                    if let Some(re) = title_re {
                        if let Some(caps) = re.captures(&html) {
                            let t = Self::normalize_drive_title(&caps[1]);
                            if !t.is_empty() {
                                title = t;
                            }
                        }
                    }
                }
            }
        }

        Some(TrackInfo {
            identifier: file_id.to_string(),
            is_seekable: true,
            author: "Google Drive".into(),
            length: duration_ms,
            is_stream: false,
            position: 0,
            title,
            uri: Some(url.to_string()),
            artwork_url: Some(format!("https://lh3.googleusercontent.com/d/{}", file_id)),
            isrc: None,
            source_name: "googledrive".into(),
            chapters: None,
        })
    }

    fn extract_filename(disposition: &str) -> Option<String> {
        let utf8_re = Regex::new(r"filename\*\s*=\s*UTF-8''([^;]+)").ok()?;
        if let Some(caps) = utf8_re.captures(disposition) {
            let decoded = urlencoding::decode(&caps[1]).ok()?;
            let cleaned: String = decoded.chars().filter(|&c| !r#"\/:*?"<>|"#.contains(c)).collect();
            let trimmed = cleaned.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }

        let simple_re = Regex::new(r#"filename\s*=\s*"([^"]+)""#).ok()?;
        if let Some(caps) = simple_re.captures(disposition) {
            let cleaned: String = caps[1].chars().filter(|&c| !r#"\/:*?"<>|"#.contains(c)).collect();
            let trimmed = cleaned.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }

        None
    }

    async fn resolve_folder_via_embedded(&self, folder_id: &str) -> Option<(String, Vec<TrackData>)> {
        let url = format!(
            "https://drive.google.com/embeddedfolderview?id={}#list",
            folder_id
        );

        let resp = self
            .client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
            .ok()?;

        let html = resp.text().await.ok()?;

        let title_re = Regex::new(r"<title>(.*?)</title>").ok()?;
        let folder_title = title_re
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| Self::decode_html_entities(m.as_str()))
            .unwrap_or_else(|| "Google Drive Folder".to_string());

        let entry_re = Regex::new(
            r#"<div class="flip-entry" id="entry-([A-Za-z0-9_-]{20,})"[\s\S]*?<a href="https://drive\.google\.com/file/d/([A-Za-z0-9_-]{20,})/view[^"]*"[\s\S]*?drive-thirdparty\.googleusercontent\.com/(?:128|16)/type/([^"/]+/[^"/]+)[\s\S]*?<div class="flip-entry-title">([\s\S]*?)</div>"#,
        ).ok()?;

        let mut tracks = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for caps in entry_re.captures_iter(&html) {
            let file_id = caps.get(2)?.as_str().to_string();
            let mime = Self::decode_html_entities(caps.get(3)?.as_str());
            if seen.contains(&file_id) {
                continue;
            }
            if !mime.starts_with("audio/") && !mime.starts_with("video/") {
                continue;
            }
            let raw_title = Self::decode_html_entities(caps.get(4)?.as_str());
            let title = if raw_title.is_empty() {
                format!("Google Drive File {}", file_id)
            } else {
                raw_title
            };

            let track_info = TrackInfo {
                identifier: file_id.clone(),
                is_seekable: true,
                author: "Google Drive".into(),
                length: 0,
                is_stream: false,
                position: 0,
                title,
                uri: Some(format!("https://drive.google.com/file/d/{}/view", file_id)),
                artwork_url: Some(format!("https://lh3.googleusercontent.com/d/{}", file_id)),
                isrc: None,
                source_name: "googledrive".into(),
                chapters: None,
            };

            let mut track = TrackData {
                encoded: None,
                info: track_info,
                plugin_info: serde_json::json!({}),
                user_data: serde_json::json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            track.encoded = Some(encode_track(&track));
            tracks.push(track);
            seen.insert(file_id);
        }

        if tracks.is_empty() {
            return None;
        }
        Some((folder_title, tracks))
    }
}

#[async_trait]
impl SourceProvider for GoogleDriveSource {
    fn name(&self) -> &'static str {
        "googledrive"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["gdrive", "drive"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["gdsearch"]
    }

    async fn search(&self, _query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let folder_pattern = Regex::new(
            r"https?://(?:docs|drive)\.google\.com/drive/folders/([a-zA-Z0-9_-]{28,})",
        )
        .ok();

        if let Some(re) = folder_pattern {
            if let Some(caps) = re.captures(query) {
                let folder_id = &caps[1];
                if let Some((title, tracks)) = self.resolve_folder_via_embedded(folder_id).await {
                    return Ok(SourceResult::Playlist {
                        data: PlaylistData {
                            encoded: String::new(),
                            info: PlaylistInfo {
                                name: title,
                                selected_track: 0,
                            },
                            plugin_info: serde_json::json!({}),
                            tracks,
                        },
                    });
                }
            }
        }

        let file_pattern = Regex::new(
            r"https?://(?:docs|drive|drive\.usercontent)\.google\.com/(?:(?:uc|open|download)\?.*?id=|file/d/)([a-zA-Z0-9_-]{28,})",
        )
        .ok();

        let video_pattern =
            Regex::new(r"https?://video\.google\.com/get_player\?.*?docid=([a-zA-Z0-9_-]{28,})")
                .ok();

        let file_id = file_pattern
            .as_ref()
            .and_then(|re| re.captures(query))
            .or_else(|| video_pattern.as_ref().and_then(|re| re.captures(query)))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str());

        match file_id {
            Some(id) => {
                let info = self.get_file_info(id, query).await;
                match info {
                    Some(track_info) => {
                        let mut track = TrackData {
                            encoded: None,
                            info: track_info,
                            plugin_info: serde_json::json!({}),
                            user_data: serde_json::json!({}),
                            details: Vec::new(),
                            message_flags: 0,
                        };
                        track.encoded = Some(encode_track(&track));
                        Ok(SourceResult::Track(track))
                    }
                    None => Ok(SourceResult::Empty),
                }
            }
            None => Ok(SourceResult::Empty),
        }
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let file_id = &track.identifier;
        let url = format!(
            "https://drive.google.com/uc?id={}&export=download&authuser=0",
            file_id
        );
        let format = Self::get_format(None, Some(&track.title));

        Ok(TrackUrlResult {
            url: Some(url),
            protocol: Some("https".into()),
            format: serde_json::json!(format),
            new_track: None,
            additional_data: serde_json::json!({}),
            exception: None,
        })
    }
}
