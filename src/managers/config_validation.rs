use crate::config::NodeLinkConfig;

pub struct ConfigValidationManager;

#[derive(Debug)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ConfigValidationManager {
    pub fn validate(config: &NodeLinkConfig) -> ValidationResult {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Server config
        if config.server.host.is_empty() {
            errors.push("server.host must not be empty".into());
        }
        if config.server.port == 0 || config.server.port > 65535 {
            errors.push("server.port must be between 1 and 65535".into());
        }
        if config.server.password.is_empty() {
            errors.push("server.password must not be empty".into());
        }

        // Sources
        if config.sources.youtube.enabled {
            if !config.sources.youtube.hl.is_empty()
                && config.sources.youtube.hl.len() != 2
            {
                warnings.push("youtube.hl should be a 2-letter language code (e.g. 'en')".into());
            }
            if !config.sources.youtube.gl.is_empty()
                && config.sources.youtube.gl.len() != 2
            {
                warnings.push("youtube.gl should be a 2-letter country code (e.g. 'US')".into());
            }
        }

        if config.sources.spotify.enabled {
            if config.sources.spotify.client_id.is_empty() {
                errors.push("spotify.client_id is required when spotify is enabled".into());
            }
            if config.sources.spotify.client_secret.is_empty() {
                errors.push("spotify.client_secret is required when spotify is enabled".into());
            }
        }

        if config.sources.deezer.enabled && config.sources.deezer.arl.as_ref().map_or(true, |s| s.is_empty()) {
            warnings.push("deezer.arl is empty — Deezer may not work".into());
        }

        if config.sources.yandexmusic.enabled && config.sources.yandexmusic.access_token.as_ref().map_or(true, |s| s.is_empty()) {
            warnings.push("yandexmusic.access_token is empty".into());
        }

        // Clustering
        if config.cluster.enabled {
            if config.cluster.redis_url.is_none() || config.cluster.redis_url.as_ref().map_or(true, |u| u.is_empty()) {
                errors.push("cluster.redis_url is required when clustering is enabled".into());
            }
            if config.cluster.heartbeat_interval_secs == 0 {
                warnings.push("cluster.heartbeat_interval_secs should be > 0".into());
            }
        }

        // Rate limiting
        if config.rate_limit.enabled {
            if config.rate_limit.max_requests == 0 {
                errors.push("rate_limit.max_requests must be > 0".into());
            }
            if config.rate_limit.window_ms == 0 {
                errors.push("rate_limit.window_ms must be > 0".into());
            }
        }

        // Metrics
        if config.metrics.enabled
            && config.metrics.password.is_some()
            && config.metrics.password.as_deref() == Some("")
        {
            warnings.push("metrics.password is set but empty".into());
        }

        // Plugin paths
        if config.plugins.enabled {
            for path in &config.plugins.paths {
                if path.is_empty() {
                    errors.push("plugin path must not be empty".into());
                }
            }
        }

        // Cache
        if config.cache_encryption_key.is_some() {
            let key = config.cache_encryption_key.as_ref().unwrap();
            if key.len() != 44 {
                warnings.push("cache_encryption_key should be a 44-character base64-encoded 32-byte key".into());
            }
        }

        // Local files
        if config.sources.local.enabled && config.sources.local.base_path.is_empty() {
            errors.push("local.base_path is required when local provider is enabled".into());
        }

        ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings,
        }
    }

    pub fn validate_and_report(config: &NodeLinkConfig) {
        let result = Self::validate(config);
        for err in &result.errors {
            tracing::error!(target: "ConfigValidation", "{}", err);
        }
        for warn in &result.warnings {
            tracing::warn!(target: "ConfigValidation", "{}", warn);
        }
        if !result.valid {
            tracing::error!(target: "ConfigValidation", "Configuration validation FAILED — {} error(s)", result.errors.len());
        }
    }
}
