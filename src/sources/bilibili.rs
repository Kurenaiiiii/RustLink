use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

const BILIBILI_API: &str = "https://api.bilibili.com/x/web-interface";

pub struct BilibiliSource {
    client: Client,
}

impl BilibiliSource {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                .default_headers({
                    let mut h = reqwest::header::HeaderMap::new();
                    h.insert(
                        reqwest::header::REFERER,
                        reqwest::header::HeaderValue::from_static("https://www.bilibili.com"),
                    );
                    h
                })
                .build()
                .unwrap(),
        }
    }

    async fn api_get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{BILIBILI_API}{path}"))
            .send()
            .await?;
        let data: Value = resp.error_for_status()?.json().await?;
        if data["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("Bilibili API error {}: {}", data["code"], data["message"]);
        }
        Ok(data["data"].clone())
    }

    fn extract_id(input: &str) -> Option<(String, String)> {
        // BV ID: BV1xx...
        if input.starts_with("BV") && input.len() >= 12 {
            return Some(("bvid".into(), input.to_string()));
        }
        // AV ID: av12345 or aid=12345
        if let Some(id) = input.strip_prefix("av").or_else(|| input.strip_prefix("AV")) {
            if id.chars().all(|c| c.is_ascii_digit()) {
                return Some(("aid".into(), id.to_string()));
            }
        }
        if let Some(id) = input.strip_prefix("aid=") {
            if id.chars().all(|c| c.is_ascii_digit()) {
                return Some(("aid".into(), id.to_string()));
            }
        }

        // URL parsing
        if let Ok(url) = url::Url::parse(input) {
            if let Some(host) = url.host_str() {
                if host.contains("bilibili.com") {
                    let segs: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
                    for seg in &segs {
                        if seg.starts_with("BV") && seg.len() >= 12 {
                            return Some(("bvid".into(), seg.to_string()));
                        }
                        if let Some(id) = seg.strip_prefix("av").or_else(|| seg.strip_prefix("AV")) {
                            if id.chars().all(|c| c.is_ascii_digit()) {
                                return Some(("aid".into(), id.to_string()));
                            }
                        }
                    }
                    // Check query params
                    if let Some(bv) = url.query_pairs().find(|(k, _)| k == "bvid") {
                        return Some(("bvid".into(), bv.1.to_string()));
                    }
                    if let Some(av) = url.query_pairs().find(|(k, _)| k == "aid" || k == "avid") {
                        return Some(("aid".into(), av.1.to_string()));
                    }
                }
            }
        }
        None
    }
}

#[async_trait]
impl SourceProvider for BilibiliSource {
    fn name(&self) -> &'static str {
        "bilibili"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["bili", "bilibili"]
    }

    async fn search(&self, query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let resp = self
            .client
            .get(format!("https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={encoded}&page=1&order=click"))
            .header("Referer", "https://www.bilibili.com")
            .send()
            .await;

        let data = match resp {
            Ok(r) => match r.json::<Value>().await {
                Ok(j) => j,
                Err(e) => {
                    warn!(target: "Bilibili", "Search parse error: {e}");
                    return Ok(SourceResult::Empty);
                }
            },
            Err(e) => {
                warn!(target: "Bilibili", "Search request error: {e}");
                return Ok(SourceResult::Empty);
            }
        };

        if data["code"].as_i64().unwrap_or(-1) != 0 {
            return Ok(SourceResult::Empty);
        }

        let results = data
            .pointer("/data/result")
            .and_then(|r| r.as_array())
            .map(|a| a.to_vec())
            .unwrap_or_default();

        let mut tracks: Vec<TrackData> = Vec::new();
        for item in &results {
            let bvid = item["bvid"].as_str().unwrap_or("");
            let title = item["title"].as_str().unwrap_or("");
            let author = item["author"].as_str().unwrap_or("Unknown");
            let duration = item["duration"].as_str().unwrap_or("0");
            let duration_ms = parse_bili_duration(duration);
            let pic = item["pic"].as_str();

            if bvid.is_empty() || title.is_empty() {
                continue;
            }

            let mut track = TrackData {
                encoded: None,
                info: TrackInfo {
                    identifier: bvid.to_string(),
                    is_seekable: true,
                    author: author.to_string(),
                    length: duration_ms,
                    is_stream: false,
                    position: 0,
                    title: title.to_string(),
                    uri: Some(format!("https://www.bilibili.com/video/{bvid}")),
                    artwork_url: pic.map(|s| {
                        if s.starts_with("//") {
                            format!("https:{s}")
                        } else {
                            s.to_string()
                        }
                    }),
                    isrc: None,
                    source_name: "bilibili".to_string(),
                    chapters: None,
                },
                plugin_info: json!({}),
                user_data: json!({}),
                details: Vec::new(),
                message_flags: 0,
            };
            track.encoded = Some(encode_track(&track));
            tracks.push(track);
        }

        if tracks.is_empty() {
            Ok(SourceResult::Empty)
        } else {
            Ok(SourceResult::Search { data: tracks })
        }
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let trimmed = query.trim();
        let (kind, id) = match Self::extract_id(trimmed) {
            Some(v) => v,
            None => return self.search(trimmed, None).await,
        };

        info!(target: "Bilibili", "Resolving {kind}: {id}");

        let path = match kind.as_str() {
            "bvid" => format!("/view?bvid={id}"),
            "aid" => format!("/view?aid={id}"),
            _ => return Ok(SourceResult::Empty),
        };

        let data = match self.api_get(&path).await {
            Ok(d) => d,
            Err(e) => {
                warn!(target: "Bilibili", "Resolve error: {e}");
                return Ok(SourceResult::Empty);
            }
        };

        let bvid = data["bvid"].as_str().unwrap_or(&id);
        let title = data["title"].as_str().unwrap_or("Unknown Title");
        let author = data
            .get("owner")
            .and_then(|o| o["name"].as_str())
            .unwrap_or("Unknown");
        let duration = data["duration"].as_i64().unwrap_or(0) * 1000;
        let pic = data["pic"].as_str();
        let cid = data
            .pointer("/cid")
            .or_else(|| {
                data.pointer("/pages/0/cid")
            })
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: if cid > 0 {
                    format!("{bvid}/{cid}")
                } else {
                    bvid.to_string()
                },
                is_seekable: true,
                author: author.to_string(),
                length: duration,
                is_stream: false,
                position: 0,
                title: title.to_string(),
                uri: Some(format!("https://www.bilibili.com/video/{bvid}")),
                artwork_url: pic.map(|s| {
                    if s.starts_with("//") {
                        format!("https:{s}")
                    } else {
                        s.to_string()
                    }
                }),
                isrc: None,
                source_name: "bilibili".to_string(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));

        Ok(SourceResult::Track(track))
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let (bvid, cid) = track.identifier.split_once('/').unwrap_or((&track.identifier, "0"));
        let cid_val = cid.parse::<u64>().unwrap_or(0);

        // First get video info for cid if needed
        let (avid, cid) = if cid_val > 0 {
            (0u64, cid_val)
        } else {
            let data = match self.api_get(&format!("/view?bvid={bvid}")).await {
                Ok(d) => d,
                Err(e) => {
                    return Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: json!({}),
                        new_track: None,
                        additional_data: json!({}),
                        exception: Some(format!("Bilibili: view error: {e}")),
                    });
                }
            };
            let avid = data["aid"].as_i64().unwrap_or(0) as u64;
            let cid = data
                .pointer("/cid")
                .or_else(|| data.pointer("/pages/0/cid"))
                .and_then(|c| c.as_i64())
                .unwrap_or(0) as u64;
            (avid, cid)
        };

        let avid = if avid > 0 {
            avid
        } else {
            // Try to get avid from bvid
            let data = match self.api_get(&format!("/view?bvid={bvid}")).await {
                Ok(d) => d,
                Err(e) => {
                    return Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: json!({}),
                        new_track: None,
                        additional_data: json!({}),
                        exception: Some(format!("Bilibili: view error: {e}")),
                    });
                }
            };
            data["aid"].as_i64().unwrap_or(0) as u64
        };

        if avid == 0 || cid == 0 {
            return Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: json!({}),
                new_track: None,
                additional_data: json!({}),
                exception: Some("Bilibili: could not determine avid/cid".into()),
            });
        }

        // Get play URL
        let play_url = format!(
            "https://api.bilibili.com/x/player/playurl?avid={avid}&cid={cid}&qn=16&fnver=0&fnval=4048&fourk=1"
        );
        let resp = self
            .client
            .get(&play_url)
            .header("Referer", "https://www.bilibili.com")
            .send()
            .await;

        let play_data = match resp {
            Ok(r) => match r.json::<Value>().await {
                Ok(j) => j,
                Err(e) => {
                    return Ok(TrackUrlResult {
                        url: None,
                        protocol: None,
                        format: json!({}),
                        new_track: None,
                        additional_data: json!({}),
                        exception: Some(format!("Bilibili: playurl parse error: {e}")),
                    });
                }
            },
            Err(e) => {
                return Ok(TrackUrlResult {
                    url: None,
                    protocol: None,
                    format: json!({}),
                    new_track: None,
                    additional_data: json!({}),
                    exception: Some(format!("Bilibili: playurl request error: {e}")),
                });
            }
        };

        if play_data["code"].as_i64().unwrap_or(-1) != 0 {
            return Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: json!({}),
                new_track: None,
                additional_data: json!({}),
                exception: Some(format!(
                    "Bilibili: playurl error {}: {}",
                    play_data["code"],
                    play_data["message"]
                )),
            });
        }

        // Try dash audio first, then flac, then mp3
        let audio_url = play_data
            .pointer("/data/dash/audio")
            .and_then(|a| a.as_array())
            .and_then(|arr| {
                arr.iter()
                    .max_by_key(|a| a.get("bandwidth").and_then(|b| b.as_i64()).unwrap_or(0))
            })
            .and_then(|best| best["baseUrl"].as_str())
            .or_else(|| {
                play_data
                    .pointer("/data/durl/0/url")
                    .and_then(|u| u.as_str())
            })
            .or_else(|| {
                play_data
                    .pointer("/data/durl/0/backup_url/0")
                    .and_then(|u| u.as_str())
            });

        match audio_url {
            Some(url) => Ok(TrackUrlResult {
                url: Some(url.to_string()),
                protocol: Some("https".into()),
                format: json!({"protocol": "https"}),
                new_track: None,
                additional_data: json!({}),
                exception: None,
            }),
            None => Ok(TrackUrlResult {
                url: None,
                protocol: None,
                format: json!({}),
                new_track: None,
                additional_data: json!({}),
                exception: Some("Bilibili: no audio URL found".into()),
            }),
        }
    }
}

fn parse_bili_duration(dur: &str) -> i64 {
    let parts: Vec<&str> = dur.split(':').collect();
    match parts.len() {
        3 => {
            let h = parts[0].parse::<i64>().unwrap_or(0);
            let m = parts[1].parse::<i64>().unwrap_or(0);
            let s = parts[2].parse::<i64>().unwrap_or(0);
            (h * 3600 + m * 60 + s) * 1000
        }
        2 => {
            let m = parts[0].parse::<i64>().unwrap_or(0);
            let s = parts[1].parse::<i64>().unwrap_or(0);
            (m * 60 + s) * 1000
        }
        1 => parts[0].parse::<i64>().unwrap_or(0) * 1000,
        _ => 0,
    }
}
