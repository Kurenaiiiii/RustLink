use async_trait::async_trait;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::json;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

#[derive(Default)]
pub struct HttpProvider;

#[async_trait]
impl SourceProvider for HttpProvider {
    fn name(&self) -> &'static str {
        "http"
    }

    async fn search(
        &self,
        query: &str,
        _search_type: Option<&str>,
    ) -> anyhow::Result<SourceResult> {
        self.resolve(query, None).await
    }

    async fn resolve(&self, url: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Ok(SourceResult::empty());
        }

        let response = reqwest::Client::new().head(url).send().await?;
        if !response.status().is_success() {
            return Ok(SourceResult::error(format!(
                "HTTP error {} while resolving",
                response.status().as_u16()
            )));
        }

        let headers = response.headers();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let valid_media = content_type.is_empty()
            || content_type.starts_with("audio/")
            || content_type.starts_with("video/")
            || content_type == "application/octet-stream";

        if !valid_media {
            return Ok(SourceResult::error(format!(
                "Unsupported content type: {content_type}"
            )));
        }

        let title = headers
            .get(CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .and_then(extract_filename)
            .or_else(|| url.rsplit('/').next().map(str::to_owned))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Unknown".into());
        let is_stream = headers.get(CONTENT_LENGTH).is_none();

        let artwork_url = if url.starts_with("https://cdn.discordapp.com")
            && content_type.starts_with("video/")
        {
            let cleaned = url.strip_suffix('&').unwrap_or(url);
            let base = cleaned.replace(
                "https://cdn.discordapp.com",
                "https://media.discordapp.net",
            );
            let sep = if base.contains('?') { '&' } else { '?' };
            Some(format!("{base}{sep}format=webp"))
        } else {
            None
        };

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: url.to_owned(),
                is_seekable: !is_stream,
                author: "unknown".into(),
                length: -1,
                is_stream,
                position: 0,
                title,
                uri: Some(url.to_owned()),
                artwork_url,
                isrc: None,
                source_name: "http".into(),
                chapters: None,
            },
            plugin_info: json!({ "contentType": content_type }),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));

        Ok(SourceResult::Track(track))
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        Ok(TrackUrlResult {
            url: track.uri.clone(),
            protocol: Some("https".into()),
            format: json!("mp3"),
            new_track: None,
            additional_data: json!({}),
            exception: None,
        })
    }
}

fn extract_filename(disposition: &str) -> Option<String> {
    disposition
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("filename="))
        .map(|value| value.trim_matches('"').to_owned())
}
