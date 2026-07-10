use std::time::Duration;
use axum::{extract::{Path, State}, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::{auth::middleware::AuthUser, error::AppError, state::AppState};

// The RAG orchestrator chains multiple Claude API calls — give it enough runway.
const RAG_TIMEOUT: Duration = Duration::from_secs(120);

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
        .timeout(RAG_TIMEOUT)
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

#[utoipa::path(
    get,
    path = "/api/chat-jobs/{job_id}",
    tag = "chat",
    params(
        ("job_id" = String, Path, description = "Job ID returned by POST /api/chat")
    ),
    responses(
        (status = 200, description = "Job status and answer when completed"),
        (status = 404, description = "Job not found or expired"),
        (status = 503, description = "RAG service not configured", body = crate::error::ErrorBody),
        (status = 502, description = "RAG service error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_chat_job(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rag_url = state.rag_url.as_deref().ok_or_else(|| {
        AppError::Internal("RAG service not configured (RAG_URL env var missing)".into())
    })?;

    let mut req = state
        .http_client
        .get(format!("{rag_url}/api/chat-jobs/{job_id}"))
        .timeout(Duration::from_secs(10))
        .header("Accept", "application/json");

    if let Some(secret) = &state.rag_secret {
        req = req.header("X-Rag-Secret", secret.as_ref());
    }

    let res = req
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("RAG service unreachable: {e}")))?;

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::NotFound);
    }

    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        tracing::warn!("RAG chat-job poll returned {status}: {text}");
        return Err(AppError::Internal(format!("RAG service error {status}")));
    }

    let payload: serde_json::Value = res
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("RAG response parse error: {e}")))?;

    Ok(Json(payload))
}
