use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, warn};

use xsalsa20poly1305::XSalsa20Poly1305;

use crate::player::voice::EncryptionMode;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RelayConfig {
    pub enabled: bool,
    pub buffer_size: usize,
    pub bind_address: String,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            buffer_size: 1024,
            bind_address: "0.0.0.0:0".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InterceptedPacket {
    pub ssrc: u32,
    pub sequence: u16,
    pub timestamp: u32,
    pub encrypted: Vec<u8>,
    pub decrypted: Vec<u8>,
    pub payload_type: u8,
    pub source: SocketAddr,
}

pub struct VoiceRelay;

impl VoiceRelay {
    #[allow(dead_code)]
    pub fn start_listener(
        udp_socket: Arc<UdpSocket>,
        ssrc: u32,
        secret_key: [u8; 32],
        encryption_mode: EncryptionMode,
        packet_tx: mpsc::Sender<InterceptedPacket>,
    ) {
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                let (len, source) = match tokio::time::timeout(
                    Duration::from_secs(30),
                    udp_socket.recv_from(&mut buf),
                )
                .await
                {
                    Ok(Ok((len, source))) => (len, source),
                    Ok(Err(e)) => {
                        warn!(target: "VoiceRelay", "UDP recv error: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    Err(_) => continue,
                };

                if len < 12 {
                    continue;
                }

                let packet_data = &buf[..len];
                let rtp_header = &packet_data[..12];

                let payload_type = rtp_header[1] & 0x7f;
                let sequence = u16::from_be_bytes([rtp_header[2], rtp_header[3]]);
                let timestamp = u32::from_be_bytes([rtp_header[4], rtp_header[5], rtp_header[6], rtp_header[7]]);
                let packet_ssrc = u32::from_be_bytes([rtp_header[8], rtp_header[9], rtp_header[10], rtp_header[11]]);

                if packet_ssrc != ssrc {
                    continue;
                }

                let encrypted_data = &packet_data[12..];

                let decrypted = match decrypt_packet(encrypted_data, &secret_key, sequence, encryption_mode, rtp_header) {
                    Some(d) => d,
                    None => continue,
                };

                let intercepted = InterceptedPacket {
                    ssrc: packet_ssrc,
                    sequence,
                    timestamp,
                    encrypted: encrypted_data.to_vec(),
                    decrypted,
                    payload_type,
                    source,
                };

                if packet_tx.send(intercepted).await.is_err() {
                    info!(target: "VoiceRelay", "Relay receiver dropped, stopping listener");
                    break;
                }
            }
        });
    }
}

#[allow(dead_code)]
fn decrypt_xsalsa20(
    encrypted: &[u8],
    secret_key: &[u8; 32],
    sequence: u16,
    mode: EncryptionMode,
) -> Option<Vec<u8>> {
    let (ciphertext, nonce_bytes) = match mode {
        EncryptionMode::XSalsa20Poly1305 => {
            let mut n = [0u8; 24];
            n[..4].copy_from_slice(&sequence.to_le_bytes());
            (encrypted, n.to_vec())
        }
        EncryptionMode::XSalsa20Poly1305Suffix => {
            if encrypted.len() < 24 {
                return None;
            }
            let (ct, suffix) = encrypted.split_at(encrypted.len() - 24);
            (ct, suffix.to_vec())
        }
        EncryptionMode::XSalsa20Poly1305Lite => {
            if encrypted.len() < 4 {
                return None;
            }
            let (ct, suffix) = encrypted.split_at(encrypted.len() - 4);
            let mut n = [0u8; 24];
            n[..4].copy_from_slice(suffix);
            (ct, n.to_vec())
        }
        _ => return None,
    };

    let cipher = XSalsa20Poly1305::new_from_slice(secret_key).ok()?;
    let mut nonce_arr = [0u8; 24];
    nonce_arr.copy_from_slice(&nonce_bytes);

    cipher
        .decrypt(
            &nonce_arr.into(),
            Payload {
                msg: ciphertext,
                aad: &[],
            },
        )
        .ok()
}

#[allow(dead_code)]
fn decrypt_aes256_gcm(
    encrypted: &[u8],
    secret_key: &[u8; 32],
    sequence: u16,
    rtp_header: &[u8],
    mode: EncryptionMode,
) -> Option<Vec<u8>> {
    let (ciphertext, nonce_vec, aad) = match mode {
        EncryptionMode::AeadAes256Gcm => {
            if encrypted.len() < 12 {
                return None;
            }
            let (ct, suffix) = encrypted.split_at(encrypted.len() - 12);
            (ct, suffix.to_vec(), &[][..])
        }
        EncryptionMode::AeadAes256GcmRtpsize => {
            let mut n = vec![0u8; 4];
            n.copy_from_slice(&sequence.to_le_bytes());
            (encrypted, n, rtp_header)
        }
        _ => return None,
    };

    let cipher = Aes256Gcm::new_from_slice(secret_key).ok()?;

    let mut nonce_full = vec![0u8; 12];
    nonce_full[..nonce_vec.len()].copy_from_slice(&nonce_vec);

    cipher
        .decrypt(
            Nonce::from_slice(&nonce_full),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .ok()
}

#[allow(dead_code)]
fn decrypt_packet(
    encrypted: &[u8],
    secret_key: &[u8; 32],
    sequence: u16,
    mode: EncryptionMode,
    rtp_header: &[u8],
) -> Option<Vec<u8>> {
    match mode {
        EncryptionMode::XSalsa20Poly1305
        | EncryptionMode::XSalsa20Poly1305Suffix
        | EncryptionMode::XSalsa20Poly1305Lite => {
            decrypt_xsalsa20(encrypted, secret_key, sequence, mode)
        }
        EncryptionMode::AeadAes256Gcm | EncryptionMode::AeadAes256GcmRtpsize => {
            decrypt_aes256_gcm(encrypted, secret_key, sequence, rtp_header, mode)
        }
    }
}
