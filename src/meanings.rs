use serde::{Deserialize, Serialize};

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
    pub url: Option<String>,
    pub source: String,
    pub song: MeaningSongInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeaningSongInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artwork_url: Option<String>,
}

pub async fn load_meaning(
    title: &str,
    author: &str,
    language: &str,
) -> anyhow::Result<Option<MeaningResult>> {
    // Try Wikipedia first (simpler, no HTML parsing needed)
    if let Some(result) = wikipedia_meaning(title, author, language).await? {
        return Ok(Some(result));
    }
    Ok(None)
}

async fn wikipedia_meaning(
    title: &str,
    author: &str,
    language: &str,
) -> anyhow::Result<Option<MeaningResult>> {
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
                    url: Some(url),
                    source: "wikipedia".into(),
                    song: MeaningSongInfo {
                        title: Some(title.to_string()),
                        artist: Some(author.to_string()),
                        artwork_url: None,
                    },
                },
            }));
        }
    }

    Ok(None)
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
