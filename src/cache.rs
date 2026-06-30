use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

struct CachedEntry {
    data: Vec<u8>,
    inserted_at: Instant,
}

enum EncryptionState {
    Plain,
    Encrypted { key: [u8; 32] },
}

pub struct TrackCache {
    entries: RwLock<HashMap<String, CachedEntry>>,
    ttl: Duration,
    max_entries: usize,
    encryption: EncryptionState,
}

impl TrackCache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
            max_entries,
            encryption: EncryptionState::Plain,
        }
    }

    pub fn with_encryption(mut self, key: [u8; 32]) -> Self {
        self.encryption = EncryptionState::Encrypted { key };
        self
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, aes_gcm::Error> {
        match &self.encryption {
            EncryptionState::Plain => Ok(plaintext.to_vec()),
            EncryptionState::Encrypted { key } => {
                let mut nonce = [0u8; 12];
                getrandom::getrandom(&mut nonce)
                    .map_err(|_| aes_gcm::Error)?;
                let cipher = Aes256Gcm::new_from_slice(key)
                    .map_err(|_| aes_gcm::Error)?;
                let mut encrypted = cipher
                    .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &[] })
                    .map_err(|_| aes_gcm::Error)?;
                let mut result = nonce.to_vec();
                result.append(&mut encrypted);
                Ok(result)
            }
        }
    }

    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, aes_gcm::Error> {
        match &self.encryption {
            EncryptionState::Plain => Ok(data.to_vec()),
            EncryptionState::Encrypted { key } => {
                if data.len() < 12 {
                    return Err(aes_gcm::Error);
                }
                let (nonce, ciphertext) = data.split_at(12);
                let cipher = Aes256Gcm::new_from_slice(key)
                    .map_err(|_| aes_gcm::Error)?;
                cipher
                    .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: &[] })
                    .map_err(|_| aes_gcm::Error)
            }
        }
    }

    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            if entry.inserted_at.elapsed() < self.ttl {
                let decrypted = self.decrypt(&entry.data).ok()?;
                return serde_json::from_slice(&decrypted).ok();
            }
        }
        None
    }

    pub async fn set(&self, key: String, data: serde_json::Value) {
        let bytes = serde_json::to_vec(&data).unwrap_or_default();
        let encrypted = match self.encrypt(&bytes) {
            Ok(e) => e,
            Err(_) => return,
        };

        let mut entries = self.entries.write().await;
        let now = Instant::now();

        entries.retain(|_, v| now.duration_since(v.inserted_at) < self.ttl);
        while entries.len() >= self.max_entries {
            let oldest = entries
                .iter()
                .min_by_key(|(_, v)| v.inserted_at)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                entries.remove(&k);
            } else {
                break;
            }
        }

        entries.insert(key, CachedEntry { data: encrypted, inserted_at: now });
    }

    #[allow(dead_code)]
    pub async fn invalidate(&self, key: &str) {
        self.entries.write().await.remove(key);
    }

    #[allow(dead_code)]
    pub async fn clear(&self) {
        self.entries.write().await.clear();
    }
}
