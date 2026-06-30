use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SponsorBlockSegment {
    pub segment: [f64; 2],
    pub category: String,
    #[serde(rename = "UUID")]
    pub uuid: String,
    #[serde(default)]
    pub _video_duration: f64,
}

impl SponsorBlockSegment {
    pub fn start(&self) -> f64 {
        self.segment[0]
    }
    pub fn end(&self) -> f64 {
        self.segment[1]
    }
}

pub async fn fetch_segments(
    video_id: &str,
    categories: &[String],
) -> anyhow::Result<Vec<SponsorBlockSegment>> {
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let categories_str = categories.join(",");
    let url = format!(
        "https://sponsor.ajay.app/api/skipSegments?videoID={}&categories={}",
        urlencoding(video_id),
        urlencoding(&categories_str)
    );
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let segments: Vec<SponsorBlockSegment> = resp.json().await?;
    Ok(segments)
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
