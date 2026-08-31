use cipher::{BlockCipherDecrypt, KeyInit};
use des::Des;

pub fn des_ecb_decrypt(key: &[u8; 8], data: &mut [u8]) -> Result<(), &'static str> {
    if data.len() % 8 != 0 {
        return Err("Data length must be a multiple of 8");
    }

    let cipher = Des::new_from_slice(key).map_err(|_| "Invalid DES key")?;

    for chunk in data.chunks_mut(8) {
        let block: &mut [u8; 8] = chunk.try_into().map_err(|_| "Invalid block size")?;
        let mut cipher_block: cipher::Block<Des> = (*block).into();
        cipher.decrypt_block(&mut cipher_block);
        *block = cipher_block.into();
    }

    Ok(())
}

pub fn des_ecb_decrypt_bytes(key: &[u8; 8], data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut buf = data.to_vec();
    des_ecb_decrypt(key, &mut buf)?;
    Ok(buf)
}

/// Decrypts a base64-encoded DES-ECB payload and returns the plaintext as UTF-8.
/// Useful for JioSaavn encrypted media URLs.
pub fn des_ecb_decrypt_base64(encrypted_base64: &str, key: &[u8; 8]) -> Result<String, &'static str> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(encrypted_base64.as_bytes())
        .map_err(|_| "Invalid base64 input")?;
    let decrypted = des_ecb_decrypt_bytes(key, &data)?;
    let unpadded = pkcs7_unpad(&decrypted);
    String::from_utf8(unpadded).map_err(|_| "Decrypted data is not valid UTF-8")
}

fn pkcs7_unpad(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return data.to_vec();
    }
    let pad = data[data.len() - 1] as usize;
    if pad == 0 || pad > 8 || pad > data.len() {
        return data.to_vec();
    }
    if data[data.len() - pad..].iter().all(|&b| b == pad as u8) {
        data[..data.len() - pad].to_vec()
    } else {
        data.to_vec()
    }
}
