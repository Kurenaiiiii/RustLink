/// Spotify TOTP authentication for web-player token generation.

use sha1::{Digest, Sha1};
use std::sync::Mutex;

#[derive(Debug, Clone)]
struct EncodedSpotifySecretEntry {
    secret: &'static str,
    version: u32,
}

const ENCODED_SECRETS: &[EncodedSpotifySecretEntry] = &[
    EncodedSpotifySecretEntry { secret: ",7/*F(\"rLJ2oxaKL^f+E1xvP@N", version: 61 },
    EncodedSpotifySecretEntry { secret: "OmE{ZA.J^\":0FG\\Uz?[@WW", version: 60 },
    EncodedSpotifySecretEntry { secret: "{iOFn;4}<1PFYKPV?5{%u14]M>/V0hDH", version: 59 },
];

const SECRET_FETCH_INTERVAL: u64 = 60 * 60 * 1000;
const SECRETS_URL: &str =
    "https://raw.githubusercontent.com/xyloflake/spot-secrets-go/refs/heads/main/secrets/secretDict.json";
const USER_AGENT_MOBILE: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

struct TotpState {
    secret: Option<String>,
    version: Option<String>,
    last_fetch_time: u64,
}

static TOTP_STATE: Mutex<TotpState> = Mutex::new(TotpState {
    secret: None,
    version: None,
    last_fetch_time: 0,
});

fn decode_secret(encoded: &str) -> Vec<u8> {
    let t: u8 = 33;
    let n: u8 = 9;
    encoded
        .chars()
        .enumerate()
        .map(|(i, c)| (c as u8) ^ ((i as u8 % t) + n))
        .collect()
}

fn bytes_to_hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0))
        .collect()
}

async fn ensure_totp_secrets() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    {
        let state = TOTP_STATE.lock().unwrap();
        if state.secret.is_some() && now - state.last_fetch_time < SECRET_FETCH_INTERVAL {
            return;
        }
    }

    let client = reqwest::Client::new();
    match client
        .get(SECRETS_URL)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    if let Some(obj) = data.as_object() {
                        let versions: Vec<u32> = obj
                            .keys()
                            .filter_map(|k| k.parse::<u32>().ok())
                            .collect();
                        if let Some(&newest) = versions.iter().max() {
                            if let Some(entry) = obj.get(&newest.to_string()) {
                                if let Some(arr) = entry.as_array() {
                                    let mapped: Vec<u8> = arr
                                        .iter()
                                        .enumerate()
                                        .map(|(i, v)| {
                                            let val = v.as_u64().unwrap_or(0) as u8;
                                            val ^ ((i as u8 % 33) + 9)
                                        })
                                        .collect();
                                    let ascii_str = String::from_utf8_lossy(&mapped);
                                    let mut state = TOTP_STATE.lock().unwrap();
                                    state.secret = Some(ascii_str.to_string());
                                    state.version = Some(newest.to_string());
                                    state.last_fetch_time = now;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("SpotifyAuth: Failed to parse secrets JSON: {e}");
                }
            }
        }
        Ok(resp) => {
            tracing::warn!("SpotifyAuth: Secrets fetch returned {}", resp.status());
        }
        Err(e) => {
            tracing::warn!("SpotifyAuth: Error fetching TOTP secrets: {e}. Using fallback.");
        }
    }

    {
        let mut state = TOTP_STATE.lock().unwrap();
        if state.secret.is_none() {
            let fallback_data: [u8; 26] = [
                99, 111, 47, 88, 49, 56, 118, 65, 52, 67, 50, 104, 117, 101, 55, 94, 95, 75, 94,
                49, 69, 36, 85, 64, 74, 60,
            ];
            let mapped: Vec<u8> = fallback_data
                .iter()
                .enumerate()
                .map(|(i, &v)| v ^ ((i as u8 % 33) + 9))
                .collect();
            let ascii_str = String::from_utf8_lossy(&mapped);
            state.secret = Some(ascii_str.to_string());
            state.version = Some("19".to_string());
        }
    }
}

async fn get_server_time(sp_dc: Option<&str>) -> u64 {
    let client = reqwest::Client::new();
    let mut req = client.get("https://open.spotify.com/api/server-time");
    if let Some(cookie) = sp_dc {
        req = req.header("Cookie", format!("sp_dc={cookie}"));
    }
    req = req.header("User-Agent", USER_AGENT_MOBILE);

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(data) => data
                    .get("serverTime")
                    .and_then(|v| v.as_u64())
                    .unwrap_or_else(now_ms),
                _ => now_ms(),
            }
        }
        _ => now_ms(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn generate_totp(secret_hex: &str, timestamp_ms: u64, step: u64) -> String {
    let counter = timestamp_ms / 1000 / step;

    let counter_bytes = counter.to_be_bytes();

    let key = hex_to_bytes(secret_hex);

    let hmac = hmac_sha1(&key, &counter_bytes);

    let offset = (hmac[hmac.len() - 1] & 0x0f) as usize;

    let code = ((hmac[offset] as u32 & 0x7f) << 24
        | (hmac[offset + 1] as u32 & 0xff) << 16
        | (hmac[offset + 2] as u32 & 0xff) << 8
        | (hmac[offset + 3] as u32 & 0xff))
        % 1000000;

    format!("{code:06}")
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> Vec<u8> {
    const BLOCK_SIZE: usize = 64;
    let mut k = key.to_vec();

    if k.len() > BLOCK_SIZE {
        let mut hasher = Sha1::new();
        hasher.update(&k);
        k = hasher.finalize().to_vec();
    }

    k.resize(BLOCK_SIZE, 0);

    let mut ipad = vec![0x36u8; BLOCK_SIZE];
    let mut opad = vec![0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha1::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha1::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize().to_vec()
}

async fn perform_token_request(
    secret: &str,
    version: &str,
    sp_dc: Option<&str>,
    product_type: &str,
) -> Result<serde_json::Value, String> {
    let is_web_player = product_type == "web-player";
    let server_time_ms = if is_web_player { now_ms() } else { get_server_time(sp_dc).await };
    let local_time_ms = now_ms();

    let totp_local = generate_totp(secret, local_time_ms, 30);
    let totp_server = generate_totp(secret, server_time_ms, 900);

    let mut url = format!(
        "https://open.spotify.com/api/token?reason=init&productType={}&totp={}&totpServer={}&totpVer={}",
        product_type, totp_local,
        if is_web_player { &totp_local } else { &totp_server },
        version
    );
    if !is_web_player {
        url.push_str("&platform=web");
    }

    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("User-Agent", USER_AGENT_MOBILE)
        .header("Origin", "https://open.spotify.com/")
        .header("Referer", "https://open.spotify.com/")
        .header("Accept", "application/json");

    if let Some(cookie) = sp_dc {
        if !is_web_player {
            req = req.header("Cookie", format!("sp_dc={cookie}"));
        }
    }

    let resp = req.send().await.map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Spotify Auth Error: {}", resp.status()));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("Parse failed: {e}"))?;
    Ok(body)
}

/// Retrieves a Spotify local token compatible with web/mobile player flows.
pub async fn get_local_token(
    sp_dc: Option<&str>,
    product_type: Option<&str>,
) -> Result<serde_json::Value, String> {
    let product_type = product_type.unwrap_or("mobile-web-player");

    let primary = &ENCODED_SECRETS[0];
    let native_secret_bytes = decode_secret(primary.secret);
    let ascii_str = String::from_utf8_lossy(&native_secret_bytes);
    let native_secret = bytes_to_hex_string(ascii_str.as_bytes());
    let native_version = primary.version.to_string();

    let result = perform_token_request(&native_secret, &native_version, sp_dc, product_type).await;

    match result {
        Ok(token) => Ok(token),
        Err(_) => {
            ensure_totp_secrets().await;
            let state = TOTP_STATE.lock().unwrap();
            if let Some(ref secret) = state.secret {
                let version = state.version.as_deref().unwrap_or("19");
                perform_token_request(secret, version, sp_dc, product_type).await
            } else {
                Err("No TOTP secret available".to_string())
            }
        }
    }
}

/// Gets the access token string from a Spotify local token response.
pub fn get_access_token(response: &serde_json::Value) -> Option<&str> {
    response.get("accessToken")?.as_str()
}

/// Gets the access token expiration timestamp from a response.
pub fn get_access_token_expiration(response: &serde_json::Value) -> Option<u64> {
    response
        .get("accessTokenExpirationTimestampMs")?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_secret() {
        let decoded = decode_secret(",7/*F(\"rLJ2oxaKL^f+E1xvP@N");
        assert!(!decoded.is_empty());
        for &b in &decoded {
            assert!(b.is_ascii_graphic() || b.is_ascii_whitespace() || b.is_ascii_alphanumeric());
        }
    }

    #[test]
    fn test_generate_totp() {
        let secret_hex =
            bytes_to_hex_string(&decode_secret(",7/*F(\"rLJ2oxaKL^f+E1xvP@N"));
        let code = generate_totp(&secret_hex, 1700000000000, 30);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_hmac_sha1() {
        let result = hmac_sha1(b"key", b"data");
        assert_eq!(result.len(), 20);
    }

    #[test]
    fn test_totp_reproducibility() {
        let secret_hex =
            bytes_to_hex_string(&decode_secret(",7/*F(\"rLJ2oxaKL^f+E1xvP@N"));
        let code1 = generate_totp(&secret_hex, 1700000000000, 30);
        let code2 = generate_totp(&secret_hex, 1700000000000, 30);
        assert_eq!(code1, code2);
    }
}
