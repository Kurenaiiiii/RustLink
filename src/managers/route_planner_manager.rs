use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlannerStatus {
    pub ip_block_type: String,
    pub ip_block_size: String,
    pub rotating: bool,
    pub current_index: usize,
    pub addresses: Vec<String>,
    pub failing: HashMap<String, String>,
    pub blocked: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum RoutePlannerStrategy {
    RoundRobin,
    RotateOnBan,
    LoadBalance,
}

impl Default for RoutePlannerStrategy {
    fn default() -> Self {
        RoutePlannerStrategy::RotateOnBan
    }
}

struct IpBlock {
    cidr: String,
    base_int: u128,
    size: u128,
    last_offset: u128,
    is_ipv6: bool,
}

pub struct RoutePlannerManager {
    blocks: Arc<Mutex<Vec<IpBlock>>>,
    banned_ips: Arc<Mutex<HashMap<String, Instant>>>,
    banned_blocks: Arc<Mutex<HashMap<String, Instant>>>,
    strategy: RoutePlannerStrategy,
    current_block: Arc<Mutex<usize>>,
    ban_cooldown_ms: u64,
}

impl Default for RoutePlannerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutePlannerManager {
    pub fn new() -> Self {
        Self {
            blocks: Arc::new(Mutex::new(Vec::new())),
            banned_ips: Arc::new(Mutex::new(HashMap::new())),
            banned_blocks: Arc::new(Mutex::new(HashMap::new())),
            strategy: RoutePlannerStrategy::RotateOnBan,
            current_block: Arc::new(Mutex::new(0)),
            ban_cooldown_ms: 600_000,
        }
    }

    pub fn with_strategy(mut self, strategy: RoutePlannerStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    pub fn with_ban_cooldown(mut self, cooldown_ms: u64) -> Self {
        self.ban_cooldown_ms = cooldown_ms;
        self
    }

    pub async fn add_block(&self, cidr: &str) -> Result<(), String> {
        let (base, mask_len) = cidr
            .split_once('/')
            .ok_or_else(|| format!("Invalid CIDR: {}", cidr))?;

        let mask_len: u32 = mask_len
            .parse()
            .map_err(|_| format!("Invalid mask length: {}", mask_len))?;

        let is_ipv6 = base.contains(':');
        let total_bits = if is_ipv6 { 128 } else { 32 };

        if mask_len > total_bits {
            return Err(format!("Mask length {} exceeds {} bits", mask_len, total_bits));
        }

        let base_int = self.ip_to_u128(base)?;
        let mask = if mask_len == 0 {
            0u128
        } else {
            let shift = total_bits - mask_len;
            (u128::MAX << shift) & (if is_ipv6 { u128::MAX } else { 0xFFFF_FFFFu128 })
        };
        let network_int = base_int & mask;
        let size = 1u128 << (total_bits - mask_len);

        let mut blocks = self.blocks.lock().await;
        blocks.push(IpBlock {
            cidr: cidr.to_string(),
            base_int: network_int,
            size,
            last_offset: 0,
            is_ipv6,
        });

        Ok(())
    }

    pub async fn get_ip(&self) -> Option<String> {
        let now = Instant::now();
        let mut blocks = self.blocks.lock().await;
        let banned_blocks = self.banned_blocks.lock().await;
        let banned_ips = self.banned_ips.lock().await;

        if blocks.is_empty() {
            return None;
        }

        match self.strategy {
            RoutePlannerStrategy::RoundRobin | RoutePlannerStrategy::RotateOnBan => {
                let start_block = {
                    let mut current = self.current_block.lock().await;
                    *current = (*current + 1) % blocks.len();
                    *current
                };

                for i in 0..blocks.len() {
                    let idx = (start_block + i) % blocks.len();
                    let block = &mut blocks[idx];

                    if let Some(expires) = banned_blocks.get(&block.cidr) {
                        if *expires > now {
                            continue;
                        }
                    }

                    for _attempt in 0..10 {
                        block.last_offset = (block.last_offset + 1) % block.size;
                        let ip_int = block.base_int + block.last_offset;
                        let ip = self.u128_to_ip(ip_int, block.is_ipv6);
                        let is_banned = banned_ips
                            .get(&ip)
                            .map(|exp| *exp > now)
                            .unwrap_or(false);
                        if !is_banned {
                            return Some(ip);
                        }
                    }
                }
                None
            }
            RoutePlannerStrategy::LoadBalance => {
                use rand::Rng;
                let available: Vec<usize> = (0..blocks.len())
                    .filter(|&i| {
                        banned_blocks
                            .get(&blocks[i].cidr)
                            .map(|exp| *exp <= now)
                            .unwrap_or(true)
                    })
                    .collect();

                if available.is_empty() {
                    return None;
                }

                let mut rng = rand::thread_rng();
                let idx = available[rng.gen_range(0..available.len())];
                let block = &mut blocks[idx];

                let offset = rng.gen_range(0..block.size.min(u128::MAX as u128));
                let ip_int = block.base_int + offset;
                let ip = self.u128_to_ip(ip_int, block.is_ipv6);

                let is_banned = banned_ips
                    .get(&ip)
                    .map(|exp| *exp > now)
                    .unwrap_or(false);

                if is_banned { None } else { Some(ip) }
            }
        }
    }

    pub async fn ban_ip(&self, ip: &str) {
        let now = Instant::now();
        let mut banned = self.banned_ips.lock().await;
        banned.insert(ip.to_string(), now + Duration::from_millis(self.ban_cooldown_ms));

        // Check if we should ban the whole block
        if let Ok(ip_int) = self.ip_to_u128(ip) {
            let blocks = self.blocks.lock().await;
            if let Some(block) = blocks.iter().find(|b| {
                ip_int >= b.base_int && ip_int < b.base_int + b.size
            }) {
                let mut failed_count = 0;
                for banned_ip in banned.keys() {
                    if let Ok(bip) = self.ip_to_u128(banned_ip) {
                        if bip >= block.base_int && bip < block.base_int + block.size {
                            failed_count += 1;
                        }
                    }
                }
                if failed_count >= 5 {
                    let mut banned_blocks = self.banned_blocks.lock().await;
                    banned_blocks.insert(
                        block.cidr.clone(),
                        now + Duration::from_millis(self.ban_cooldown_ms * 2),
                    );
                }
            }
        }
    }

    pub async fn free_ip(&self, ip: &str) {
        self.banned_ips.lock().await.remove(ip);
    }

    pub async fn free_all(&self) {
        self.banned_ips.lock().await.clear();
        self.banned_blocks.lock().await.clear();
    }

    pub async fn status(&self) -> RoutePlannerStatus {
        let blocks = self.blocks.lock().await;
        let banned_ips = self.banned_ips.lock().await;
        let current = *self.current_block.lock().await;

        RoutePlannerStatus {
            ip_block_type: if blocks.first().map(|b| b.is_ipv6).unwrap_or(false) {
                "Inet6Address".into()
            } else {
                "Inet4Address".into()
            },
            ip_block_size: blocks.len().to_string(),
            rotating: false,
            current_index: current,
            addresses: blocks.iter().map(|b| b.cidr.clone()).collect(),
            failing: banned_ips
                .iter()
                .map(|(k, _)| (k.clone(), "failing".into()))
                .collect(),
            blocked: self
                .banned_blocks
                .lock()
                .await
                .keys()
                .cloned()
                .collect(),
        }
    }

    fn ip_to_u128(&self, ip: &str) -> Result<u128, String> {
        if ip.contains(':') {
            self.ipv6_to_u128(ip)
        } else {
            self.ipv4_to_u128(ip)
        }
    }

    fn ipv4_to_u128(&self, ip: &str) -> Result<u128, String> {
        let octets: Vec<&str> = ip.split('.').collect();
        if octets.len() != 4 {
            return Err(format!("Invalid IPv4 address: {}", ip));
        }
        let mut result: u128 = 0;
        for octet in octets {
            let val: u32 = octet
                .parse()
                .map_err(|_| format!("Invalid IPv4 address: {}", ip))?;
            if val > 255 {
                return Err(format!("Invalid IPv4 address: {}", ip));
            }
            result = (result << 8) | val as u128;
        }
        Ok(result)
    }

    fn ipv6_to_u128(&self, ip: &str) -> Result<u128, String> {
        let expanded = self.expand_ipv6(ip)?;
        let groups: Vec<&str> = expanded.split(':').collect();
        if groups.len() != 8 {
            return Err(format!("Invalid IPv6 address: {}", ip));
        }
        let mut result: u128 = 0;
        for group in groups {
            let val = u16::from_str_radix(group, 16)
                .map_err(|_| format!("Invalid IPv6 address: {}", ip))?;
            result = (result << 16) | val as u128;
        }
        Ok(result)
    }

    fn expand_ipv6(&self, ip: &str) -> Result<String, String> {
        if ip.contains("::") {
            let parts: Vec<&str> = ip.split("::").collect();
            if parts.len() > 2 {
                return Err(format!("Invalid IPv6 address: {}", ip));
            }
            let left: Vec<&str> = if parts[0].is_empty() {
                Vec::new()
            } else {
                parts[0].split(':').collect()
            };
            let right: Vec<&str> = if parts.len() > 1 && !parts[1].is_empty() {
                parts[1].split(':').collect()
            } else {
                Vec::new()
            };
            let missing = 8 - (left.len() + right.len());
            let mut groups = left.clone();
            for _ in 0..missing {
                groups.push("0");
            }
            groups.extend(right);
            Ok(groups.join(":"))
        } else {
            Ok(ip.to_string())
        }
    }

    fn u128_to_ip(&self, value: u128, is_ipv6: bool) -> String {
        if is_ipv6 {
            let mut parts = Vec::new();
            for i in 0..8 {
                let shift = (7 - i) * 16;
                let val = ((value >> shift) & 0xFFFF) as u16;
                parts.push(format!("{:x}", val));
            }
            parts.join(":")
        } else {
            let mut parts = Vec::new();
            for i in 0..4 {
                let shift = (3 - i) * 8;
                let val = ((value >> shift) & 0xFF) as u8;
                parts.push(val.to_string());
            }
            parts.join(".")
        }
    }
}
