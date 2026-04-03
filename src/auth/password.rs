use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use crate::error::AppError;

pub fn hash_password(plain: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| AppError::Internal("Password hashing failed".into()))
}

pub fn verify_password(plain: &str, hash: &str) -> Result<(), AppError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|_| AppError::Internal("Invalid stored password hash".into()))?;
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .map_err(|_| AppError::Unauthorized)
}
