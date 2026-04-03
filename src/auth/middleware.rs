use axum::{
    extract::{FromRef, FromRequestParts, State},
    http::request::Parts,
};
use uuid::Uuid;
use crate::{error::AppError, state::AppState};
use super::jwt::decode_token;

/// Axum extractor that validates the `Authorization: Bearer <token>` header
/// and provides the authenticated user's id and email to handlers.
pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let State(app_state) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Internal("State extraction failed".into()))?;

        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthorized)?;

        let claims = decode_token(token, &app_state.jwt_secret)?;
        Ok(AuthUser { user_id: claims.sub, email: claims.email })
    }
}
