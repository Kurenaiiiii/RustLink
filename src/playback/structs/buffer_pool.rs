use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_POOL_SIZE_BYTES: usize = 20 * 1024 * 1024;
const MAX_BUCKET_ENTRIES: usize = 4;
const IDLE_CLEAR_MS: u64 = 60_000;
const CLEANUP_INTERVAL_MS: u64 = 60_000;

struct BufferPoolInner {
    pools: HashMap<usize, Vec<Vec<u8>>>,
    total_bytes: usize,
    acquire_calls: u64,
    reuse_hits: u64,
    new_allocs: u64,
    release_calls: u64,
    rejected_releases: u64,
    clear_calls: u64,
    high_water_bytes: usize,
    last_activity_at: Instant,
}

impl BufferPoolInner {
    fn new() -> Self {
        Self {
            pools: HashMap::new(),
            total_bytes: 0,
            acquire_calls: 0,
            reuse_hits: 0,
            new_allocs: 0,
            release_calls: 0,
            rejected_releases: 0,
            clear_calls: 0,
            high_water_bytes: 0,
            last_activity_at: Instant::now(),
        }
    }

    fn get_aligned_size(size: usize) -> usize {
        if size <= 1024 {
            return 1024;
        }
        let mut n = size - 1;
        n |= n >> 1;
        n |= n >> 2;
        n |= n >> 4;
        n |= n >> 8;
        n |= n >> 16;
        #[cfg(target_pointer_width = "64")]
        {
            n |= n >> 32;
        }
        n + 1
    }

    fn cleanup_if_needed(&mut self) {
        let now = Instant::now();
        let idle_duration = now.duration_since(self.last_activity_at);

        if self.total_bytes > 0 && idle_duration >= Duration::from_millis(IDLE_CLEAR_MS) {
            self.pools.clear();
            self.total_bytes = 0;
            return;
        }

        if self.total_bytes > MAX_POOL_SIZE_BYTES {
            let mut sizes: Vec<usize> = self.pools.keys().cloned().collect();
            sizes.sort_by(|a, b| b.cmp(a));

            for size in sizes {
                if self.total_bytes <= MAX_POOL_SIZE_BYTES {
                    break;
                }
                if let Some(bucket) = self.pools.get(&size) {
                    if !bucket.is_empty() {
                        self.total_bytes -= size * bucket.len();
                        self.pools.remove(&size);
                    }
                }
            }

            if self.total_bytes > MAX_POOL_SIZE_BYTES {
                self.pools.clear();
                self.total_bytes = 0;
            }
        }
    }

    pub fn acquire(&mut self, size: usize) -> Vec<u8> {
        self.last_activity_at = Instant::now();
        self.acquire_calls += 1;

        let aligned_size = Self::get_aligned_size(size);

        if let Some(pool) = self.pools.get_mut(&aligned_size) {
            if let Some(buffer) = pool.pop() {
                self.reuse_hits += 1;
                self.total_bytes -= aligned_size;
                return buffer;
            }
        }

        self.new_allocs += 1;
        vec![0u8; aligned_size]
    }

    pub fn release(&mut self, mut buffer: Vec<u8>) {
        self.last_activity_at = Instant::now();
        self.release_calls += 1;

        if buffer.len() < 1024 || buffer.len() > 10 * 1024 * 1024 {
            self.rejected_releases += 1;
            return;
        }

        if self.total_bytes + buffer.len() > MAX_POOL_SIZE_BYTES * 3 / 4 {
            self.cleanup_if_needed();
        }

        if self.total_bytes + buffer.len() > MAX_POOL_SIZE_BYTES {
            self.rejected_releases += 1;
            return;
        }

        let pool = self.pools.entry(buffer.len()).or_default();
        if pool.len() >= MAX_BUCKET_ENTRIES {
            self.rejected_releases += 1;
            return;
        }

        buffer.clear();
        let cap = buffer.capacity();
        pool.push(buffer);
        self.total_bytes += cap;
        if self.total_bytes > self.high_water_bytes {
            self.high_water_bytes = self.total_bytes;
        }
    }

    pub fn clear(&mut self) {
        self.last_activity_at = Instant::now();
        self.clear_calls += 1;
        self.pools.clear();
        self.total_bytes = 0;
    }

    pub fn get_stats(&self) -> BufferPoolStats {
        let mut entries = 0;
        let mut top_buckets: Vec<BucketStats> = self
            .pools
            .iter()
            .map(|(size, list)| BucketStats {
                size: *size,
                count: list.len(),
                bytes: size * list.len(),
            })
            .collect();

        top_buckets.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        for b in &top_buckets {
            entries += b.count;
        }

        let reuse_ratio = if self.acquire_calls > 0 {
            self.reuse_hits as f64 / self.acquire_calls as f64
        } else {
            0.0
        };

        let _rejection_rate = if self.release_calls > 0 {
            self.rejected_releases as f64 / self.release_calls as f64
        } else {
            0.0
        };

        BufferPoolStats {
            total_bytes: self.total_bytes,
            high_water_bytes: self.high_water_bytes,
            buckets: self.pools.len(),
            entries,
            acquire_calls: self.acquire_calls,
            reuse_hits: self.reuse_hits,
            new_allocs: self.new_allocs,
            release_calls: self.release_calls,
            rejected_releases: self.rejected_releases,
            clear_calls: self.clear_calls,
            reuse_ratio,
            top_buckets: top_buckets.into_iter().take(20).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BucketStats {
    pub size: usize,
    pub count: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    pub total_bytes: usize,
    pub high_water_bytes: usize,
    pub buckets: usize,
    pub entries: usize,
    pub acquire_calls: u64,
    pub reuse_hits: u64,
    pub new_allocs: u64,
    pub release_calls: u64,
    pub rejected_releases: u64,
    pub clear_calls: u64,
    pub reuse_ratio: f64,
    pub top_buckets: Vec<BucketStats>,
}

pub struct BufferPool {
    inner: Arc<Mutex<BufferPoolInner>>,
}

impl BufferPool {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(BufferPoolInner::new()));

        let cleanup_inner = Arc::clone(&inner);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(CLEANUP_INTERVAL_MS));
                cleanup_inner.lock().unwrap().cleanup_if_needed();
            }
        });

        Self { inner }
    }

    pub fn acquire(&self, size: usize) -> Vec<u8> {
        self.inner.lock().unwrap().acquire(size)
    }

    pub fn release(&self, buffer: Vec<u8>) {
        self.inner.lock().unwrap().release(buffer);
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn get_stats(&self) -> BufferPoolStats {
        self.inner.lock().unwrap().get_stats()
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_global_pool() -> &'static BufferPool {
    static POOL: std::sync::OnceLock<BufferPool> = std::sync::OnceLock::new();
    POOL.get_or_init(BufferPool::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_release() {
        let pool = BufferPool::new();
        let buf = pool.acquire(2048);
        assert_eq!(buf.len(), 2048);
        pool.release(buf);
        let stats = pool.get_stats();
        assert_eq!(stats.acquire_calls, 1);
        assert_eq!(stats.release_calls, 1);
    }

    #[test]
    fn test_reuse() {
        let pool = BufferPool::new();
        let buf1 = pool.acquire(2048);
        pool.release(buf1);
        let buf2 = pool.acquire(2048);
        let stats = pool.get_stats();
        assert_eq!(stats.reuse_hits, 1);
        assert_eq!(stats.new_allocs, 1);
    }

    #[test]
    fn test_aligned_sizes() {
        let pool = BufferPool::new();
        let buf = pool.acquire(1500);
        assert_eq!(buf.len(), 2048);
        let buf = pool.acquire(5000);
        assert_eq!(buf.len(), 8192);
        let buf = pool.acquire(500);
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn test_clear() {
        let pool = BufferPool::new();
        let buf = pool.acquire(2048);
        pool.release(buf);
        pool.clear();
        let stats = pool.get_stats();
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.buckets, 0);
    }
}