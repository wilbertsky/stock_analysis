use axum::{extract::State, http::StatusCode, Json};
use uuid::Uuid;

use crate::{
    auth::middleware::AdminUser,
    error::AppError,
    models::{CreateInviteRequest, InviteCodeRow},
    state::AppState,
};

// ── POST /api/admin/invite-codes ─────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/admin/invite-codes",
    tag = "admin",
    request_body = CreateInviteRequest,
    description = "Create an invite code assigned to a specific email address. Admin only.",
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Invite code created", body = InviteCodeRow),
        (status = 400, description = "Invalid input"),
        (status = 403, description = "Admin required"),
    )
)]
pub async fn create_invite(
    State(state): State<AppState>,
    auth: AdminUser,
    Json(body): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<InviteCodeRow>), AppError> {
    if body.email.trim().is_empty() || !body.email.contains('@') {
        return Err(AppError::BadRequest("Invalid email address".into()));
    }

    let code = Uuid::new_v4().to_string();

    let row = sqlx::query!(
        "INSERT INTO invite_codes (code, email, created_by)
         VALUES ($1, $2, $3)
         RETURNING code, email, created_at",
        code,
        body.email.trim().to_ascii_lowercase(),
        auth.user_id,
    )
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(InviteCodeRow {
            code: row.code,
            email: row.email,
            created_at: row.created_at,
            used_at: None,
        }),
    ))
}

// ── GET /api/admin/invite-codes ───────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/admin/invite-codes",
    tag = "admin",
    description = "List all invite codes with their status. Admin only.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of invite codes", body = Vec<InviteCodeRow>),
        (status = 403, description = "Admin required"),
    )
)]
pub async fn list_invites(
    State(state): State<AppState>,
    _auth: AdminUser,
) -> Result<Json<Vec<InviteCodeRow>>, AppError> {
    let rows = sqlx::query!(
        "SELECT code, email, created_at, used_at
         FROM invite_codes
         ORDER BY created_at DESC"
    )
    .fetch_all(&state.db)
    .await?;

    let codes = rows
        .into_iter()
        .map(|r| InviteCodeRow {
            code: r.code,
            email: r.email,
            created_at: r.created_at,
            used_at: r.used_at,
        })
        .collect();

    Ok(Json(codes))
}
