use axum::{extract::State, http::StatusCode, Json};
use tokio::task::spawn_blocking;
use uuid::Uuid;
use crate::{
    auth::{jwt::encode_token, middleware::AuthUser, password},
    crypto,
    error::AppError,
    models::{AuthResponse, LoginRequest, MeResponse, RegisterRequest, UpsertFmpKeyRequest},
    state::AppState,
};

// ── POST /api/auth/register ───────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    description = "Create a new user account. Passwords are hashed with Argon2.",
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

    let plain = body.password.clone();
    let hash = spawn_blocking(move || password::hash_password(&plain))
        .await
        .map_err(|_| AppError::Internal("Thread join error".into()))??;

    let row = sqlx::query_as::<_, (Uuid, String)>(
        "INSERT INTO users (email, password_hash) VALUES ($1, $2) RETURNING id, email",
    )
    .bind(body.email.trim())
    .bind(hash)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.constraint() == Some("users_email_key") => {
            AppError::BadRequest("Email already registered".into())
        }
        other => AppError::Db(other),
    })?;

    Ok((
        StatusCode::CREATED,
        Json(MeResponse { user_id: row.0, email: row.1, has_fmp_key: false }),
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
    responses(
        (status = 200, description = "JWT token", body = AuthResponse),
        (status = 401, description = "Invalid credentials", body = crate::error::ErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, email, password_hash FROM users WHERE email = $1",
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

    let token = encode_token(row.0, &row.1, &state.jwt_secret)?;
    Ok(Json(AuthResponse { token }))
}

// ── GET /api/auth/me ──────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    description = "Returns the currently authenticated user's profile.",
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
    let row = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        "SELECT id, email, fmp_key_enc FROM users WHERE id = $1",
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Unauthorized)?;

    Ok(Json(MeResponse {
        user_id: row.0,
        email: row.1,
        has_fmp_key: row.2.is_some(),
    }))
}

// ── PUT /api/auth/fmp-key ─────────────────────────────────────────────────────

#[utoipa::path(
    put,
    path = "/api/auth/fmp-key",
    tag = "auth",
    request_body = UpsertFmpKeyRequest,
    description = "Store or replace your personal FMP API key. \
        The key is encrypted at rest with AES-256-GCM. Once stored, authenticated \
        portfolio and analysis endpoints will use your key instead of the server default.",
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
