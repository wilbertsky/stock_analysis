use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use sqlx::Row;

use crate::{
    auth::middleware::{AdminUser, AuthUser},
    error::AppError,
    models::{FeedbackRow, SubmitFeedbackRequest},
    state::AppState,
};

// ── POST /api/feedback ────────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/feedback",
    tag = "feedback",
    request_body = SubmitFeedbackRequest,
    description = "Submit feedback. Stored with a timestamp and marked unread for admin review.",
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Feedback submitted"),
        (status = 400, description = "Empty message", body = crate::error::ErrorBody),
        (status = 401, description = "Not authenticated", body = crate::error::ErrorBody),
    )
)]
pub async fn submit_feedback(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<SubmitFeedbackRequest>,
) -> Result<StatusCode, AppError> {
    if body.message.trim().is_empty() {
        return Err(AppError::BadRequest("Message must not be empty".into()));
    }
    sqlx::query(
        "INSERT INTO feedback (user_id, message) VALUES ($1, $2)",
    )
    .bind(auth.user_id)
    .bind(body.message.trim())
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── GET /api/feedback ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FeedbackFilter {
    /// When true, return only unread feedback.
    pub unread_only: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/feedback",
    tag = "feedback",
    params(("unread_only" = Option<bool>, Query, description = "Filter to unread only")),
    description = "List all feedback entries. Admin only. Sorted newest first.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Feedback list", body = Vec<FeedbackRow>),
        (status = 401, description = "Not authenticated", body = crate::error::ErrorBody),
        (status = 403, description = "Admin only", body = crate::error::ErrorBody),
    )
)]
pub async fn list_feedback(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(filter): Query<FeedbackFilter>,
) -> Result<Json<Vec<FeedbackRow>>, AppError> {
    let unread_only = filter.unread_only.unwrap_or(false);

    let rows = if unread_only {
        sqlx::query(
            "SELECT f.id, f.user_id, u.email AS user_email, f.message, f.created_at, f.is_read
             FROM feedback f
             LEFT JOIN users u ON u.id = f.user_id
             WHERE f.is_read = false
             ORDER BY f.created_at DESC",
        )
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query(
            "SELECT f.id, f.user_id, u.email AS user_email, f.message, f.created_at, f.is_read
             FROM feedback f
             LEFT JOIN users u ON u.id = f.user_id
             ORDER BY f.created_at DESC",
        )
        .fetch_all(&state.db)
        .await?
    };

    let feedback = rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(FeedbackRow {
                id: r.try_get("id")?,
                user_id: r.try_get("user_id")?,
                user_email: r.try_get("user_email")?,
                message: r.try_get("message")?,
                created_at: r.try_get("created_at")?,
                is_read: r.try_get("is_read")?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Db)?;

    Ok(Json(feedback))
}

// ── PATCH /api/feedback/:id/read ──────────────────────────────────────────────

#[utoipa::path(
    patch,
    path = "/api/feedback/{id}/read",
    tag = "feedback",
    params(("id" = Uuid, Path, description = "Feedback UUID")),
    description = "Mark a feedback entry as read. Admin only.",
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Marked as read"),
        (status = 401, description = "Not authenticated", body = crate::error::ErrorBody),
        (status = 403, description = "Admin only", body = crate::error::ErrorBody),
        (status = 404, description = "Not found", body = crate::error::ErrorBody),
    )
)]
pub async fn mark_read(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let result = sqlx::query("UPDATE feedback SET is_read = true WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
