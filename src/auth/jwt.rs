use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::AppError;

/// Token lifetime: 24 hours.
const TOKEN_EXPIRY_SECS: i64 = 86_400;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — user UUID.
    pub sub: Uuid,
    pub email: String,
    /// "admin" or "subscriber"
    pub role: String,
    /// Unix timestamp (seconds) at which the token expires.
    pub exp: usize,
}

pub fn encode_token(user_id: Uuid, email: &str, role: &str, secret: &[u8]) -> Result<String, AppError> {
    let exp = (chrono::Utc::now().timestamp() + TOKEN_EXPIRY_SECS) as usize;
    let claims = Claims { sub: user_id, email: email.to_owned(), role: role.to_owned(), exp };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret))
        .map_err(|_| AppError::Internal("Token encoding failed".into()))
}

pub fn decode_token(token: &str, secret: &[u8]) -> Result<Claims, AppError> {
    decode::<Claims>(token, &DecodingKey::from_secret(secret), &Validation::default())
        .map(|data| data.claims)
        .map_err(|_| AppError::Unauthorized)
}
