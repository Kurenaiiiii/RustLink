pub mod genius;
pub mod musixmatch;
pub mod deezer;
pub mod yandex;
pub mod bilibili;
pub mod letrasmus;
pub mod monochrome;
pub mod aligner;
pub mod lrclib;
pub mod youtube;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedLyricLine {
    pub time: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsData {
    pub source: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub synced_lyrics: Vec<SyncedLyricLine>,
    pub plain_lyrics: Option<String>,
}

fn parse_lrc_time(s: &str) -> Result<f64, ()> {
    if let Some((mins, rest)) = s.split_once(':') {
        let m: f64 = mins.parse().map_err(|_| ())?;
        let secs: f64 = rest.parse().map_err(|_| ())?;
        Ok(m * 60.0 + secs)
    } else {
        Err(())
    }
}

pub(crate) fn parse_lrc(lrc: &str) -> Vec<SyncedLyricLine> {
    let mut lines = Vec::new();
    for line in lrc.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some((time_str, text)) = rest.split_once(']') {
                if let Ok(seconds) = parse_lrc_time(time_str) {
                    let t = text.trim().to_string();
                    if !t.is_empty() {
                        lines.push(SyncedLyricLine {
                            time: seconds,
                            text: t,
                        });
                    }
                }
            }
        }
    }
    lines.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
    lines
}

pub(crate) fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub async fn fetch_lyrics(
    title: &str,
    artist: &str,
    album: Option<&str>,
    identifier: Option<&str>,
) -> anyhow::Result<Option<LyricsData>> {
    let client = reqwest::Client::new();

    // Try YouTube captions first if we have a video ID
    if let Some(video_id) = identifier {
        if let Some(lyrics) = youtube::fetch_youtube_captions(&client, video_id, title, artist).await? {
            return Ok(Some(lyrics));
        }
    }

    // Try dedicated lyrics providers
    if let Some(lyrics) = musixmatch::fetch_musixmatch(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = lrclib::fetch_lrclib(&client, title, artist, album).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = genius::fetch_genius(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = deezer::fetch_deezer(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = yandex::fetch_yandex(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = monochrome::fetch_monochrome(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    if let Some(lyrics) = letrasmus::fetch_letrasmus(&client, title, artist).await? {
        return Ok(Some(lyrics));
    }

    Ok(None)
}
