use async_trait::async_trait;
use reqwest::Url;
use serde_json::json;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::{encode_track, TrackData, TrackInfo};

pub struct GoogleTtsProvider {
    language: String,
}

impl GoogleTtsProvider {
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
        }
    }

    fn build_title(text: &str) -> String {
        let visible = if text.chars().count() > 50 {
            let prefix: String = text.chars().take(47).collect();
            format!("{prefix}...")
        } else {
            text.to_owned()
        };
        format!("TTS: {visible}")
    }

    fn build_url(&self, text: &str) -> anyhow::Result<String> {
        let text_len = text.chars().count().to_string();
        let url = Url::parse_with_params(
            "https://translate.google.com/translate_tts",
            &[
                ("ie", "UTF-8"),
                ("q", text),
                ("tl", self.language.as_str()),
                ("total", "1"),
                ("idx", "0"),
                ("textlen", text_len.as_str()),
                ("client", "gtx"),
            ],
        )?;
        Ok(url.to_string())
    }
}

#[async_trait]
impl SourceProvider for GoogleTtsProvider {
    fn name(&self) -> &'static str {
        "google-tts"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["gtts", "speak"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["gtts", "speak"]
    }

    async fn search(
        &self,
        query: &str,
        _search_type: Option<&str>,
    ) -> anyhow::Result<SourceResult> {
        self.resolve(query, None).await
    }

    async fn resolve(&self, text: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(SourceResult::empty());
        }

        let url = self.build_url(text)?;
        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: format!("gtts:{text}"),
                is_seekable: true,
                author: "Google TTS".into(),
                length: -1,
                is_stream: false,
                position: 0,
                title: Self::build_title(text),
                uri: Some(url.clone()),
                artwork_url: None,
                isrc: None,
                source_name: "google-tts".into(),
                chapters: None,
            },
            plugin_info: json!({ "language": self.language }),
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
