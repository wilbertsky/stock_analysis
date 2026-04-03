use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("FMP API request failed: {0}")]
    Fmp(#[from] reqwest::Error),

    #[error("Ticker not found or no data returned")]
    NotFound,

    #[error("Insufficient historical data: need {needed} years, have {have}")]
    InsufficientData { needed: u32, have: usize },

    #[error("{0}")]
    Unprocessable(String),

    #[error("Invalid input: {0}")]
    BadRequest(String),

    #[error("Authentication required")]
    Unauthorized,

    #[error("Access denied")]
    Forbidden,

    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("Encryption error")]
    Crypto,

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::InsufficientData { .. } | AppError::Unprocessable(_) => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Fmp(_) => StatusCode::BAD_GATEWAY,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::Db(_) | AppError::Crypto | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, Json(ErrorBody { error: self.to_string() })).into_response()
    }
}
