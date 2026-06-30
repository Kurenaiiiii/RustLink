use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use anyhow::{anyhow, Result};

type Aes128CbcDec = cbc::Decryptor<Aes128>;

pub struct AESDecryptor {
    key: [u8; 16],
    iv: [u8; 16],
}

impl AESDecryptor {
    pub fn new(key: &[u8], iv: Option<&[u8]>) -> Result<Self> {
        if key.len() != 16 {
            return Err(anyhow!("AES-128 key must be 16 bytes, got {}", key.len()));
        }

        let mut key_arr = [0u8; 16];
        key_arr.copy_from_slice(key);

        let iv_arr = if let Some(iv) = iv {
            if iv.len() != 16 {
                return Err(anyhow!("AES-128 IV must be 16 bytes, got {}", iv.len()));
            }
            let mut iv_arr = [0u8; 16];
            iv_arr.copy_from_slice(iv);
            iv_arr
        } else {
            [0u8; 16]
        };

        Ok(Self {
            key: key_arr,
            iv: iv_arr,
        })
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() % 16 != 0 {
            return Err(anyhow!("Data length must be multiple of 16 bytes for CBC"));
        }

        let mut buffer = data.to_vec();
        let cipher = Aes128CbcDec::new(&self.key.into(), &self.iv.into());
        let unpadded = cipher
            .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buffer)
            .map_err(|e| anyhow!("Decryption failed: {:?}", e))?;

        Ok(unpadded.to_vec())
    }
}

pub fn parse_iv(iv_str: &str) -> Result<[u8; 16]> {
    let iv_str = iv_str.strip_prefix("0x").unwrap_or(iv_str);
    let bytes = hex::decode(iv_str)?;
    if bytes.len() != 16 {
        return Err(anyhow!("IV must be 16 bytes"));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::Aes128;
    use aes::cipher::BlockEncryptMut;
    use cbc::cipher::KeyIvInit;

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;

    #[test]
    fn test_aes_decrypt() {
        let key = [0u8; 16];
        let iv = [0u8; 16];
        let plaintext = b"Hello, World!123";
        let mut buffer = plaintext.to_vec();
        let pad_len = 16 - (buffer.len() % 16);
        buffer.extend(std::iter::repeat(pad_len as u8).take(pad_len));
        // Proper AES-128-CBC encryption with zero key
        let cipher = Aes128CbcEnc::new(&key.into(), &iv.into());
        cipher.encrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buffer, plaintext.len()).unwrap();

        let decryptor = AESDecryptor::new(&key, Some(&iv)).unwrap();
        let decrypted = decryptor.decrypt(&buffer).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_parse_iv() {
        let iv = parse_iv("0x1234567890abcdef1234567890abcdef").unwrap();
        assert_eq!(iv[0], 0x12);
        assert_eq!(iv[15], 0xef);
    }
}