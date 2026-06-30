use super::LyricsData;
use regex::Regex;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const CLEAN_PATTERNS: &[&str] = &[
    r"\s*\([^)]*(?:official|lyrics?|video|audio|mv|visualizer|color\s*coded|hd|4k|prod\.)[^)]*\)",
    r"\s*\[[^\]]*(?:official|lyrics?|video|audio|mv|visualizer|color\s*coded|hd|4k|prod\.)[^\]]*\]",
    r"\s*-\s*Topic$",
    r"VEVO$",
];

fn clean_metadata(text: &str) -> String {
    let mut result = text.to_string();
    for pattern in CLEAN_PATTERNS {
        if let Ok(re) = Regex::new(pattern) {
            result = re.replace_all(&result, "").to_string();
        }
    }
    result.trim().to_string()
}

fn extract_lyrics_from_html(html: &str) -> Option<String> {
    let re = Regex::new(r#"<div[^>]*data-lyrics-container[^>]*>"#).ok()?;
    let mut lyrics_parts = Vec::new();
    let mut remaining = html;
    while let Some(match_start) = re.find(remaining) {
        let start = match_start.end();
        let mut depth = 1;
        let mut end = start;
        for (i, b) in remaining[start..].bytes().enumerate() {
            if b == b'<' {
                if remaining[start + i..].starts_with("</div>") {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                } else if remaining[start + i..].starts_with("<div") {
                    depth += 1;
                }
            }
        }
        if end > start {
            lyrics_parts.push(&remaining[start..end]);
        }
        remaining = &remaining[end + 6..];
    }

    if lyrics_parts.is_empty() {
        return None;
    }

    let combined = lyrics_parts.join("\n");
    let with_newlines = combined.replace("<br/>", "\n").replace("<br />", "\n");
    let tag_re = Regex::new(r"<[^>]*>").unwrap();
    let stripped = tag_re.replace_all(&with_newlines, "");
    let decoded = decode_html_entities(&stripped);
    let result = decoded
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

pub async fn fetch_genius(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let clean_title = clean_metadata(title);
    let clean_artist = clean_metadata(artist);
    let query = if !clean_artist.is_empty()
        && !clean_title.to_lowercase().starts_with(&clean_artist.to_lowercase())
    {
        format!("{} {}", clean_title, clean_artist)
    } else {
        clean_title.clone()
    };

    let search_url = format!(
        "https://genius.com/api/search/multi?q={}",
        urlencoding(&query)
    );

    let resp = client
        .get(&search_url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await?;

    if resp.status() != 200 {
        return Ok(None);
    }

    let search_data: serde_json::Value = resp.json().await?;
    let song_path = match search_data["response"]["sections"]
        .as_array()
        .and_then(|sections| {
            sections.iter().find(|s| s["type"].as_str() == Some("song"))
        })
        .and_then(|section| section["hits"].as_array())
        .and_then(|hits| hits.first())
        .and_then(|hit| hit["result"]["path"].as_str())
    {
        Some(p) => p.to_string(),
        None => return Ok(None),
    };

    let page_url = format!("https://genius.com{}", song_path);
    let page_resp = client
        .get(&page_url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;

    if page_resp.status() != 200 {
        return Ok(None);
    }

    let html = page_resp.text().await?;
    let plain = match extract_lyrics_from_html(&html) {
        Some(p) => p,
        None => return Ok(None),
    };

    Ok(Some(LyricsData {
        source: "genius".into(),
        title: clean_title,
        artist: clean_artist,
        album: None,
        synced_lyrics: vec![],
        plain_lyrics: Some(plain),
    }))
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
