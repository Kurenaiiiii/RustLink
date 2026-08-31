use crate::lyrics::{LyricsData, SyncedLyricLine, urlencoding};

pub async fn fetch_deezer(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let search_url = format!(
        "https://api.deezer.com/search?q=artist:\"{}%20track:\"{}\"",
        urlencoding(artist),
        urlencoding(title)
    );
    let resp = client.get(&search_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let data: serde_json::Value = resp.json().await?;
    let track_id = match data["data"][0]["id"].as_u64() {
        Some(id) => id,
        None => return Ok(None),
    };

    let lyrics_url = format!("https://api.deezer.com/track/{}/lyrics", track_id);
    let resp = client.get(&lyrics_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let lyrics_data: serde_json::Value = resp.json().await?;

    let lyrics_text = lyrics_data["lyrics"].as_str().unwrap_or("");
    let sync_json = lyrics_data["sync_lyrics"].as_str().unwrap_or("");

    let synced = if !sync_json.is_empty() {
        if let Ok(lines) = serde_json::from_str::<Vec<serde_json::Value>>(sync_json) {
            lines
                .iter()
                .filter_map(|l| {
                    let time = l["lrc_timestamp"].as_str()?;
                    let text = l["text"].as_str()?;
                    let (mins, rest) = time.split_once(':')?;
                    let m: f64 = mins.parse().ok()?;
                    let s: f64 = rest.parse().ok()?;
                    Some(SyncedLyricLine {
                        time: m * 60.0 + s,
                        text: text.to_string(),
                    })
                })
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    Ok(Some(LyricsData {
        source: "deezer".into(),
        title: lyrics_data["lyrics_title"].as_str().unwrap_or(title).to_string(),
        artist: lyrics_data["lyrics_artist"].as_str().unwrap_or(artist).to_string(),
        album: lyrics_data["lyrics_album"].as_str().map(|s| s.to_string()),
        synced_lyrics: synced,
        plain_lyrics: if lyrics_text.is_empty() { None } else { Some(lyrics_text.to_string()) },
    }))
}
