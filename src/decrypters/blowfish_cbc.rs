use blowfish::Blowfish;
use cipher::{BlockCipherDecrypt, KeyInit};
use md5::Digest;

pub const DEEZER_IV: [u8; 8] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];

pub const DEEZER_CHUNK_SIZE: usize = 2048;

pub fn blowfish_cbc_decrypt(cipher: &Blowfish, iv: &[u8; 8], data: &mut [u8]) {
    let block_size = 8;
    let mut prev: [u8; 8] = *iv;

    for block_start in (0..data.len()).step_by(block_size) {
        let end = (block_start + block_size).min(data.len());
        if end - block_start != block_size {
            break;
        }

        let block_slice = &data[block_start..end];
        let current: [u8; 8] = block_slice.try_into().expect("block size should be 8");

        let mut cipher_block: cipher::Block<Blowfish> = current.into();
        cipher.decrypt_block(&mut cipher_block);

        let decrypted: [u8; 8] = cipher_block.into();
        for i in 0..8 {
            data[block_start + i] = decrypted[i] ^ prev[i];
        }
        prev = current;
    }
}

pub fn deezer_decrypt_chunk(cipher: &Blowfish, chunk: &mut [u8; 2048]) {
    blowfish_cbc_decrypt(cipher, &DEEZER_IV, chunk);
}

pub fn deezer_blowfish_decrypt(key: &[u8], data: &mut [u8]) -> Result<(), &'static str> {
    let cipher = Blowfish::new_from_slice(key).map_err(|_| "Invalid Blowfish key length")?;

    for (chunk_index, chunk) in data.chunks_mut(DEEZER_CHUNK_SIZE).enumerate() {
        if chunk.len() < DEEZER_CHUNK_SIZE {
            break;
        }
        if chunk_index % 3 == 0 {
            let block: &mut [u8; 2048] = chunk
                .try_into()
                .expect("chunk should be exactly 2048 bytes");
            deezer_decrypt_chunk(&cipher, block);
        }
    }

    Ok(())
}

pub fn calculate_deezer_key(song_id: &str, decryption_key: &[u8; 16]) -> [u8; 16] {
    let hash = md5::Md5::digest(song_id.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    let hex_bytes = hex.as_bytes();

    let mut track_key = [0u8; 16];
    for i in 0..16 {
        track_key[i] = hex_bytes[i] ^ hex_bytes[i + 16] ^ decryption_key[i];
    }
    track_key
}
