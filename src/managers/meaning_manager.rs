use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::MeaningsConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeaningResult {
    pub load_type: String,
    pub data: MeaningData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeaningData {
    pub title: Option<String>,
    pub description: Option<String>,
    pub paragraphs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation: Option<Value>,
    pub url: Option<String>,
    #[serde(rename = "type")]
    pub data_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meaning_meta: Option<Value>,
    pub provider: String,
    pub song: MeaningSongInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeaningSongInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub youtube_id: Option<String>,
    pub letras_id: Option<String>,
    pub artwork_url: Option<String>,
}

#[async_trait]
pub trait MeaningProvider: Send + Sync {
    fn name(&self) -> &str;
    fn priority(&self) -> u32 {
        0
    }
    async fn get_meaning(&self, title: &str, author: &str, language: &str) -> anyhow::Result<Option<MeaningResult>>;
}

pub struct MeaningManager {
    providers: Vec<Box<dyn MeaningProvider>>,
}

impl Default for MeaningManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MeaningManager {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register<P: MeaningProvider + 'static>(&mut self, provider: P) {
        self.providers.push(Box::new(provider));
    }

    pub fn load_from_config(&mut self, config: &MeaningsConfig) {
        if config.wikipedia.enabled {
            self.register(WikipediaMeaningProvider::new());
        }
        if config.letrasmus.enabled {
            self.register(super::letrasmus_meaning::LetrasMusMeaningProvider::new());
        }
    }

    pub async fn load_meaning(
        &self,
        title: &str,
        author: &str,
        language: &str,
        source_name: Option<&str>,
    ) -> anyhow::Result<Option<MeaningResult>> {
        // Try source-specific provider first
        if let Some(source) = source_name {
            for provider in &self.providers {
                if provider.name() == source {
                    if let Some(result) = provider.get_meaning(title, author, language).await? {
                        return Ok(Some(result));
                    }
                }
            }
        }

        // Try all providers sorted by priority
        let mut sorted: Vec<&Box<dyn MeaningProvider>> = self.providers.iter().collect();
        sorted.sort_by(|a, b| b.priority().cmp(&a.priority()));

        for provider in sorted {
            let result = provider.get_meaning(title, author, language).await?;
            if result.is_some() {
                return Ok(result);
            }
        }

        Ok(None)
    }
}

struct WikipediaMeaningProvider;

impl WikipediaMeaningProvider {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MeaningProvider for WikipediaMeaningProvider {
    fn name(&self) -> &str {
        "wikipedia"
    }

    fn priority(&self) -> u32 {
        10
    }

    async fn get_meaning(&self, title: &str, author: &str, language: &str) -> anyhow::Result<Option<MeaningResult>> {
        let client = reqwest::Client::new();
        let queries = [
            format!("{} (song)", title),
            title.to_string(),
            author.to_string(),
        ];

        for query in &queries {
            let encoded = urlencoding(query);
            let url = format!(
                "https://{}.wikipedia.org/w/api.php?action=query&format=json&prop=extracts|description&titles={}&redirects=1&explaintext=1",
                language, encoded
            );

            let resp = client.get(&url).send().await?;
            if resp.status() != reqwest::StatusCode::OK {
                continue;
            }
            let data: serde_json::Value = resp.json().await?;
            let pages = data["query"]["pages"].as_object().cloned().unwrap_or_default();
            for (_page_id, page) in &pages {
                let extract = page["extract"].as_str().unwrap_or("");
                if extract.is_empty() || extract == "\n" {
                    continue;
                }
                let paragraphs: Vec<String> = extract
                    .split('\n')
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                if paragraphs.is_empty() {
                    continue;
                }
                let page_title = page["title"].as_str().unwrap_or(query);
                let url = format!(
                    "https://{}.wikipedia.org/wiki/{}",
                    language,
                    urlencoding(&page_title.replace(' ', "_"))
                );
                return Ok(Some(MeaningResult {
                    load_type: "meaning".into(),
                    data: MeaningData {
                        title: page["description"].as_str().map(|s| s.to_string()),
                        description: None,
                        paragraphs,
                        translation: None,
                        url: Some(url),
                        data_type: None,
                        meaning_meta: None,
                        provider: "wikipedia".into(),
                        song: MeaningSongInfo {
                            title: Some(title.to_string()),
                            artist: Some(author.to_string()),
                            youtube_id: None,
                            letras_id: None,
                            artwork_url: None,
                        },
                    },
                }));
            }
        }

        Ok(None)
    }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
