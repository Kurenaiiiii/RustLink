use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct RateLimitRule {
    pub max_requests: u32,
    pub time_window_ms: u64,
}

impl Default for RateLimitRule {
    fn default() -> Self {
        Self {
            max_requests: 100,
            time_window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub global: RateLimitRule,
    pub per_ip: RateLimitRule,
    pub per_user_id: Option<RateLimitRule>,
    pub per_guild_id: Option<RateLimitRule>,
    pub ignore_paths: Vec<String>,
    pub ignore: IgnoreConfig,
    pub trust_proxy: bool,
    pub max_entries: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            global: RateLimitRule {
                max_requests: 1000,
                time_window_ms: 60_000,
            },
            per_ip: RateLimitRule {
                max_requests: 100,
                time_window_ms: 10_000,
            },
            per_user_id: Some(RateLimitRule {
                max_requests: 50,
                time_window_ms: 5_000,
            }),
            per_guild_id: Some(RateLimitRule {
                max_requests: 20,
                time_window_ms: 5_000,
            }),
            ignore_paths: Vec::new(),
            ignore: IgnoreConfig::default(),
            trust_proxy: false,
            max_entries: 10_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreConfig {
    pub user_ids: Vec<String>,
    pub guild_ids: Vec<String>,
    pub ips: Vec<String>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub reset_ms: u64,
}

struct RateLimitEntry {
    requests: Vec<Instant>,
    last_seen: Instant,
}

pub struct RateLimitManager {
    config: RateLimitConfig,
    store: Arc<Mutex<HashMap<String, RateLimitEntry>>>,
}

impl RateLimitManager {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn check(
        &self,
        pathname: &str,
        remote_addr: Option<&str>,
        user_id: Option<&str>,
        guild_id: Option<&str>,
    ) -> RateLimitResult {
        if !self.config.enabled {
            return RateLimitResult {
                allowed: true,
                limit: 0,
                remaining: 0,
                reset_ms: 0,
            };
        }

        if self.is_ignored_path(pathname) {
            return RateLimitResult {
                allowed: true,
                limit: 0,
                remaining: 0,
                reset_ms: 0,
            };
        }

        if self.should_ignore(pathname, remote_addr, user_id, guild_id) {
            return RateLimitResult {
                allowed: true,
                limit: 0,
                remaining: 0,
                reset_ms: 0,
            };
        }

        let now = Instant::now();
        let mut store = self.store.lock().await;

        let mut best_result: Option<RateLimitResult> = None;

        // Global check
        let global_result = self.check_scope(&mut store, "global", "", &self.config.global, now);
        if !global_result.allowed {
            return global_result;
        }
        best_result = Self::pick_best(best_result, global_result);

        // Per-IP check
        if let Some(ip) = remote_addr {
            let ip_result = self.check_scope(&mut store, "ip", ip, &self.config.per_ip, now);
            if !ip_result.allowed {
                return ip_result;
            }
            best_result = Self::pick_best(best_result, ip_result);
        }

        // Per-user check
        if let Some(uid) = user_id {
            if let Some(ref rule) = self.config.per_user_id {
                let user_result = self.check_scope(&mut store, "userId", uid, rule, now);
                if !user_result.allowed {
                    return user_result;
                }
                best_result = Self::pick_best(best_result, user_result);
            }
        }

        // Per-guild check
        if let Some(gid) = guild_id {
            if let Some(ref rule) = self.config.per_guild_id {
                let guild_result = self.check_scope(&mut store, "guildId", gid, rule, now);
                if !guild_result.allowed {
                    return guild_result;
                }
                best_result = Self::pick_best(best_result, guild_result);
            }
        }

        best_result.unwrap_or(RateLimitResult {
            allowed: true,
            limit: 0,
            remaining: 0,
            reset_ms: 0,
        })
    }

    fn check_scope(
        &self,
        store: &mut HashMap<String, RateLimitEntry>,
        scope: &str,
        id: &str,
        rule: &RateLimitRule,
        now: Instant,
    ) -> RateLimitResult {
        let key = format!("{}:{}", scope, id);
        let window = Duration::from_millis(rule.time_window_ms);

        let entry = store.entry(key).or_insert(RateLimitEntry {
            requests: Vec::new(),
            last_seen: now,
        });

        // Prune expired entries
        entry.requests.retain(|t| now.duration_since(*t) <= window);
        let active = entry.requests.len() as u32;

        let first_request = entry.requests.first().copied();
        let reset_ms = match first_request {
            Some(t) => {
                let elapsed = now.duration_since(t).as_millis() as u64;
                rule.time_window_ms.saturating_sub(elapsed)
            }
            None => rule.time_window_ms,
        };

        if active >= rule.max_requests {
            return RateLimitResult {
                allowed: false,
                limit: rule.max_requests,
                remaining: 0,
                reset_ms,
            };
        }

        entry.requests.push(now);
        entry.last_seen = now;

        RateLimitResult {
            allowed: true,
            limit: rule.max_requests,
            remaining: rule.max_requests.saturating_sub(active + 1),
            reset_ms,
        }
    }

    fn pick_best(current: Option<RateLimitResult>, candidate: RateLimitResult) -> Option<RateLimitResult> {
        match current {
            None => Some(candidate),
            Some(curr) => {
                if candidate.remaining < curr.remaining {
                    Some(candidate)
                } else if candidate.remaining == curr.remaining && candidate.reset_ms < curr.reset_ms {
                    Some(candidate)
                } else {
                    Some(curr)
                }
            }
        }
    }

    fn is_ignored_path(&self, pathname: &str) -> bool {
        self.config.ignore_paths.iter().any(|p| pathname.starts_with(p))
    }

    fn should_ignore(
        &self,
        pathname: &str,
        remote_addr: Option<&str>,
        user_id: Option<&str>,
        guild_id: Option<&str>,
    ) -> bool {
        let ignore = &self.config.ignore;
        if let Some(ip) = remote_addr {
            if ignore.ips.iter().any(|i| i == ip) {
                return true;
            }
        }
        if let Some(uid) = user_id {
            if ignore.user_ids.iter().any(|u| u == uid) {
                return true;
            }
        }
        if let Some(gid) = guild_id {
            if ignore.guild_ids.iter().any(|g| g == gid) {
                return true;
            }
        }
        ignore.paths.iter().any(|p| pathname.starts_with(p))
    }

    pub async fn clear(&self) {
        self.store.lock().await.clear();
    }

    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut store = self.store.lock().await;
        let max_window = self.max_window_ms();
        let prune_after = Duration::from_millis(max_window * 3);

        store.retain(|_, entry| {
            let active = entry
                .requests
                .iter()
                .filter(|t| now.duration_since(**t) <= Duration::from_millis(max_window))
                .count();
            active > 0 || now.duration_since(entry.last_seen) <= prune_after
        });

        // Enforce max entries
        let max_entries = self.config.max_entries.max(100);
        if store.len() > max_entries {
            let mut entries: Vec<(String, Instant)> = store
                .iter()
                .map(|(k, v)| (k.clone(), v.last_seen))
                .collect();
            entries.sort_by(|a, b| a.1.cmp(&b.1));
            let to_remove = store.len() - max_entries;
            for (key, _) in entries.iter().take(to_remove) {
                store.remove(key);
            }
        }
    }

    fn max_window_ms(&self) -> u64 {
        let mut max = self.config.global.time_window_ms;
        max = max.max(self.config.per_ip.time_window_ms);
        if let Some(ref rule) = self.config.per_user_id {
            max = max.max(rule.time_window_ms);
        }
        if let Some(ref rule) = self.config.per_guild_id {
            max = max.max(rule.time_window_ms);
        }
        max
    }
}
