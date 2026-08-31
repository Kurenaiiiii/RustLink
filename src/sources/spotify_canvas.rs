/// Spotify Canvas — looping video/gif backdrop for tracks.
/// Uses Spotify internal API to fetch canvas metadata.

use serde_json::Value;

pub struct SpotifyCanvas {
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct CanvasData {
    pub url: String,
    pub file_id: String,
    pub artist_id: String,
    pub width: u32,
    pub height: u32,
    pub canvas_type: CanvasType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CanvasType {
    Video,
    Image,
    Unknown,
}

impl SpotifyCanvas {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Fetch canvas for a track using Spotify's internal API.
    /// Requires a valid Spotify access token.
    pub async fn fetch_canvas(&self, track_id: &str, access_token: &str) -> anyhow::Result<Option<CanvasData>> {
        let url = format!(
            "https://api.spotify.com/v1/tracks/{}/canvas",
            track_id
        );

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Ok(None);
        }

        let data: Value = resp.json().await?;
        let canvas = data["canvases"]
            .as_array()
            .and_then(|arr| arr.first())
            .ok_or_else(|| anyhow::anyhow!("No canvas found"))?;

        let url = canvas["url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No canvas URL"))?
            .to_string();
        let file_id = canvas["fileId"].as_str().unwrap_or("").to_string();
        let artist_id = canvas["artistId"].as_str().unwrap_or("").to_string();
        let width = canvas["width"].as_u64().unwrap_or(0) as u32;
        let height = canvas["height"].as_u64().unwrap_or(0) as u32;
        let ct = match canvas["type"].as_str() {
            Some("video") | Some("VIDEO") => CanvasType::Video,
            Some("image") | Some("IMAGE") => CanvasType::Image,
            _ => CanvasType::Unknown,
        };

        Ok(Some(CanvasData {
            url,
            file_id,
            artist_id,
            width,
            height,
            canvas_type: ct,
        }))
    }

    /// Scrape canvas URL from Spotify's embed page (no token required).
    pub async fn scrape_canvas(&self, track_id: &str) -> anyhow::Result<Option<String>> {
        let url = format!("https://open.spotify.com/embed/track/{}", track_id);
        let resp = self.client.get(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Ok(None);
        }

        let html = resp.text().await?;

        // Look for canvas URL in the embed JSON
        if let Some(start) = html.find(r#""canvasUrl""#) {
            let rest = &html[start..];
            if let Some(url_start) = rest.find(r#""#) {
                let after_quote = &rest[url_start + 1..];
                if let Some(url_end) = after_quote.find(r#""#) {
                    let canvas_url = &after_quote[..url_end];
                    if !canvas_url.is_empty() {
                        return Ok(Some(canvas_url.to_string()));
                    }
                }
            }
        }

        Ok(None)
    }
}
