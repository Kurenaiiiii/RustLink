use crate::cache::TrackCache;

use std::sync::Arc;

pub struct TrackCacheManager {
    cache: Arc<TrackCache>,
}

impl TrackCacheManager {
    pub fn new(cache: TrackCache) -> Self {
        Self { cache: Arc::new(cache) }
    }

    pub fn inner(&self) -> &TrackCache {
        &self.cache
    }

    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.cache.get(key).await
    }

    pub async fn set(&self, key: String, data: serde_json::Value) {
        self.cache.set(key, data).await
    }

    pub async fn invalidate(&self, key: &str) {
        self.cache.invalidate(key).await
    }

    pub async fn clear(&self) {
        self.cache.clear().await
    }
}
