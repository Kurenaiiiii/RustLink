use super::{urlencoding, parse_lrc, LyricsData};

pub async fn fetch_lrclib(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: Option<&str>,
) -> anyhow::Result<Option<LyricsData>> {
    let mut url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}",
        urlencoding(artist),
        urlencoding(title)
    );
    if let Some(album) = album {
        url.push_str(&format!("&album_name={}", urlencoding(album)));
    }

    match client.get(&url).send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
            let data: serde_json::Value = resp.json().await?;
            let synced_raw = data["syncedLyrics"].as_str().unwrap_or("");

            Ok(Some(LyricsData {
                source: "lrclib".into(),
                title: data["trackName"].as_str().unwrap_or(title).to_string(),
                artist: data["artistName"].as_str().unwrap_or(artist).to_string(),
                album: data["albumName"].as_str().map(|s| s.to_string()),
                synced_lyrics: parse_lrc(synced_raw),
                plain_lyrics: data["plainLyrics"].as_str().map(|s| s.to_string()),
            }))
        }
        _ => Ok(None),
    }
}
