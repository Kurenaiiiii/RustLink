use crate::lyrics::LyricsData;

pub async fn fetch_letrasmus(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let url = format!(
        "https://www.letras.mus.br/{}/{}/",
        slugify(artist),
        slugify(title)
    );

    match client.get(&url).send().await {
        Ok(resp) if resp.status() == reqwest::StatusCode::OK => {
            let html = resp.text().await?;

            let plain = extract_lyrics_text(&html).ok_or_else(|| anyhow::anyhow!("No lyrics article found"))?;
            let song_title = extract_meta(&html, "og:title").unwrap_or_else(|| title.to_string());
            let song_artist = extract_meta(&html, "music:musician_description")
                .or_else(|| extract_meta(&html, "og:description"))
                .unwrap_or_else(|| artist.to_string());

            Ok(Some(LyricsData {
                source: "letrasmus".into(),
                title: song_title,
                artist: song_artist,
                album: None,
                synced_lyrics: Vec::new(),
                plain_lyrics: if plain.is_empty() { None } else { Some(plain) },
            }))
        }
        _ => Ok(None),
    }
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' | '_' => c,
            ' ' => '-',
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn extract_lyrics_text(html: &str) -> Option<String> {
    let marker = r#"<article>"#;
    let end_marker = r#"</article>"#;
    let start = html.find(marker)?;
    let end = html[start..].find(end_marker)?;
    let content = &html[start..start + end];

    let mut text = String::new();
    let mut in_tag = false;
    let mut in_br = false;
    for ch in content.chars() {
        match ch {
            '<' => {
                in_tag = true;
                in_br = false;
            }
            '>' => {
                in_tag = false;
                if in_br {
                    text.push('\n');
                }
                in_br = false;
            }
            _ if !in_tag => {
                if ch == '\n' || ch == '\r' {
                    continue;
                }
                text.push(ch);
                in_br = ch == 'b' || ch == 'r';
            }
            _ => {
                in_br = (ch == 'b' || ch == 'r' || ch == '/') && in_br;
            }
        }
    }

    let text = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    Some(text)
}

fn extract_meta(html: &str, property: &str) -> Option<String> {
    let pattern = format!(r#"property="{}""#, property);
    let start = html.find(&pattern)?;
    let rest = &html[start..];
    let content_start = rest.find(r#"content=""#)?;
    let content_start = content_start + 9;
    let content_end = rest[content_start..].find('"')?;
    Some(rest[content_start..content_start + content_end].to_string())
}
