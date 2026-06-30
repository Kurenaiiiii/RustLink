use crate::lyrics::{LyricsData, parse_lrc, urlencoding};

pub async fn fetch_bilibili(
    client: &reqwest::Client,
    title: &str,
    _artist: &str,
) -> anyhow::Result<Option<LyricsData>> {
    let search_url = format!(
        "https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={}",
        urlencoding(title)
    );
    let resp = client.get(&search_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let data: serde_json::Value = resp.json().await?;
    let videos = &data["data"]["result"];
    let bvid = videos[0]["bvid"].as_str();
    let bvid = match bvid {
        Some(id) => id.to_string(),
        None => return Ok(None),
    };

    // Get video info to find cid
    let info_url = format!("https://api.bilibili.com/x/web-interface/view?bvid={}", bvid);
    let resp = client.get(&info_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let info: serde_json::Value = resp.json().await?;
    let cid = info["data"]["cid"].as_u64();
    let cid = match cid {
        Some(id) => id,
        None => return Ok(None),
    };

    // Get CC/subtitle data
    let sub_url = format!(
        "https://api.bilibili.com/x/web-interface/view?bvid={}&cid={}",
        bvid, cid
    );
    let resp = client.get(&sub_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let sub_info: serde_json::Value = resp.json().await?;
    let subtitles = sub_info["data"]["subtitle"]["subtitles"].as_array();

    let subtitle_url = subtitles
        .and_then(|arr| arr.first())
        .and_then(|s| s["subtitle_url"].as_str())
        .map(|s| if s.starts_with("//") { format!("https:{}", s) } else { s.to_string() });

    let sub_url = match subtitle_url {
        Some(url) => url,
        None => return Ok(None),
    };

    let resp = client.get(&sub_url).send().await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    let sub_body: serde_json::Value = resp.json().await?;
    let bodies = sub_body["body"].as_array();

    let synced: Vec<crate::lyrics::SyncedLyricLine> = bodies
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let from = item["from"].as_f64()?;
                    let content = item["content"].as_str()?;
                    Some(crate::lyrics::SyncedLyricLine {
                        time: from,
                        text: content.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let plain = synced.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n");

    Ok(Some(LyricsData {
        source: "bilibili".into(),
        title: title.to_string(),
        artist: String::new(),
        album: None,
        synced_lyrics: synced,
        plain_lyrics: if plain.is_empty() { None } else { Some(plain) },
    }))
}
