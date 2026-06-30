use std::collections::VecDeque;

/// Translates text using Google Translate's free API (no key required).
/// Supports auto-detection of source language.
pub async fn translate(text: &str, target_lang: &str, source_lang: Option<&str>) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let sl = source_lang.unwrap_or("auto");

    // Use the GTX endpoint (used by many OSS translate tools)
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl={}&tl={}&dt=t&q={}",
        sl,
        target_lang,
        urlencoding(text)
    );

    let resp = client.get(&url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        anyhow::bail!("Translate API returned {}", resp.status());
    }

    // Response: [[["translated","original",null,null,1],...], null, "es", null, null]
    let body: Vec<serde_json::Value> = resp.json().await?;
    let mut result = String::new();

    if let Some(translations) = body.first().and_then(|v| v.as_array()) {
        for entry in translations {
            if let Some(parts) = entry.as_array() {
                if let Some(text_val) = parts.first().and_then(|v| v.as_str()) {
                    result.push_str(text_val);
                }
            }
        }
    }

    Ok(result)
}

/// Batch translate multiple texts (preserving order)
pub async fn translate_batch(texts: &[&str], target_lang: &str, source_lang: Option<&str>) -> anyhow::Result<Vec<String>> {
    let mut results = Vec::with_capacity(texts.len());
    for text in texts {
        match translate(text, target_lang, source_lang).await {
            Ok(t) => results.push(t),
            Err(e) => results.push(format!("[error: {e}]")),
        }
    }
    Ok(results)
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Detect language of text using Google Translate
pub async fn detect_language(text: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=en&dt=t&q={}",
        urlencoding(text)
    );

    let resp = client.get(&url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        anyhow::bail!("Detect API returned {}", resp.status());
    }

    let body: Vec<serde_json::Value> = resp.json().await?;
    // Response: [[["translated","original",...,source_lang],...], detected_lang]
    if let Some(detected) = body.get(2) {
        if let Some(lang) = detected.as_str() {
            return Ok(lang.to_string());
        }
    }
    // Fallback: check inside translation entries
    if let Some(translations) = body.first().and_then(|v| v.as_array()) {
        if let Some(entry) = translations.first().and_then(|v| v.as_array()) {
            if let Some(lang) = entry.get(2).and_then(|v| v.as_str()) {
                return Ok(lang.to_string());
            }
        }
    }

    Ok("unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_translate_hello() {
        let result = translate("Hello world", "es", None).await;
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(!text.is_empty());
    }

    #[tokio::test]
    async fn test_detect_english() {
        let lang = detect_language("This is a test").await;
        assert!(lang.is_ok());
        assert_eq!(lang.unwrap(), "en");
    }
}
