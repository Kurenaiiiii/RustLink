use async_trait::async_trait;
use serde_json::json;

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::tracks::TrackInfo;

pub struct StubSource {
    name: &'static str,
}

impl StubSource {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl SourceProvider for StubSource {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn search(&self, _query: &str, _search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn resolve(&self, _query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        Ok(SourceResult::Empty)
    }

    async fn get_track_url(&self, _track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: json!({}),
            new_track: None,
            additional_data: json!({}),
            exception: Some(format!("{} source not fully implemented yet", self.name)),
        })
    }
}
