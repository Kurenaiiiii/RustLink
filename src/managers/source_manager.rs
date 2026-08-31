use std::sync::Arc;

use crate::plugins::PluginManager;
use crate::sources::{SourceRegistry, SourceResult, TrackUrlResult};
use crate::tracks::{Chapter, TrackInfo};

/// Wraps SourceRegistry with route planner integration and enhanced error handling.
pub struct SourceManager {
    registry: SourceRegistry,
    plugin_manager: Option<Arc<PluginManager>>,
}

impl SourceManager {
    pub fn new(registry: SourceRegistry) -> Self {
        Self {
            registry,
            plugin_manager: None,
        }
    }

    pub fn with_plugin_manager(mut self, pm: Option<Arc<PluginManager>>) -> Self {
        self.plugin_manager = pm;
        self
    }

    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    pub async fn search(&self, source: &str, query: &str) -> anyhow::Result<SourceResult> {
        // Determine search type from query format (e.g. "ytsearch:..." or bare query)
        let search_type = if query.contains(':') {
            query.split(':').next().unwrap_or("search")
        } else {
            "search"
        };
        if let Some(pm) = &self.plugin_manager {
            pm.on_search(query, source, search_type).await;
        }
        self.registry.search(source, query).await
    }

    pub async fn search_with_default(&self, default_source: &str, query: &str) -> anyhow::Result<SourceResult> {
        self.registry.search_with_default(default_source, query).await
    }

    pub async fn resolve(&self, query: &str) -> anyhow::Result<SourceResult> {
        if let Some(pm) = &self.plugin_manager {
            pm.on_resolve(query, "unknown").await;
        }
        self.registry.resolve(query).await
    }

    pub async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        self.registry.get_track_url(track).await
    }

    pub async fn get_chapters(&self, track: &TrackInfo) -> anyhow::Result<Vec<Chapter>> {
        self.registry.get_chapters(track).await
    }

    pub async fn source_names(&self) -> Vec<String> {
        self.registry.source_names().await
    }
}
