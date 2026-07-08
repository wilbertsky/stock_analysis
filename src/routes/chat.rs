use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::{auth::middleware::AuthUser, error::AppError, state::AppState};

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ChatHistoryItem {
    /// "user" or "assistant"
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChatRequest {
    /// The question to ask (3–500 characters).
    pub question: String,
    /// Prior conversation turns, oldest first (max 12).
    #[serde(default)]
    pub history: Vec<ChatHistoryItem>,
}

#[utoipa::path(
    post,
    path = "/api/chat",
    tag = "chat",
    request_body = ChatRequest,
    responses(
        (status = 200, description = "Answer from the RAG assistant"),
        (status = 400, description = "Bad request", body = crate::error::ErrorBody),
        (status = 503, description = "RAG service not configured", body = crate::error::ErrorBody),
        (status = 502, description = "RAG service error", body = crate::error::ErrorBody),
    )
)]
pub async fn post_chat(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<ChatRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rag_url = state.rag_url.as_deref().ok_or_else(|| {
        AppError::Internal("RAG service not configured (RAG_URL env var missing)".into())
    })?;

    let forward = serde_json::json!({
        "question": body.question,
        "history": body.history,
    });

    let bytes = serde_json::to_vec(&forward)
        .map_err(|e| AppError::Internal(format!("serialize error: {e}")))?;

    let mut req = state
        .http_client
        .post(format!("{rag_url}/api/chat"))
        .header("Content-Type", "application/ld+json");

    if let Some(secret) = &state.rag_secret {
        req = req.header("X-Rag-Secret", secret.as_ref());
    }

    let res = req
        .body(bytes)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("RAG service unreachable: {e}")))?;

    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        tracing::warn!("RAG service returned {status}: {text}");
        return Err(AppError::Internal(format!("RAG service error {status}")));
    }

    let payload: serde_json::Value = res
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("RAG response parse error: {e}")))?;

    Ok(Json(payload))
}
