use super::LyricsData;

const APP_ID: &str = "web-desktop-app-v1.0";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const TOKEN_URL: &str = "https://apic-desktop.musixmatch.com/ws/1.1/token.get";
const MACRO_URL: &str = "https://apic-desktop.musixmatch.com/ws/1.1/macro.subtitles.get";

const CLEAN_PATTERNS: &[&str] = &[
    r"\s*\([^)]*(?:official|lyrics?|video|audio|mv|visualizer|color\s*coded|hd|4k|prod\.)[^)]*\)",
    r"\s*\[[^\]]*(?:official|lyrics?|video|audio|mv|visualizer|color\s*coded|hd|4k|prod\.)[^\]]*\]",
    r"\s*-\s*Topic$",
    r"VEVO$",
    r"\s*[([]\s*(?:ft\.?|feat\.?|featuring)\s+[^)\]]+[)\]]",
];

async fn fetch_token(client: &reqwest::Client) -> anyhow::Result<String> {
    let url = format!("{}?app_id={}", TOKEN_URL, APP_ID);
    let resp = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "*/*")
        .header("Cookie", "AWSELB=unknown; x-mxm-user-id=undefined; x-mxm-token-guid=undefined")
        .send()
        .await?;

    if resp.status() != 200 {
        anyhow::bail!("Musixmatch token fetch failed: HTTP {}", resp.status());
    }

    let data: serde_json::Value = resp.json().await?;
    let status = data["message"]["header"]["status_code"].as_i64().unwrap_or(0);
    if status != 200 {
        anyhow::bail!("Musixmatch token API error: {}", status);
    }

    data["message"]["body"]["user_token"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No token in Musixmatch response"))
}

fn clean_title_artist(title: &str) -> String {
    let mut result = title.to_string();
    for pattern in CLEAN_PATTERNS {
        if let Ok(re) = regex::Regex::new(pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }
    result.trim().to_string()
}

fn parse_subtitles(sub_body: &str) -> Option<Vec<(f64, String)>> {
    let val: serde_json::Value = serde_json::from_str(sub_body).ok()?;
    let items = val.as_array().or_else(|| val.get("subtitle")?.as_array())?;

    let mut lines = Vec::new();
    for item in items {
        let text = item.get("text")?.as_str()?;
        let time = item.get("time")?.get("total")?.as_f64()?;
        if !text.trim().is_empty() {
            lines.push((time, text.to_string()));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines)
}

fn parse_plain_lyrics(lyrics_body: &str) -> Option<String> {
    let text = lyrics_body.trim();
    if text.is_empty() || text == "..." {
        return None;
    }
    Some(text.to_string())
}

pub async fn fetch_musixmatch(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let token = match fetch_token(client).await {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };

    let clean_title = clean_title_artist(title);
    let clean_artist = clean_title_artist(artist);

    let macro_url = format!(
        "{}?app_id={}&format=json&namespace=lyrics_richsynched&subtitle_format=mxm&q_track={}&q_artist={}&usertoken={}",
        MACRO_URL,
        APP_ID,
        urlencoding(&clean_title),
        urlencoding(&clean_artist),
        token
    );

    let resp = client
        .get(&macro_url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await?;

    if resp.status() != 200 {
        return Ok(None);
    }

    let data: serde_json::Value = resp.json().await?;
    let status = data["message"]["header"]["status_code"].as_i64().unwrap_or(0);
    if status != 200 {
        return Ok(None);
    }

    let macro_calls = &data["message"]["body"]["macro_calls"];
    let lyrics_body = macro_calls["track.lyrics.get"]["message"]["body"]["lyrics"]["lyrics_body"]
        .as_str();
    let subtitle_body = macro_calls["track.subtitles.get"]["message"]["body"]["subtitle_list"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|item| item["subtitle"]["subtitle_body"].as_str());

    let track_name = macro_calls["matcher.track.get"]["message"]["body"]["track"]["track_name"]
        .as_str()
        .unwrap_or(title);

    if let Some(sub_body) = subtitle_body {
        if let Some(lines) = parse_subtitles(sub_body) {
            let synced = lines
                .into_iter()
                .map(|(t, text)| super::SyncedLyricLine {
                    time: t,
                    text,
                })
                .collect();
            return Ok(Some(LyricsData {
                source: "musixmatch".into(),
                title: track_name.to_string(),
                artist: clean_artist.clone(),
                album: None,
                synced_lyrics: synced,
                plain_lyrics: None,
            }));
        }
    }

    if let Some(body) = lyrics_body {
        if let Some(plain) = parse_plain_lyrics(body) {
            return Ok(Some(LyricsData {
                source: "musixmatch".into(),
                title: track_name.to_string(),
                artist: clean_artist.clone(),
                album: None,
                synced_lyrics: vec![],
                plain_lyrics: Some(plain),
            }));
        }
    }

    Ok(None)
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
