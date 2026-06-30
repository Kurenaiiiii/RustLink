use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct DosProtectionManager {
    inner: std::sync::Arc<Mutex<DosState>>,
    config: DosConfig,
}

#[derive(Clone)]
pub struct DosConfig {
    pub enabled: bool,
    pub max_requests_per_second: u32,
    pub ban_duration_ms: u64,
    pub burst_multiplier: u32,
}

impl Default for DosConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_requests_per_second: 50,
            ban_duration_ms: 60000,
            burst_multiplier: 3,
        }
    }
}

struct DosState {
    ip_requests: HashMap<String, Vec<Instant>>,
    banned_ips: HashMap<String, Instant>,
    global_requests: Vec<Instant>,
}

impl DosProtectionManager {
    pub fn new(config: DosConfig) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(DosState {
                ip_requests: HashMap::new(),
                banned_ips: HashMap::new(),
                global_requests: Vec::new(),
            })),
            config,
        }
    }

    pub fn check(&self, ip: &str) -> bool {
        if !self.config.enabled {
            return true;
        }

        let mut state = self.inner.lock().unwrap();
        let now = Instant::now();

        // Clean expired bans
        state.banned_ips.retain(|_, exp| *exp > now);

        // Check if banned
        if state.banned_ips.contains_key(ip) {
            return false;
        }

        // Clean old entries
        let window = Duration::from_secs(1);
        state
            .ip_requests
            .entry(ip.to_string())
            .or_default()
            .retain(|t| now.duration_since(*t) <= window);
        state.global_requests.retain(|t| now.duration_since(*t) <= window);

        let ip_count = state.ip_requests.get(ip).map(|v| v.len()).unwrap_or(0);
        let global_count = state.global_requests.len();

        let max_per_ip = self.config.max_requests_per_second as usize;
        let global_max = max_per_ip * self.config.burst_multiplier as usize;

        if ip_count >= max_per_ip || global_count >= global_max {
            if ip_count >= max_per_ip * 2 {
                state.banned_ips.insert(
                    ip.to_string(),
                    now + Duration::from_millis(self.config.ban_duration_ms),
                );
            }
            return false;
        }

        state
            .ip_requests
            .entry(ip.to_string())
            .or_default()
            .push(now);
        state.global_requests.push(now);

        true
    }

    pub fn is_banned(&self, ip: &str) -> bool {
        let state = self.inner.lock().unwrap();
        state
            .banned_ips
            .get(ip)
            .map(|exp| *exp > Instant::now())
            .unwrap_or(false)
    }

    pub fn unban(&self, ip: &str) {
        let mut state = self.inner.lock().unwrap();
        state.banned_ips.remove(ip);
    }

    pub fn metrics(&self) -> DosMetrics {
        let state = self.inner.lock().unwrap();
        DosMetrics {
            active_bans: state.banned_ips.len(),
            tracked_ips: state.ip_requests.len(),
            total_recent: state.global_requests.len(),
        }
    }
}

pub struct DosMetrics {
    pub active_bans: usize,
    pub tracked_ips: usize,
    pub total_recent: usize,
}
