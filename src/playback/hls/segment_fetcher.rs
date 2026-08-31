use crate::playback::hls::types::*;
use crate::playback::hls::aes_decryptor::AESDecryptor;
use anyhow::{anyhow, Result};
use reqwest::Client;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Clone)]
pub struct SegmentFetcher {
    client: Client,
    headers: std::collections::HashMap<String, String>,
    local_address: Option<String>,
    proxy: Option<String>,
    on_resolve_url: Option<Arc<dyn Fn(String) -> Option<String> + Send + Sync>>,
}

impl SegmentFetcher {
    pub fn new(options: SegmentFetcherOptions) -> Self {
        let mut client_builder = Client::builder();
        if let Some(ref local) = options.local_address {
            client_builder = client_builder.local_address(local.parse().ok());
        }
        if let Some(ref proxy) = options.proxy {
            if let Ok(p) = reqwest::Proxy::all(proxy) {
                client_builder = client_builder.proxy(p);
            }
        }
        let client = client_builder.build().expect("Failed to create HTTP client");

        Self {
            client,
            headers: options.headers.unwrap_or_default(),
            local_address: options.local_address,
            proxy: options.proxy,
            on_resolve_url: options.on_resolve_url,
        }
    }

    pub async fn fetch_segment(
        &self,
        segment: &HLSSegment,
        stream: bool,
    ) -> Result<SegmentFetchResult> {
        let url = if let Some(resolve) = &self.on_resolve_url {
            resolve(segment.url.clone()).unwrap_or(segment.url.clone())
        } else {
            segment.url.clone()
        };

        let mut req = self.client.get(&url);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("Segment fetch failed: HTTP {}", resp.status()));
        }

        let bytes = resp.bytes().await?;
        let mut data = bytes.to_vec();

        if stream {
            let stream = futures::stream::iter(vec![Ok::<_, Box<dyn std::error::Error + Send + Sync>>(bytes)]);
            Ok(SegmentFetchResult {
                segment: segment.clone(),
                data: None,
                stream: Some(Box::pin(stream)),
            })
        } else {

            if let Some(key) = &segment.key {
                if key.method == "AES-128" {
                    if let Some(key_uri) = &key.uri {
                        let key_data = self.fetch_key(key_uri).await?;
                        let iv = key.iv.as_ref().map(|s| parse_iv(s)).transpose()?.unwrap_or([0u8; 16]);
                        let decryptor = AESDecryptor::new(&key_data, Some(&iv))?;
                        data = decryptor.decrypt(&data)?;
                    }
                }
            }

            Ok(SegmentFetchResult {
                segment: segment.clone(),
                data: Some(data),
                stream: None,
            })
        }
    }

    pub async fn fetch_map(&self, map: &HLSMap, key: Option<&HLSKey>) -> Result<Option<Vec<u8>>> {
        let url = if let Some(resolve) = &self.on_resolve_url {
            resolve(map.uri.clone()).unwrap_or(map.uri.clone())
        } else {
            map.uri.clone()
        };

        let mut req = self.client.get(&url);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }

        let mut data = resp.bytes().await?.to_vec();

        if let Some(key) = key {
            if key.method == "AES-128" {
                if let Some(key_uri) = &key.uri {
                    let key_data = self.fetch_key(key_uri).await?;
                    let iv = key.iv.as_ref().map(|s| parse_iv(s)).transpose()?.unwrap_or([0u8; 16]);
                    let decryptor = AESDecryptor::new(&key_data, Some(&iv))?;
                    data = decryptor.decrypt(&data)?;
                }
            }
        }

        Ok(Some(data))
    }

    async fn fetch_key(&self, key_url: &str) -> Result<Vec<u8>> {
        let url = if let Some(resolve) = &self.on_resolve_url {
            resolve(key_url.to_string()).unwrap_or(key_url.to_string())
        } else {
            key_url.to_string()
        };

        let mut req = self.client.get(&url);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("Key fetch failed: HTTP {}", resp.status()));
        }

        let key_data = resp.bytes().await?;
        Ok(key_data.to_vec())
    }
}

#[derive(Clone, Default)]
pub struct SegmentFetcherOptions {
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub local_address: Option<String>,
    pub proxy: Option<String>,
    pub on_resolve_url: Option<Arc<dyn Fn(String) -> Option<String> + Send + Sync>>,
}

pub struct SegmentFetchResult {
    pub segment: HLSSegment,
    pub data: Option<Vec<u8>>,
    pub stream: Option<Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>>,
}

fn parse_iv(iv_str: &str) -> anyhow::Result<[u8; 16]> {
    let iv_str = iv_str.strip_prefix("0x").unwrap_or(iv_str);
    let bytes = hex::decode(iv_str)?;
    if bytes.len() != 16 {
        return Err(anyhow!("IV must be 16 bytes"));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}