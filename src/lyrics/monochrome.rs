use crate::lyrics::{LyricsData, SyncedLyricLine, parse_lrc, urlencoding};

pub async fn fetch_monochrome(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let search_url = format!(
        "https://api.monochrome.lyrics/api/v1/search?q={}%20{}",
        urlencoding(artist),
        urlencoding(title)
    );

    match client.get(&search_url).send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
            let results: serde_json::Value = resp.json().await?;
            let track = results["data"]
                .as_array()
                .and_then(|arr| arr.first())
                .ok_or_else(|| anyhow::anyhow!("No track found"))?;

            let track_id = track["id"].as_str()
                .ok_or_else(|| anyhow::anyhow!("No track ID"))?;
            let lrc_url = format!(
                "https://api.monochrome.lyrics/api/v1/lyrics/{}/lrc",
                track_id
            );

            let lrc_resp = client.get(&lrc_url).send().await?;
            if lrc_resp.status() != reqwest::StatusCode::OK {
                return Ok(None);
            }

            let lrc_text = lrc_resp.text().await?;
            let synced = parse_lrc(&lrc_text);

            let plain = track["lyrics"].as_str().unwrap_or("");

            Ok(Some(LyricsData {
                source: "monochrome".into(),
                title: track["title"].as_str().unwrap_or(title).to_string(),
                artist: track["artist"].as_str().unwrap_or(artist).to_string(),
                album: track["album"].as_str().map(|s| s.to_string()),
                synced_lyrics: synced,
                plain_lyrics: if plain.is_empty() { None } else { Some(plain.to_string()) },
            }))
        }
        _ => Ok(None),
    }
}
