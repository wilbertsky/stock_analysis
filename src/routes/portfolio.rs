use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use futures::future::{join_all, try_join_all};
use sqlx::Row;
use uuid::Uuid;
use crate::{
    auth::middleware::AuthUser,
    error::AppError,
    models::{
        AddHoldingRequest, CreatePortfolioRequest, HoldingPerformance, HoldingRow,
        ImportHoldingsResponse, ImportRowResult, PortfolioPerformanceResponse, PortfolioRow,
        PublicPortfolioSummary, RealizedGainRow, RealizedGainsSummary, SellHoldingRequest,
    },
    state::AppState,
};

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

/// Parsed row from either a CSV or XLSX import file.
struct ImportRow {
    row_num: usize,
    ticker: String,
    date: Option<chrono::NaiveDate>,
    shares: Option<f64>,
    price_override: Option<f64>,
}

type ParseErrors = Vec<(usize, String, String)>; // (row_num, ticker, msg)

// ── CSV parser ────────────────────────────────────────────────────────────────

fn parse_csv_bytes(bytes: &[u8]) -> Result<(Vec<ImportRow>, ParseErrors), AppError> {
    let mut rdr = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(bytes);

    let headers = rdr
        .headers()
        .map_err(|e| AppError::BadRequest(format!("CSV header error: {e}")))?
        .clone();

    let find_col = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));

    let ticker_col = find_col("ticker")
        .ok_or_else(|| AppError::BadRequest("CSV must have a 'ticker' column".into()))?;
    let date_col   = find_col("date");
    let shares_col = find_col("shares");
    let price_col  = find_col("price");

    let mut parse_errors = ParseErrors::new();
    let mut valid_rows: Vec<ImportRow> = Vec::new();

    for (i, result) in rdr.records().enumerate() {
        let row_num = i + 2;
        let record = match result {
            Err(e) => { parse_errors.push((row_num, String::new(), e.to_string())); continue; }
            Ok(r) => r,
        };

        let ticker = record.get(ticker_col).unwrap_or("").trim().to_uppercase();
        if ticker.is_empty() {
            parse_errors.push((row_num, String::new(), "Empty ticker".into()));
            continue;
        }

        let date = date_col.and_then(|c| {
            let s = record.get(c).unwrap_or("").trim();
            if s.is_empty() { return None; }
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .or_else(|_| chrono::NaiveDate::parse_from_str(s, "%m/%d/%Y"))
                .ok()
        });

        if let Some(d) = date {
            if d >= Utc::now().date_naive() {
                parse_errors.push((row_num, ticker, "Date must be in the past".into()));
                continue;
            }
        }

        let shares = shares_col.and_then(|c| {
            let s = record.get(c).unwrap_or("").trim();
            if s.is_empty() { None } else { s.parse::<f64>().ok() }
        });

        let price_override = price_col.and_then(|c| {
            let s = record.get(c).unwrap_or("").trim();
            if s.is_empty() { None } else { s.parse::<f64>().ok() }
        });

        valid_rows.push(ImportRow { row_num, ticker, date, shares, price_override });
    }

    Ok((valid_rows, parse_errors))
}

// ── XLSX / XLS parser ─────────────────────────────────────────────────────────

fn parse_xlsx_bytes(bytes: &[u8]) -> Result<(Vec<ImportRow>, ParseErrors), AppError> {
    use calamine::{open_workbook_auto_from_rs, Data, DataType, Reader};

    let cursor = std::io::Cursor::new(bytes);
    let mut wb = open_workbook_auto_from_rs(cursor)
        .map_err(|e| AppError::BadRequest(format!("Cannot open spreadsheet: {e}")))?;

    let sheet_name = wb
        .sheet_names()
        .first()
        .ok_or_else(|| AppError::BadRequest("Spreadsheet has no sheets".into()))?
        .clone();

    let range = wb
        .worksheet_range(&sheet_name)
        .map_err(|e| AppError::BadRequest(format!("Cannot read sheet '{sheet_name}': {e}")))?;

    let mut iter = range.rows();
    let header_row = iter
        .next()
        .ok_or_else(|| AppError::BadRequest("Spreadsheet is empty".into()))?;

    let find_col = |name: &str| -> Option<usize> {
        header_row.iter().position(|cell: &Data| {
            cell.get_string()
                .map(|s| s.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
    };

    let ticker_col = find_col("ticker")
        .ok_or_else(|| AppError::BadRequest("Spreadsheet must have a 'ticker' column".into()))?;
    let date_col   = find_col("date");
    let shares_col = find_col("shares");
    let price_col  = find_col("price");

    let mut parse_errors = ParseErrors::new();
    let mut valid_rows: Vec<ImportRow> = Vec::new();

    for (i, row) in iter.enumerate() {
        let row_num = i + 2;

        // Silently skip wholly-empty trailing rows (common in Excel exports).
        if row.iter().all(|c| matches!(c, Data::Empty)) { continue; }

        let ticker = row.get(ticker_col)
            .and_then(|c: &Data| c.get_string())
            .unwrap_or("")
            .trim()
            .to_uppercase();
        if ticker.is_empty() {
            parse_errors.push((row_num, String::new(), "Empty ticker".into()));
            continue;
        }

        let date = date_col
            .and_then(|c| row.get(c))
            .and_then(|cell| xlsx_cell_to_date(cell));

        if let Some(d) = date {
            if d >= Utc::now().date_naive() {
                parse_errors.push((row_num, ticker, "Date must be in the past".into()));
                continue;
            }
        }

        let shares        = shares_col.and_then(|c| row.get(c)).and_then(xlsx_cell_to_f64);
        let price_override = price_col.and_then(|c| row.get(c)).and_then(xlsx_cell_to_f64);

        valid_rows.push(ImportRow { row_num, ticker, date, shares, price_override });
    }

    Ok((valid_rows, parse_errors))
}

fn xlsx_cell_to_f64(cell: &calamine::Data) -> Option<f64> {
    match cell {
        calamine::Data::Float(f) => Some(*f),
        calamine::Data::Int(i)   => Some(*i as f64),
        calamine::Data::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn xlsx_cell_to_date(cell: &calamine::Data) -> Option<chrono::NaiveDate> {
    match cell {
        calamine::Data::DateTime(d) => {
            // calamine 0.26 `dates` feature: as_datetime() returns NaiveDateTime.
            d.as_datetime().map(|dt| dt.date())
        }
        calamine::Data::DateTimeIso(s) => {
            // OOXML ISO timestamp: "2023-01-15" or "2023-01-15T00:00:00"
            chrono::NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()
        }
        calamine::Data::String(s) => {
            let s = s.trim();
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .or_else(|_| chrono::NaiveDate::parse_from_str(s, "%m/%d/%Y"))
                .ok()
        }
        _ => None,
    }
}

async fn fetch_performance(
    state: &AppState,
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

    // Fetch current prices concurrently via Yahoo (providers.quote_price).
    let price_futs: Vec<_> = holdings_data
        .iter()
        .map(|h| {
            let ticker = h.ticker.clone();
            async move { state.providers.quote_price(&ticker).await }
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

    // Fetch realized gains for this portfolio.
    let rg_rows = sqlx::query(
        "SELECT id, ticker,
                shares::FLOAT8         AS shares,
                cost_per_share::FLOAT8 AS cost_per_share,
                sale_price::FLOAT8     AS sale_price,
                realized_gain::FLOAT8  AS realized_gain,
                sold_at
         FROM realized_gains WHERE portfolio_id = $1
         ORDER BY sold_at DESC",
    )
    .bind(portfolio_id)
    .fetch_all(&state.db)
    .await?;

    let realized_rows: Vec<RealizedGainRow> = rg_rows
        .iter()
        .map(|r| {
            Ok::<_, AppError>(RealizedGainRow {
                id:             r.try_get("id").map_err(AppError::Db)?,
                ticker:         r.try_get("ticker").map_err(AppError::Db)?,
                shares:         r.try_get::<f64, _>("shares").map_err(AppError::Db)?,
                cost_per_share: r.try_get::<f64, _>("cost_per_share").map_err(AppError::Db)?,
                sale_price:     r.try_get::<f64, _>("sale_price").map_err(AppError::Db)?,
                realized_gain:  r.try_get::<f64, _>("realized_gain").map_err(AppError::Db)?,
                sold_at:        r.try_get("sold_at").map_err(AppError::Db)?,
            })
        })
        .collect::<Result<_, _>>()?;

    let total_realized_gain = realized_rows.iter().map(|r| r.realized_gain).sum();

    Ok(PortfolioPerformanceResponse {
        portfolio,
        holdings,
        total_return_pct,
        realized: RealizedGainsSummary {
            rows: realized_rows,
            total_realized_gain,
        },
    })
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
    Ok(Json(fetch_performance(&state, id).await?))
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
    description = "Add a ticker to a portfolio. When `date` is omitted the current price is used. \
        When `date` is provided (YYYY-MM-DD), the closing price on that date (or the nearest prior \
        trading day) is fetched from Yahoo Finance history and stored as the cost basis.",
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Holding added", body = HoldingRow),
        (status = 400, description = "Invalid date or ticker", body = crate::error::ErrorBody),
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

    let (price, added_at): (f64, DateTime<Utc>) = match body.date {
        Some(date) => {
            if date >= Utc::now().date_naive() {
                return Err(AppError::BadRequest(
                    "Date must be in the past".into(),
                ));
            }
            let p = state.providers.price_on_date(&ticker, date).await?;
            let ts = date.and_hms_opt(0, 0, 0)
                .ok_or_else(|| AppError::BadRequest("Invalid date".into()))?
                .and_utc();
            (p, ts)
        }
        None => (state.providers.quote_price(&ticker).await?, Utc::now()),
    };

    let row = sqlx::query(
        "INSERT INTO portfolio_holdings (portfolio_id, ticker, price_at_add, shares, added_at)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, ticker,
                   price_at_add::FLOAT8 AS price_at_add,
                   shares::FLOAT8       AS shares,
                   added_at",
    )
    .bind(id)
    .bind(&ticker)
    .bind(price)
    .bind(body.shares)
    .bind(added_at)
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

// ── POST /api/portfolio/{id}/holdings/{holding_id}/sell ──────────────────────

#[utoipa::path(
    post,
    path = "/api/portfolio/{id}/holdings/{holding_id}/sell",
    tag = "portfolio",
    params(
        ("id"         = Uuid, Path, description = "Portfolio UUID"),
        ("holding_id" = Uuid, Path, description = "Holding UUID"),
    ),
    request_body = SellHoldingRequest,
    description = "Record a (partial or full) sale of a holding. \
        The cost basis is taken from `price_at_add` on the holding. \
        For a partial sale the holding's share count is reduced. \
        For a full sale the holding is removed. \
        The realized gain is recorded in `realized_gains`.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Realized gain row", body = RealizedGainRow),
        (status = 400, description = "Invalid request", body = crate::error::ErrorBody),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
        (status = 404, description = "Portfolio or holding not found", body = crate::error::ErrorBody),
    )
)]
pub async fn sell_holding(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, holding_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SellHoldingRequest>,
) -> Result<Json<RealizedGainRow>, AppError> {
    if body.shares <= 0.0 {
        return Err(AppError::BadRequest("shares must be positive".into()));
    }
    assert_owns_portfolio(&state, id, auth.user_id).await?;

    // Fetch the holding to get ticker, cost basis, and current share count.
    let h_row = sqlx::query(
        "SELECT ticker, price_at_add::FLOAT8 AS price_at_add, shares::FLOAT8 AS shares
         FROM portfolio_holdings WHERE id = $1 AND portfolio_id = $2",
    )
    .bind(holding_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let ticker: String = h_row.try_get("ticker").map_err(AppError::Db)?;
    let cost_per_share: f64 = h_row.try_get("price_at_add").map_err(AppError::Db)?;
    let current_shares: Option<f64> = h_row.try_get("shares").map_err(AppError::Db)?;

    // Validate share count if known.
    if let Some(owned) = current_shares {
        if body.shares > owned + 1e-9 {
            return Err(AppError::BadRequest(format!(
                "Cannot sell {:.6} shares; holding only has {:.6}",
                body.shares, owned
            )));
        }
    }

    // Determine sale price.
    let (sale_price, sold_at) = match body.date {
        Some(date) => {
            if date >= Utc::now().date_naive() {
                return Err(AppError::BadRequest("date must be in the past".into()));
            }
            let p = if let Some(override_price) = body.price {
                override_price
            } else {
                state.providers.price_on_date(&ticker, date).await?
            };
            let ts = date.and_hms_opt(0, 0, 0)
                .ok_or_else(|| AppError::BadRequest("Invalid date".into()))?
                .and_utc();
            (p, ts)
        }
        None => {
            let p = if let Some(override_price) = body.price {
                override_price
            } else {
                state.providers.quote_price(&ticker).await?
            };
            (p, Utc::now())
        }
    };

    let realized_gain = (sale_price - cost_per_share) * body.shares;

    // Perform the update and insert in a transaction.
    let mut tx = state.db.begin().await?;

    // Insert realized gain record.
    let rg_row = sqlx::query(
        "INSERT INTO realized_gains (portfolio_id, ticker, shares, cost_per_share, sale_price, realized_gain, sold_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, ticker,
                   shares::FLOAT8         AS shares,
                   cost_per_share::FLOAT8 AS cost_per_share,
                   sale_price::FLOAT8     AS sale_price,
                   realized_gain::FLOAT8  AS realized_gain,
                   sold_at",
    )
    .bind(id)
    .bind(&ticker)
    .bind(body.shares)
    .bind(cost_per_share)
    .bind(sale_price)
    .bind(realized_gain)
    .bind(sold_at)
    .fetch_one(&mut *tx)
    .await?;

    // Reduce or remove the holding.
    if let Some(owned) = current_shares {
        let remaining = owned - body.shares;
        if remaining < 1e-9 {
            // Full sell — remove the holding.
            sqlx::query("DELETE FROM portfolio_holdings WHERE id = $1")
                .bind(holding_id)
                .execute(&mut *tx)
                .await?;
        } else {
            // Partial sell — update share count.
            sqlx::query("UPDATE portfolio_holdings SET shares = $1 WHERE id = $2")
                .bind(remaining)
                .bind(holding_id)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        // No share count recorded; treat as full sell.
        sqlx::query("DELETE FROM portfolio_holdings WHERE id = $1")
            .bind(holding_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    Ok(Json(RealizedGainRow {
        id:             rg_row.try_get("id").map_err(AppError::Db)?,
        ticker:         rg_row.try_get("ticker").map_err(AppError::Db)?,
        shares:         rg_row.try_get::<f64, _>("shares").map_err(AppError::Db)?,
        cost_per_share: rg_row.try_get::<f64, _>("cost_per_share").map_err(AppError::Db)?,
        sale_price:     rg_row.try_get::<f64, _>("sale_price").map_err(AppError::Db)?,
        realized_gain:  rg_row.try_get::<f64, _>("realized_gain").map_err(AppError::Db)?,
        sold_at:        rg_row.try_get("sold_at").map_err(AppError::Db)?,
    }))
}

// ── POST /api/portfolio/{id}/holdings/import ──────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/portfolio/{id}/holdings/import",
    tag = "portfolio",
    params(("id" = Uuid, Path, description = "Portfolio UUID")),
    description = "Bulk-import holdings from a CSV or XLSX/XLS file uploaded as `multipart/form-data`.\n\n\
        **Required column:** `ticker`\n\
        **Optional columns:** `date` (YYYY-MM-DD or MM/DD/YYYY), `shares`, `price`\n\n\
        The format is auto-detected from the `Content-Type` header or file extension. \
        When `date` is present the closing price on that date is fetched from Yahoo Finance \
        history. When `price` is provided it is used directly, skipping the lookup entirely. \
        All prices are fetched concurrently before a single transaction inserts the successful \
        rows — individual failures are reported per-row without rolling back the rest.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Import results", body = ImportHoldingsResponse),
        (status = 400, description = "Missing file or invalid CSV", body = crate::error::ErrorBody),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
        (status = 403, description = "Not your portfolio", body = crate::error::ErrorBody),
    )
)]
pub async fn import_holdings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<ImportHoldingsResponse>, AppError> {
    assert_owns_portfolio(&state, id, auth.user_id).await?;

    // ── 1. Extract file field, detect format, read bytes ─────────────────────
    let field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
        .ok_or_else(|| AppError::BadRequest("No file uploaded".into()))?;

    // Detect before consuming the field (.bytes() takes ownership).
    let is_xlsx = field
        .content_type()
        .map(|ct| ct.contains("spreadsheetml") || ct.contains("ms-excel"))
        .unwrap_or(false)
        || field
            .file_name()
            .map(|n| n.ends_with(".xlsx") || n.ends_with(".xls"))
            .unwrap_or(false);

    let bytes = field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // ── 2. Parse rows (delegates to format-specific parser) ───────────────────
    let (valid_rows, parse_errors) = if is_xlsx {
        parse_xlsx_bytes(&bytes)?
    } else {
        parse_csv_bytes(&bytes)?
    };

    if valid_rows.is_empty() && parse_errors.is_empty() {
        return Err(AppError::BadRequest("File has no data rows".into()));
    }

    // ── 3. Fetch all prices concurrently ──────────────────────────────────────
    let state_ref = &state;
    let price_futs: Vec<_> = valid_rows
        .iter()
        .map(|row| {
            let ticker = row.ticker.clone();
            let date   = row.date;
            let p_ovr  = row.price_override;
            async move {
                if let Some(p) = p_ovr {
                    Ok(p)
                } else if let Some(d) = date {
                    state_ref.providers.price_on_date(&ticker, d).await
                } else {
                    state_ref.providers.quote_price(&ticker).await
                }
            }
        })
        .collect();

    let prices: Vec<Result<f64, AppError>> = join_all(price_futs).await;

    // ── 4. Insert successful rows in a single transaction ─────────────────────
    let mut tx = state.db.begin().await?;
    let mut rows:     Vec<ImportRowResult> = Vec::new();
    let mut imported: usize                = 0;
    let mut failed:   usize                = 0;

    for (row, price_result) in valid_rows.into_iter().zip(prices) {
        match price_result {
            Err(e) => {
                failed += 1;
                rows.push(ImportRowResult {
                    row: row.row_num, ticker: row.ticker,
                    ok: false, price_used: None,
                    error: Some(e.to_string()),
                });
            }
            Ok(price) => {
                let added_at: DateTime<Utc> = row
                    .date
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map(|dt| dt.and_utc())
                    .unwrap_or_else(Utc::now);

                match sqlx::query(
                    "INSERT INTO portfolio_holdings
                     (portfolio_id, ticker, price_at_add, shares, added_at)
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(id)
                .bind(&row.ticker)
                .bind(price)
                .bind(row.shares)
                .bind(added_at)
                .execute(&mut *tx)
                .await
                {
                    Ok(_) => {
                        imported += 1;
                        rows.push(ImportRowResult {
                            row: row.row_num, ticker: row.ticker,
                            ok: true, price_used: Some(price), error: None,
                        });
                    }
                    Err(e) => {
                        failed += 1;
                        rows.push(ImportRowResult {
                            row: row.row_num, ticker: row.ticker,
                            ok: false, price_used: None,
                            error: Some(format!("DB error: {e}")),
                        });
                    }
                }
            }
        }
    }

    for (row_num, ticker, msg) in parse_errors {
        failed += 1;
        rows.push(ImportRowResult {
            row: row_num, ticker,
            ok: false, price_used: None, error: Some(msg),
        });
    }

    tx.commit().await?;
    rows.sort_by_key(|r| r.row);

    Ok(Json(ImportHoldingsResponse { imported, failed, rows }))
}

// ── GET /api/community ────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/community",
    tag = "portfolio",
    description = "List all public portfolios from all subscribers, sorted by unrealized gains (highest first). \
        Portfolios with no holdings are excluded.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Ranked public portfolios", body = Vec<PublicPortfolioSummary>),
        (status = 401, description = "Missing or invalid token", body = crate::error::ErrorBody),
    )
)]
pub async fn get_community_portfolios(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<PublicPortfolioSummary>>, AppError> {
    // All public portfolios with owner email.
    let p_rows = sqlx::query(
        "SELECT p.id, p.name, u.email, u.display_name
         FROM portfolios p
         JOIN users u ON u.id = p.user_id
         WHERE p.is_public = true
         ORDER BY p.created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;

    if p_rows.is_empty() {
        return Ok(Json(vec![]));
    }

    struct PortfolioMeta {
        id: Uuid,
        name: String,
        owner: String,
    }

    let portfolios: Vec<PortfolioMeta> = p_rows
        .iter()
        .map(|r| {
            let email: String = r.try_get("email").map_err(AppError::Db)?;
            let display_name: Option<String> = r.try_get("display_name").map_err(AppError::Db)?;
            let owner = display_name.unwrap_or_else(|| {
                email.split('@').next().unwrap_or(&email).to_owned()
            });
            Ok(PortfolioMeta {
                id: r.try_get("id").map_err(AppError::Db)?,
                name: r.try_get("name").map_err(AppError::Db)?,
                owner,
            })
        })
        .collect::<Result<_, AppError>>()?;

    let portfolio_ids: Vec<Uuid> = portfolios.iter().map(|p| p.id).collect();

    // Realized gains for all these portfolios in one round-trip.
    let rg_rows = sqlx::query(
        "SELECT portfolio_id, id, ticker,
                shares::FLOAT8         AS shares,
                cost_per_share::FLOAT8 AS cost_per_share,
                sale_price::FLOAT8     AS sale_price,
                realized_gain::FLOAT8  AS realized_gain,
                sold_at
         FROM realized_gains
         WHERE portfolio_id = ANY($1)
         ORDER BY sold_at DESC",
    )
    .bind(portfolio_ids.as_slice())
    .fetch_all(&state.db)
    .await?;

    let mut realized_by_portfolio: std::collections::HashMap<Uuid, Vec<RealizedGainRow>> =
        std::collections::HashMap::new();
    for r in &rg_rows {
        let pid: Uuid = r.try_get("portfolio_id").map_err(AppError::Db)?;
        realized_by_portfolio.entry(pid).or_default().push(RealizedGainRow {
            id:             r.try_get("id").map_err(AppError::Db)?,
            ticker:         r.try_get("ticker").map_err(AppError::Db)?,
            shares:         r.try_get::<f64, _>("shares").map_err(AppError::Db)?,
            cost_per_share: r.try_get::<f64, _>("cost_per_share").map_err(AppError::Db)?,
            sale_price:     r.try_get::<f64, _>("sale_price").map_err(AppError::Db)?,
            realized_gain:  r.try_get::<f64, _>("realized_gain").map_err(AppError::Db)?,
            sold_at:        r.try_get("sold_at").map_err(AppError::Db)?,
        });
    }

    // All holdings for these portfolios in one round-trip.
    let h_rows = sqlx::query(
        "SELECT id, portfolio_id, ticker,
                price_at_add::FLOAT8 AS price_at_add,
                shares::FLOAT8       AS shares,
                added_at
         FROM portfolio_holdings
         WHERE portfolio_id = ANY($1)
         ORDER BY added_at ASC",
    )
    .bind(portfolio_ids.as_slice())
    .fetch_all(&state.db)
    .await?;

    // Group holdings by portfolio_id.
    let mut holdings_by_portfolio: std::collections::HashMap<Uuid, Vec<HoldingRecord>> =
        std::collections::HashMap::new();
    for r in &h_rows {
        let pid: Uuid = r.try_get("portfolio_id").map_err(AppError::Db)?;
        holdings_by_portfolio.entry(pid).or_default().push(HoldingRecord {
            id: r.try_get("id").map_err(AppError::Db)?,
            ticker: r.try_get("ticker").map_err(AppError::Db)?,
            price_at_add: r.try_get::<f64, _>("price_at_add").map_err(AppError::Db)?,
            shares: r.try_get::<Option<f64>, _>("shares").map_err(AppError::Db)?,
            added_at: r.try_get("added_at").map_err(AppError::Db)?,
        });
    }

    // Fetch current prices for every unique ticker concurrently.
    let unique_tickers: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        h_rows
            .iter()
            .filter_map(|r| {
                let t: String = r.try_get("ticker").ok()?;
                seen.insert(t.clone()).then_some(t)
            })
            .collect()
    };

    let state_ref = &state;
    let price_futs: Vec<_> = unique_tickers
        .iter()
        .map(|ticker| {
            let ticker = ticker.clone();
            async move {
                let price = state_ref.providers.quote_price(&ticker).await.ok();
                (ticker, price)
            }
        })
        .collect();
    let price_map: std::collections::HashMap<String, f64> = join_all(price_futs)
        .await
        .into_iter()
        .filter_map(|(t, p)| p.map(|price| (t, price)))
        .collect();

    // Build a summary for each portfolio.
    // Include portfolios that have realized gains even if all positions are closed.
    let mut summaries: Vec<PublicPortfolioSummary> = portfolios
        .into_iter()
        .filter_map(|p| {
            let realized_rows = realized_by_portfolio.remove(&p.id).unwrap_or_default();
            let total_realized_gain: f64 = realized_rows.iter().map(|r| r.realized_gain).sum();
            let realized = RealizedGainsSummary {
                total_realized_gain,
                rows: realized_rows,
            };

            let holdings_data = holdings_by_portfolio.get(&p.id);
            let holdings: Vec<HoldingPerformance> = holdings_data
                .map(|data| {
                    data.iter()
                        .filter_map(|h| {
                            let current_price = *price_map.get(&h.ticker)?;
                            let return_pct = if h.price_at_add > 0.0 {
                                (current_price / h.price_at_add - 1.0) * 100.0
                            } else {
                                0.0
                            };
                            Some(HoldingPerformance {
                                holding: HoldingRow {
                                    id: h.id,
                                    ticker: h.ticker.clone(),
                                    price_at_add: h.price_at_add,
                                    shares: h.shares,
                                    added_at: h.added_at,
                                },
                                current_price,
                                return_pct,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Skip portfolios with no activity at all.
            if holdings.is_empty() && realized.rows.is_empty() {
                return None;
            }

            let total_cost: f64 = holdings
                .iter()
                .filter_map(|h| h.holding.shares.map(|s| s * h.holding.price_at_add))
                .sum();

            let total_return_pct = if !holdings.is_empty() {
                if total_cost > 0.0 {
                    Some(
                        holdings
                            .iter()
                            .filter_map(|h| {
                                h.holding.shares.map(|s| {
                                    let w = (s * h.holding.price_at_add) / total_cost;
                                    w * h.return_pct
                                })
                            })
                            .sum::<f64>(),
                    )
                } else {
                    Some(
                        holdings.iter().map(|h| h.return_pct).sum::<f64>()
                            / holdings.len() as f64,
                    )
                }
            } else {
                None
            };

            // Dollar unrealized gain: (current - cost) * shares for positions with share counts.
            let total_unrealized_gain: f64 = holdings
                .iter()
                .filter_map(|h| {
                    h.holding.shares.map(|s| (h.current_price - h.holding.price_at_add) * s)
                })
                .sum();

            let combined_gain = total_realized_gain + total_unrealized_gain;

            Some(PublicPortfolioSummary {
                id: p.id,
                name: p.name,
                owner: p.owner,
                holdings,
                total_return_pct,
                total_unrealized_gain,
                realized,
                combined_gain,
            })
        })
        .collect();

    // Rank by combined dollar P&L (realized + unrealized) descending.
    summaries.sort_by(|a, b| {
        b.combined_gain
            .partial_cmp(&a.combined_gain)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(summaries))
}

// ── GET /api/portfolio/public/{share_token} ───────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/portfolio/public/{share_token}",
    tag = "portfolio",
    params(("share_token" = Uuid, Path, description = "Portfolio share token")),
    description = "View a public portfolio's performance without authentication. \
        Only works if the portfolio owner has set `is_public: true`.",
    security(()),
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

    Ok(Json(fetch_performance(&state, portfolio_id).await?))
}
