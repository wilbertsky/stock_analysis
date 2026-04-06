use axum::{extract::State, http::StatusCode, Json};
use tokio::task::spawn_blocking;
use uuid::Uuid;
use crate::{
    auth::{jwt::encode_token, middleware::AuthUser, password},
    crypto,
    error::AppError,
    models::{AuthResponse, LoginRequest, MeResponse, RegisterRequest, UpdateProfileRequest, UpsertFmpKeyRequest},
    state::AppState,
};

// ── POST /api/auth/register ───────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    description = "Create a new user account. Passwords are hashed with Argon2. \
        New accounts are assigned the 'subscriber' role.",
    security(()),
    responses(
        (status = 201, description = "Account created", body = MeResponse),
        (status = 400, description = "Invalid input", body = crate::error::ErrorBody),
        (status = 500, description = "Internal error", body = crate::error::ErrorBody),
    )
)]
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<MeResponse>), AppError> {
    if body.email.trim().is_empty() || !body.email.contains('@') {
        return Err(AppError::BadRequest("Invalid email address".into()));
    }
    if body.password.len() < 8 {
        return Err(AppError::BadRequest("Password must be at least 8 characters".into()));
    }
    if body.invite_code.trim().is_empty() {
        return Err(AppError::BadRequest("Invite code is required".into()));
    }

    // Validate invite code: must exist, match this email, and be unused
    let invite = sqlx::query!(
        "SELECT email FROM invite_codes WHERE code = $1 AND used_at IS NULL",
        body.invite_code.trim()
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::BadRequest("Invalid or already used invite code".into()))?;

    if invite.email.to_ascii_lowercase() != body.email.trim().to_ascii_lowercase() {
        return Err(AppError::BadRequest("Invite code is not valid for this email address".into()));
    }

    let display_name = body.display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > 50 {
                return Err(AppError::BadRequest("Display name must be 50 characters or fewer".into()));
            }
            Ok(s.to_owned())
        })
        .transpose()?;

    let plain = body.password.clone();
    let hash = spawn_blocking(move || password::hash_password(&plain))
        .await
        .map_err(|_| AppError::Internal("Thread join error".into()))??;

    let row = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        "INSERT INTO users (email, password_hash, role_id, display_name)
         VALUES ($1, $2, (SELECT id FROM roles WHERE name = 'subscriber'), $3)
         RETURNING id, email, display_name",
    )
    .bind(body.email.trim())
    .bind(hash)
    .bind(&display_name)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.constraint() == Some("users_email_key") => {
            AppError::BadRequest("Email already registered".into())
        }
        other => AppError::Db(other),
    })?;

    // Mark invite code as consumed
    sqlx::query!(
        "UPDATE invite_codes SET used_at = NOW(), used_by = $1 WHERE code = $2",
        row.0,
        body.invite_code.trim()
    )
    .execute(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(MeResponse {
            user_id: row.0,
            email: row.1,
            role: "subscriber".to_owned(),
            has_fmp_key: false,
            display_name: row.2,
        }),
    ))
}

// ── POST /api/auth/login ──────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    description = "Authenticate and receive a JWT token valid for 24 hours. \
        Pass the token as `Authorization: Bearer <token>` on subsequent requests.",
    security(()),
    responses(
        (status = 200, description = "JWT token", body = AuthResponse),
        (status = 401, description = "Invalid credentials", body = crate::error::ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String)>(
        "SELECT u.id, u.email, u.password_hash, r.name AS role
         FROM users u
         JOIN roles r ON r.id = u.role_id
         WHERE u.email = $1",
    )
    .bind(body.email.trim())
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;

    let plain = body.password.clone();
    let hash = row.2.clone();
    spawn_blocking(move || password::verify_password(&plain, &hash))
        .await
        .map_err(|_| AppError::Internal("Thread join error".into()))??;

    let token = encode_token(row.0, &row.1, &row.3, &state.jwt_secret)?;
    Ok(Json(AuthResponse { token }))
}

// ── GET /api/auth/me ──────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    description = "Returns the currently authenticated user's profile including their role.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "User profile", body = MeResponse),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn get_me(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MeResponse>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, Option<String>, Option<String>)>(
        "SELECT u.id, u.email, r.name AS role, u.fmp_key_enc, u.display_name
         FROM users u
         JOIN roles r ON r.id = u.role_id
         WHERE u.id = $1",
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;

    Ok(Json(MeResponse {
        user_id: row.0,
        email: row.1,
        role: row.2,
        has_fmp_key: row.3.is_some(),
        display_name: row.4,
    }))
}

// ── PATCH /api/auth/profile ───────────────────────────────────────────────────

#[utoipa::path(
    patch,
    path = "/api/auth/profile",
    tag = "auth",
    request_body = UpdateProfileRequest,
    description = "Update your display name. Send `null` or omit the field to clear it, \
        which reverts to showing your email prefix in community views. \
        Maximum 50 characters.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Updated profile", body = MeResponse),
        (status = 400, description = "Invalid input", body = crate::error::ErrorBody),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn update_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<Json<MeResponse>, AppError> {
    let display_name = body.display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > 50 {
                return Err(AppError::BadRequest(
                    "Display name must be 50 characters or fewer".into(),
                ));
            }
            Ok(s.to_owned())
        })
        .transpose()?;

    let row = sqlx::query_as::<_, (Uuid, String, String, Option<String>, Option<String>)>(
        "UPDATE users SET display_name = $1
         WHERE id = $2
         RETURNING id, email,
                   (SELECT name FROM roles WHERE id = role_id) AS role,
                   fmp_key_enc,
                   display_name",
    )
    .bind(&display_name)
    .bind(auth.user_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(MeResponse {
        user_id: row.0,
        email: row.1,
        role: row.2,
        has_fmp_key: row.3.is_some(),
        display_name: row.4,
    }))
}

// ── PUT /api/auth/fmp-key ─────────────────────────────────────────────────────

#[utoipa::path(
    put,
    path = "/api/auth/fmp-key",
    tag = "auth",
    request_body = UpsertFmpKeyRequest,
    description = "Store or replace your personal FMP API key. \
        The key is encrypted at rest with AES-256-GCM.",
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Key stored successfully"),
        (status = 400, description = "Invalid input", body = crate::error::ErrorBody),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn upsert_fmp_key(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UpsertFmpKeyRequest>,
) -> Result<StatusCode, AppError> {
    if body.fmp_key.trim().is_empty() {
        return Err(AppError::BadRequest("fmp_key must not be empty".into()));
    }
    let enc = crypto::encrypt(body.fmp_key.trim(), &state.fmp_enc_key)?;
    sqlx::query("UPDATE users SET fmp_key_enc = $1 WHERE id = $2")
        .bind(enc)
        .bind(auth.user_id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
