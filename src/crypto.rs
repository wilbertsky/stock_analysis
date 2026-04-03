use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::Engine as _;
use crate::error::AppError;

/// Encrypts a plaintext string with AES-256-GCM.
/// Returns a base64-encoded blob: `nonce[12] || ciphertext`.
pub fn encrypt(plain: &str, key: &[u8; 32]) -> Result<String, AppError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plain.as_bytes()).map_err(|_| AppError::Crypto)?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(&blob))
}

/// Decrypts a blob produced by [`encrypt`].
pub fn decrypt(enc_blob: &str, key: &[u8; 32]) -> Result<String, AppError> {
    let blob = base64::engine::general_purpose::STANDARD
        .decode(enc_blob)
        .map_err(|_| AppError::Crypto)?;
    if blob.len() < 12 {
        return Err(AppError::Crypto);
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|_| AppError::Crypto)?;
    String::from_utf8(plaintext).map_err(|_| AppError::Crypto)
}
