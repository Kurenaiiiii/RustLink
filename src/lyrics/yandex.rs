use crate::lyrics::{LyricsData, SyncedLyricLine, urlencoding, parse_lrc};

pub async fn fetch_yandex(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let search_url = format!(
        "https://api.music.yandex.net/search?type=track&page=0&text={}%20{}",
        urlencoding(artist),
        urlencoding(title)
    );
    let resp = client.get(&search_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let data: serde_json::Value = resp.json().await?;
    let track_id = data["result"]["tracks"]["results"][0]["id"].as_u64();
    let track_id = match track_id {
        Some(id) => id,
        None => return Ok(None),
    };

    let lyrics_url = format!("https://api.music.yandex.net/tracks/{}/lyrics", track_id);
    let resp = client.get(&lyrics_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let lyrics_json: serde_json::Value = resp.json().await?;
    let lyrics_data = &lyrics_json["result"];

    let full_lyrics = lyrics_data["lyrics"].as_str().unwrap_or("");
    let has_sync = lyrics_data["hasSync"].as_bool().unwrap_or(false);

    let synced = if has_sync {
        parse_lrc(full_lyrics)
    } else {
        Vec::new()
    };

    Ok(Some(LyricsData {
        source: "yandexmusic".into(),
        title: title.to_string(),
        artist: artist.to_string(),
        album: None,
        synced_lyrics: synced,
        plain_lyrics: if full_lyrics.is_empty() { None } else { Some(full_lyrics.to_string()) },
    }))
}
