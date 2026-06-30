use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::translate;

use super::meaning_manager::{MeaningData, MeaningProvider, MeaningResult, MeaningSongInfo};

const SOLR_ENDPOINT: &str = "https://solr.sscdn.co/letras/m1/";

fn clean_text(text: &str) -> String {
    let re_paren = Regex::new(r"\s*\([^)]*\)").unwrap();
    let re_bracket = Regex::new(r"\s*\[[^\]]*\]").unwrap();
    let re_noise = Regex::new(r"(?i)\b(official|video|audio|mv|visualizer|live|session|ao vivo|lyric|lyrics|hd|4k|remix|edit|cover|acoustic|instrumental)\b").unwrap();
    let re_feat = Regex::new(r"(?i)\bf(ea)?t\.?\b").unwrap();
    let re_nonword = Regex::new(r"[^\w\s]").unwrap();
    let re_spaces = Regex::new(r"\s+").unwrap();
    let mut s = text.to_string();
    s = re_paren.replace_all(&s, " ").to_string();
    s = re_bracket.replace_all(&s, " ").to_string();
    s = re_noise.replace_all(&s, " ").to_string();
    s = re_feat.replace_all(&s, " ").to_string();
    s = re_nonword.replace_all(&s, " ").to_string();
    s = re_spaces.replace_all(&s, " ").to_string();
    s.trim().to_string()
}

fn build_search_candidates(title: &str, author: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let raw_title = title.to_string();
    let cleaned_title = clean_text(&raw_title);
    let cleaned_author = clean_text(author);

    let push = |candidates: &mut Vec<String>, t: &str, a: &str| {
        let combined = [t, a].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        let combined = combined.trim().to_string();
        if !combined.is_empty() && !candidates.contains(&combined) {
            candidates.push(combined);
        }
    };

    if !cleaned_title.is_empty() || !cleaned_author.is_empty() {
        push(&mut candidates, &cleaned_title, &cleaned_author);
    }
    if !cleaned_title.is_empty() && !candidates.contains(&cleaned_title) {
        candidates.push(cleaned_title.clone());
    }

    if let Some(pos) = raw_title.find(" - ") {
        let left = raw_title[..pos].trim();
        let right = raw_title[pos + 3..].trim();
        let left_clean = clean_text(left);
        let right_clean = clean_text(right);
        if !right_clean.is_empty() {
            let combined = if cleaned_author.is_empty() {
                [right_clean.as_str(), left_clean.as_str()].join(" ")
            } else {
                [right_clean.as_str(), cleaned_author.as_str()].join(" ")
            };
            let combined = combined.trim().to_string();
            if !combined.is_empty() && !candidates.contains(&combined) {
                candidates.push(combined);
            }
            if !candidates.contains(&right_clean) {
                candidates.push(right_clean.clone());
            }
        }
        if !left_clean.is_empty() && !right_clean.is_empty() {
            let combined = [right_clean.as_str(), left_clean.as_str()].join(" ");
            if !candidates.contains(&combined) {
                candidates.push(combined);
            }
        }
    }

    if !cleaned_author.is_empty() && !candidates.contains(&cleaned_author) {
        candidates.push(cleaned_author);
    }

    candidates
}

fn parse_jsonp(body: &str) -> Option<Value> {
    let trimmed = body.trim();
    if trimmed.starts_with("LetrasSug(") && trimmed.ends_with(')') {
        let inner = &trimmed["LetrasSug(".len()..trimmed.len() - 1];
        return serde_json::from_str(inner).ok();
    }
    if let Some(start) = trimmed.find('(') {
        if let Some(end) = trimmed.rfind(')') {
            if end > start {
                return serde_json::from_str(&trimmed[start + 1..end]).ok();
            }
        }
    }
    serde_json::from_str(trimmed).ok()
}

struct SolrDoc {
    dns: String,
    url: String,
    txt: String,
    art: String,
}

async fn search_letras(client: &reqwest::Client, query: &str, limit: usize) -> Result<Vec<SolrDoc>, reqwest::Error> {
    let url = format!("{}?q={}&wt=json&callback=LetrasSug", SOLR_ENDPOINT, urlencoding(query));
    let resp = client.get(&url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(Vec::new());
    }
    let body = resp.text().await?;
    let parsed = match parse_jsonp(&body) {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };
    let docs = match parsed.pointer("/response/docs").and_then(|d| d.as_array()) {
        Some(arr) => arr,
        None => return Ok(Vec::new()),
    };
    let mut results = Vec::new();
    for doc in docs {
        let t = doc["t"].as_str().unwrap_or("");
        if t != "2" { continue; }
        let dns = doc["dns"].as_str().unwrap_or("");
        let url = doc["url"].as_str().unwrap_or("");
        if dns.is_empty() || url.is_empty() { continue; }
        results.push(SolrDoc {
            dns: dns.to_string(),
            url: url.to_string(),
            txt: doc["txt"].as_str().unwrap_or("Unknown").to_string(),
            art: doc["art"].as_str().unwrap_or("Unknown").to_string(),
        });
        if results.len() >= limit { break; }
    }
    Ok(results)
}

fn decode_html(text: &str) -> String {
    let re_dec = Regex::new(r"&#(\d+);").unwrap();
    let re_hex = Regex::new(r"&#x([0-9a-fA-F]+);").unwrap();
    let s = text
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let s = re_dec.replace_all(&s, |caps: &regex::Captures| {
        let code: u32 = caps[1].parse().unwrap_or(0);
        char::from_u32(code).map(|c| c.to_string()).unwrap_or_else(|| caps[0].to_string())
    }).to_string();
    re_hex.replace_all(&s, |caps: &regex::Captures| {
        let code: u32 = u32::from_str_radix(&caps[1], 16).unwrap_or(0);
        char::from_u32(code).map(|c| c.to_string()).unwrap_or_else(|| caps[0].to_string())
    }).to_string()
}

fn extract_meta(html: &str, property: &str) -> Option<String> {
    let re1 = Regex::new(&format!(
        r#"<meta[^>]+property=["']{}["'][^>]+content=["']([^"']+)["'][^>]*>"#, regex::escape(property)
    )).ok()?;
    let re2 = Regex::new(&format!(
        r#"<meta[^>]+content=["']([^"']+)[^>]+property=["']{}["'][^>]*>"#, regex::escape(property)
    )).ok()?;
    re1.captures(html).or_else(|| re2.captures(html)).map(|caps| decode_html(&caps[1]))
}

fn extract_omq_lyric(html: &str) -> Option<Value> {
    let re = Regex::new(r#"_omq\.push\(\s*\[\s*'ui/lyric'\s*,\s*({[\s\S]*?})\s*,"#).ok()?;
    let caps = re.captures(html)?;
    serde_json::from_str(&caps[1]).ok()
}

fn extract_omq_meaning(html: &str) -> Option<Value> {
    let re = Regex::new(r#"_omq\.push\(\s*\[\s*'ui/lyric'\s*,\s*({[\s\S]*?})\s*,\s*({[\s\S]*?})\s*,"#).ok()?;
    let caps = re.captures(html)?;
    serde_json::from_str(&caps[2]).ok()
}

struct MeaningBlock {
    title: Option<String>,
    body: Vec<String>,
}

fn extract_meaning(html: &str) -> MeaningBlock {
    let re = Regex::new(r#"<div class="lyric-meaning[^>]*">([\s\S]*?)</div>"#).unwrap();
    let block = match re.captures(html) {
        Some(caps) => caps[1].to_string(),
        None => return MeaningBlock { title: None, body: Vec::new() },
    };

    let title_re = Regex::new(r#"<h3[^>]*>([\s\S]*?)</h3>"#).unwrap();
    let title = title_re.captures(&block).and_then(|caps| {
        let t = caps[1].replace(|c: char| c == '<', " ").replace('>', " ");
        let t = decode_html(&t);
        let t = t.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });
    let block = title_re.replace(&block, "").to_string();

    let mut paragraphs: Vec<String> = Vec::new();
    let p_re = Regex::new(r#"<p[^>]*>([\s\S]*?)</p>"#).unwrap();
    for caps in p_re.captures_iter(&block) {
        let p_block = caps[1].to_string();
        let text = p_block.replace("<br />", "\n").replace("<br/>", "\n").replace("<br>", "\n");
        let text = Regex::new(r"<[^>]+>").unwrap().replace_all(&text, "").to_string();
        let text = decode_html(&text);
        let lines: Vec<String> = text.split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        if !lines.is_empty() {
            paragraphs.push(lines.join(" "));
        }
    }

    if paragraphs.is_empty() {
        let text = block.replace("<br />", "\n").replace("<br/>", "\n").replace("<br>", "\n");
        let text = Regex::new(r"<[^>]+>").unwrap().replace_all(&text, "").to_string();
        let text = decode_html(&text);
        let lines: Vec<String> = text.split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        if !lines.is_empty() {
            paragraphs.push(lines.join(" "));
        }
    }

    MeaningBlock { title, body: paragraphs }
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[derive(Default)]
pub struct LetrasMusMeaningProvider;

impl LetrasMusMeaningProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MeaningProvider for LetrasMusMeaningProvider {
    fn name(&self) -> &str {
        "letrasmus"
    }

    fn priority(&self) -> u32 {
        70
    }

    async fn get_meaning(&self, title: &str, author: &str, language: &str) -> anyhow::Result<Option<MeaningResult>> {
        let client = reqwest::Client::new();

        let search_candidates = build_search_candidates(title, author);
        let mut docs: Vec<SolrDoc> = Vec::new();
        for query in &search_candidates {
            let results = search_letras(&client, query, 12).await?;
            if !results.is_empty() {
                docs = results;
                break;
            }
        }

        if docs.is_empty() {
            return Ok(None);
        }

        let mut body: Option<String> = None;
        let mut meaning_url: Option<String> = None;
        let mut resolved_doc: Option<SolrDoc> = None;

        for doc in &docs {
            let url = format!("https://www.letras.mus.br/{}/{}/significado.html", doc.dns, doc.url);
            let resp = match client.get(&url).send().await {
                Ok(r) if r.status() == reqwest::StatusCode::OK => r,
                _ => continue,
            };
            let html = match resp.text().await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let meaning_check = extract_meaning(&html);
            if meaning_check.body.is_empty() { continue; }
            body = Some(html);
            meaning_url = Some(url);
            resolved_doc = Some(SolrDoc {
                dns: doc.dns.clone(),
                url: doc.url.clone(),
                txt: doc.txt.clone(),
                art: doc.art.clone(),
            });
            break;
        }

        let (html, murl, rdoc) = match (body, meaning_url, resolved_doc) {
            (Some(b), Some(u), Some(d)) => (b, u, d),
            _ => return Ok(None),
        };

        let meaning = extract_meaning(&html);
        let omq_lyric = extract_omq_lyric(&html);
        let omq_meaning = extract_omq_meaning(&html);
        let og_image = extract_meta(&html, "og:image");
        let og_title = extract_meta(&html, "og:title");
        let og_description = extract_meta(&html, "og:description");

        let translated = if !language.is_empty() && language != "pt" {
            let mut translated_paragraphs = Vec::new();
            for p in &meaning.body {
                match translate::translate(p, language, Some("pt")).await {
                    Ok(t) => translated_paragraphs.push(t),
                    Err(_) => translated_paragraphs.push(p.clone()),
                }
            }
            let translated_title = match meaning.title.as_ref() {
                Some(t) => translate::translate(t, language, Some("pt")).await.ok(),
                None => None,
            };
            let translated_description = match og_description.as_ref() {
                Some(d) => translate::translate(d, language, Some("pt")).await.ok(),
                None => None,
            };
            Some(Value::Object(serde_json::Map::from_iter([
                ("language".into(), Value::Object(serde_json::Map::from_iter([
                    ("source".into(), Value::String("pt".into())),
                    ("target".into(), Value::String(language.to_string())),
                ]))),
                ("title".into(), translated_title.map(Value::String).unwrap_or(Value::Null)),
                ("description".into(), translated_description.map(Value::String).unwrap_or(Value::Null)),
                ("paragraphs".into(), Value::Array(translated_paragraphs.into_iter().map(Value::String).collect())),
            ])))
        } else {
            None
        };

        let song = MeaningSongInfo {
            title: omq_lyric.as_ref().and_then(|o| o["Name"].as_str()).map(|s| s.to_owned()).or(Some(rdoc.txt.clone())),
            artist: omq_lyric.as_ref().and_then(|o| o["Artist"].as_str()).map(|s| s.to_owned()).or(Some(rdoc.art.clone())),
            youtube_id: omq_lyric.as_ref().and_then(|o| o["YoutubeID"].as_str()).map(|s| s.to_owned()),
            letras_id: omq_lyric.as_ref().and_then(|o| o["ID"].as_str()).map(|s| s.to_owned()),
            artwork_url: og_image,
        };

        let meaning_meta = Value::Object(serde_json::Map::from_iter([
            ("id".into(), omq_meaning.as_ref().and_then(|m| m["ID"].as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)),
            ("localeId".into(), omq_meaning.as_ref().and_then(|m| m["LocaleID"].as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)),
            ("origin".into(), omq_meaning.as_ref().and_then(|m| m["Origin"].as_str()).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null)),
            ("submittedBy".into(), Value::Null),
            ("reviewedBy".into(), Value::Null),
        ]));

        if meaning.body.is_empty() {
            return Ok(None);
        }

        Ok(Some(MeaningResult {
            load_type: "meaning".into(),
            data: MeaningData {
                title: meaning.title.or(og_title),
                description: og_description,
                paragraphs: meaning.body,
                translation: translated,
                url: Some(murl),
                data_type: Some("track".into()),
                meaning_meta: Some(meaning_meta),
                provider: "letrasmus".into(),
                song,
            },
        }))
    }
}
