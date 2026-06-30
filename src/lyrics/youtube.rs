use serde_json::Value;

use super::{parse_lrc, SyncedLyricLine, LyricsData};

fn decode_caption_text(text: &str) -> String {
    text
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Fetches YouTube captions as lyrics via the Innertube player API.
pub async fn fetch_youtube_captions(
    client: &reqwest::Client,
    video_id: &str,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    // First try to get captions from the Innertube player API
    let player_url = format!(
        "https://www.youtube.com/youtubei/v1/player?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8"
    );

    let payload = serde_json::json!({
        "videoId": video_id,
        "context": {
            "client": {
                "clientName": "ANDROID",
                "clientVersion": "19.09.37",
                "androidSdkVersion": 30,
            }
        }
    });

    let resp = match client.post(&player_url).json(&payload).send().await {
        Ok(r) if r.status() == reqwest::StatusCode::OK => r,
        _ => return Ok(None),
    };

    let data: Value = match resp.json().await {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };

    let tracks = match data
        .pointer("/captions/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(|t| t.as_array())
    {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(None),
    };

    // Pick the best caption track
    let track = tracks
        .iter()
        .find(|ct| ct["languageCode"].as_str().map_or(false, |l| l.starts_with("en")))
        .or_else(|| tracks.iter().find(|ct| ct["kind"].as_str() != Some("asr")))
        .or_else(|| tracks.first())
        .and_then(|ct| {
            let base_url = ct["baseUrl"].as_str()?;
            Some(base_url.to_owned())
        });

    let base_url = match track {
        Some(t) => t,
        None => return Ok(None),
    };

    // Fetch the transcript
    let transcript_url = if base_url.contains("fmt=") {
        base_url.replace("fmt=srv1", "fmt=json3")
            .replace("fmt=srv2", "fmt=json3")
            .replace("fmt=srv3", "fmt=json3")
    } else {
        format!("{}&fmt=json3", base_url)
    };

    let transcript_resp = match client.get(&transcript_url).send().await {
        Ok(r) if r.status() == reqwest::StatusCode::OK => r,
        _ => return Ok(None),
    };

    let transcript: Value = match transcript_resp.json().await {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };

    let events = match transcript.get("events").and_then(|e| e.as_array()) {
        Some(e) => e,
        None => return Ok(None),
    };

    let mut synced_lyrics: Vec<SyncedLyricLine> = events
        .iter()
        .filter_map(|event| {
            let t_start_ms = event["tStartMs"].as_f64()?;
            let segs = event["segs"].as_array()?;
            let text: String = segs
                .iter()
                .filter_map(|seg| seg["utf8"].as_str())
                .collect();
            let decoded = decode_caption_text(&text);
            if decoded.trim().is_empty() {
                return None;
            }
            Some(SyncedLyricLine {
                time: t_start_ms / 1000.0,
                text: decoded,
            })
        })
        .collect();

    // Some YouTube captions use LRC-like format in simpleText
    if synced_lyrics.is_empty() {
        // Check if there's a simpleText-based LRC format
        for event in events {
            if let Some(text) = event["segs"].as_array().and_then(|segs| {
                let t: String = segs.iter().filter_map(|s| s["utf8"].as_str()).collect();
                if !t.is_empty() { Some(t) } else { None }
            }) {
                let lrc_lines = parse_lrc(&text);
                if !lrc_lines.is_empty() {
                    synced_lyrics.extend(lrc_lines);
                }
            }
        }
    }

    if synced_lyrics.is_empty() {
        return Ok(None);
    }

    synced_lyrics.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());

    Ok(Some(LyricsData {
        source: "youtube".into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album: None,
        synced_lyrics,
        plain_lyrics: None,
    }))
}
