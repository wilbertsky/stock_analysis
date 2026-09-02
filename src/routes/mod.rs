pub mod admin;
pub mod ai_portfolio;
pub mod auth;
pub mod chat;
pub mod discovery;
pub mod feedback;
pub mod market;
pub mod portfolio;
pub mod screener;
pub mod stock;

use axum::Json;
use crate::models::HealthResponse;

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "health",
    security(()),
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}
