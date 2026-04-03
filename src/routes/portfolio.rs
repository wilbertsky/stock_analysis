use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use futures::future::try_join_all;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;
use crate::{
    auth::middleware::AuthUser,
    crypto,
    error::AppError,
    fmp::FmpClient,
    models::{
        AddHoldingRequest, CreatePortfolioRequest, HoldingPerformance, HoldingRow,
        PortfolioPerformanceResponse, PortfolioRow,
    },
    state::AppState,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolves the FMP client to use for a given user:
/// - If the user has stored an encrypted FMP key, decrypt it and build a per-user client.
/// - Otherwise fall back to the server-level global client.
async fn resolve_fmp_client(state: &AppState, user_id: Uuid) -> Result<Arc<FmpClient>, AppError> {
    let row = sqlx::query("SELECT fmp_key_enc FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;

    let user_key: Option<String> = row
        .and_then(|r| r.try_get::<Option<String>, _>("fmp_key_enc").ok().flatten())
        .map(|enc| crypto::decrypt(&enc, &state.fmp_enc_key))
        .transpose()?;

    Ok(state.fmp_for_key(user_key))
}

/// Validates that the given portfolio belongs to the authenticated user.
async fn assert_owns_portfolio(
    state: &AppState,
    portfolio_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let row = sqlx::query("SELECT user_id FROM portfolios WHERE id = $1")
        .bind(portfolio_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;

    let owner: Uuid = row.try_get("user_id").map_err(AppError::Db)?;
    if owner != user_id {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

struct HoldingRecord {
    id: Uuid,
    ticker: String,
    price_at_add: f64,
    shares: Option<f64>,
    added_at: DateTime<Utc>,
}

async fn fetch_performance(
    state: &AppState,
    fmp: &Arc<FmpClient>,
    portfolio_id: Uuid,
) -> Result<PortfolioPerformanceResponse, AppError> {
    let p_row = sqlx::query(
        "SELECT id, name, is_public, share_token, created_at, updated_at
         FROM portfolios WHERE id = $1",
    )
    .bind(portfolio_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let portfolio = PortfolioRow {
        id: p_row.try_get("id").map_err(AppError::Db)?,
        name: p_row.try_get("name").map_err(AppError::Db)?,
        is_public: p_row.try_get("is_public").map_err(AppError::Db)?,
        share_token: p_row.try_get("share_token").map_err(AppError::Db)?,
        created_at: p_row.try_get("created_at").map_err(AppError::Db)?,
        updated_at: p_row.try_get("updated_at").map_err(AppError::Db)?,
    };

    let h_rows = sqlx::query(
        "SELECT id, ticker,
                price_at_add::FLOAT8 AS price_at_add,
                shares::FLOAT8       AS shares,
                added_at
         FROM portfolio_holdings WHERE portfolio_id = $1
         ORDER BY added_at ASC",
    )
    .bind(portfolio_id)
    .fetch_all(&state.db)
    .await?;

    let mut holdings_data: Vec<HoldingRecord> = Vec::with_capacity(h_rows.len());
    for row in h_rows {
        holdings_data.push(HoldingRecord {
            id: row.try_get("id").map_err(AppError::Db)?,
            ticker: row.try_get("ticker").map_err(AppError::Db)?,
            price_at_add: row.try_get::<f64, _>("price_at_add").map_err(AppError::Db)?,
            shares: row.try_get::<Option<f64>, _>("shares").map_err(AppError::Db)?,
            added_at: row.try_get("added_at").map_err(AppError::Db)?,
        });
    }

    // Fetch current prices concurrently.
    let price_futs: Vec<_> = holdings_data
        .iter()
        .map(|h| {
            let fmp = fmp.clone();
            let ticker = h.ticker.clone();
            async move { fmp.quote_price(&ticker).await }
        })
        .collect();
    let current_prices: Vec<f64> = try_join_all(price_futs).await?;

    let holdings: Vec<HoldingPerformance> = holdings_data
        .into_iter()
        .zip(current_prices)
        .map(|(h, current_price)| {
            let return_pct = if h.price_at_add > 0.0 {
                (current_price / h.price_at_add - 1.0) * 100.0
            } else {
                0.0
            };
            HoldingPerformance {
                holding: HoldingRow {
                    id: h.id,
                    ticker: h.ticker,
                    price_at_add: h.price_at_add,
                    shares: h.shares,
                    added_at: h.added_at,
                },
                current_price,
                return_pct,
            }
        })
        .collect();

    let total_return_pct = if holdings.is_empty() {
        None
    } else {
        let total_cost: f64 = holdings
            .iter()
            .filter_map(|h| h.holding.shares.map(|s| s * h.holding.price_at_add))
            .sum();
        let weighted = if total_cost > 0.0 {
            holdings
                .iter()
                .filter_map(|h| {
                    h.holding.shares.map(|s| {
                        let w = (s * h.holding.price_at_add) / total_cost;
                        w * h.return_pct
                    })
                })
                .sum()
        } else {
            holdings.iter().map(|h| h.return_pct).sum::<f64>() / holdings.len() as f64
        };
        Some(weighted)
    };

    Ok(PortfolioPerformanceResponse { portfolio, holdings, total_return_pct })
}

// ── POST /api/portfolio ───────────────────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/portfolio",
    tag = "portfolio",
    request_body = CreatePortfolioRequest,
    description = "Create a new portfolio. Set `is_public: true` to enable the shareable link.",
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Portfolio created", body = PortfolioRow),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn create_portfolio(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreatePortfolioRequest>,
) -> Result<(StatusCode, Json<PortfolioRow>), AppError> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("Portfolio name must not be empty".into()));
    }
    let row = sqlx::query(
        "INSERT INTO portfolios (user_id, name, is_public)
         VALUES ($1, $2, $3)
         RETURNING id, name, is_public, share_token, created_at, updated_at",
    )
    .bind(auth.user_id)
    .bind(body.name.trim())
    .bind(body.is_public)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(PortfolioRow {
            id: row.try_get("id").map_err(AppError::Db)?,
            name: row.try_get("name").map_err(AppError::Db)?,
            is_public: row.try_get("is_public").map_err(AppError::Db)?,
            share_token: row.try_get("share_token").map_err(AppError::Db)?,
            created_at: row.try_get("created_at").map_err(AppError::Db)?,
            updated_at: row.try_get("updated_at").map_err(AppError::Db)?,
        }),
    ))
}

// ── GET /api/portfolio ────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/portfolio",
    tag = "portfolio",
    description = "List all portfolios belonging to the authenticated user (metadata only, no holdings).",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of portfolios", body = Vec<PortfolioRow>),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn list_portfolios(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<PortfolioRow>>, AppError> {
    let rows = sqlx::query(
        "SELECT id, name, is_public, share_token, created_at, updated_at
         FROM portfolios WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    let portfolios: Result<Vec<PortfolioRow>, _> = rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(PortfolioRow {
                id: r.try_get("id")?,
                name: r.try_get("name")?,
                is_public: r.try_get("is_public")?,
                share_token: r.try_get("share_token")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            })
        })
        .collect();

    Ok(Json(portfolios.map_err(AppError::Db)?))
}

// ── GET /api/portfolio/{id} ───────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/portfolio/{id}",
    tag = "portfolio",
    params(("id" = Uuid, Path, description = "Portfolio UUID")),
    description = "Get a portfolio's holdings with live performance data. \
        Fetches the current price for each ticker from FMP.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Portfolio with performance", body = PortfolioPerformanceResponse),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
        (status = 404, description = "Portfolio not found", body = crate::error::ErrorBody),
    )
)]
pub async fn get_portfolio(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<PortfolioPerformanceResponse>, AppError> {
    assert_owns_portfolio(&state, id, auth.user_id).await?;
    let fmp = resolve_fmp_client(&state, auth.user_id).await?;
    Ok(Json(fetch_performance(&state, &fmp, id).await?))
}

// ── DELETE /api/portfolio/{id} ────────────────────────────────────────────────

#[utoipa::path(
    delete,
    path = "/api/portfolio/{id}",
    tag = "portfolio",
    params(("id" = Uuid, Path, description = "Portfolio UUID")),
    description = "Delete a portfolio and all its holdings.",
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
        (status = 404, description = "Portfolio not found", body = crate::error::ErrorBody),
    )
)]
pub async fn delete_portfolio(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    assert_owns_portfolio(&state, id, auth.user_id).await?;
    sqlx::query("DELETE FROM portfolios WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── POST /api/portfolio/{id}/holdings ─────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/portfolio/{id}/holdings",
    tag = "portfolio",
    params(("id" = Uuid, Path, description = "Portfolio UUID")),
    request_body = AddHoldingRequest,
    description = "Add a ticker to a portfolio. The current market price is fetched from FMP \
        and recorded at the time of addition — this becomes the baseline for return calculations.",
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Holding added", body = HoldingRow),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
        (status = 404, description = "Portfolio or ticker not found", body = crate::error::ErrorBody),
    )
)]
pub async fn add_holding(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AddHoldingRequest>,
) -> Result<(StatusCode, Json<HoldingRow>), AppError> {
    let ticker = body.ticker.trim().to_uppercase();
    if ticker.is_empty() {
        return Err(AppError::BadRequest("Ticker must not be empty".into()));
    }
    assert_owns_portfolio(&state, id, auth.user_id).await?;

    let fmp = resolve_fmp_client(&state, auth.user_id).await?;
    let price = fmp.quote_price(&ticker).await?;

    let row = sqlx::query(
        "INSERT INTO portfolio_holdings (portfolio_id, ticker, price_at_add, shares)
         VALUES ($1, $2, $3, $4)
         RETURNING id, ticker,
                   price_at_add::FLOAT8 AS price_at_add,
                   shares::FLOAT8       AS shares,
                   added_at",
    )
    .bind(id)
    .bind(&ticker)
    .bind(price)
    .bind(body.shares)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(HoldingRow {
            id: row.try_get("id").map_err(AppError::Db)?,
            ticker: row.try_get("ticker").map_err(AppError::Db)?,
            price_at_add: row.try_get::<f64, _>("price_at_add").map_err(AppError::Db)?,
            shares: row.try_get::<Option<f64>, _>("shares").map_err(AppError::Db)?,
            added_at: row.try_get("added_at").map_err(AppError::Db)?,
        }),
    ))
}

// ── DELETE /api/portfolio/{id}/holdings/{holding_id} ──────────────────────────

#[utoipa::path(
    delete,
    path = "/api/portfolio/{id}/holdings/{holding_id}",
    tag = "portfolio",
    params(
        ("id" = Uuid, Path, description = "Portfolio UUID"),
        ("holding_id" = Uuid, Path, description = "Holding UUID"),
    ),
    description = "Remove a single holding from a portfolio.",
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
        (status = 404, description = "Portfolio or holding not found", body = crate::error::ErrorBody),
    )
)]
pub async fn remove_holding(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, holding_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    assert_owns_portfolio(&state, id, auth.user_id).await?;
    let result = sqlx::query(
        "DELETE FROM portfolio_holdings WHERE id = $1 AND portfolio_id = $2",
    )
    .bind(holding_id)
    .bind(id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /api/portfolio/public/{share_token} ───────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/portfolio/public/{share_token}",
    tag = "portfolio",
    params(("share_token" = Uuid, Path, description = "Portfolio share token")),
    description = "View a public portfolio's performance without authentication. \
        Only works if the portfolio owner has set `is_public: true`. \
        Uses the server-level FMP API key for price fetching.",
    responses(
        (status = 200, description = "Portfolio with performance", body = PortfolioPerformanceResponse),
        (status = 404, description = "Portfolio not found or not public", body = crate::error::ErrorBody),
    )
)]
pub async fn get_public_portfolio(
    State(state): State<AppState>,
    Path(share_token): Path<Uuid>,
) -> Result<Json<PortfolioPerformanceResponse>, AppError> {
    let row = sqlx::query(
        "SELECT id FROM portfolios WHERE share_token = $1 AND is_public = true",
    )
    .bind(share_token)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let portfolio_id: Uuid = row.try_get("id").map_err(AppError::Db)?;

    // Public endpoint always uses the server key — no user key available.
    Ok(Json(fetch_performance(&state, &state.fmp.clone(), portfolio_id).await?))
}
