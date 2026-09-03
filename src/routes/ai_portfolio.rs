//! AI-managed portfolio — automatic quarterly selection and rebalancing.
//!
//! Selection algorithm (hybrid):
//!   1. Run all 11 sector screeners (in-memory cache means this is fast).
//!   2. Guarantee one pick per sector (diversity baseline) — 11 slots.
//!   3. Fill remaining slots (up to MAX_HOLDINGS = 20) with the next-highest
//!      composite scorers across all sectors, capped at MAX_PER_SECTOR = 3.
//!   4. For each candidate, query the RAG app for news sentiment. If sentiment
//!      is "negative" AND composite score < 50, veto the pick and try the next.
//!   5. Allocate QUARTERLY_ALLOCATION / n_new_picks per new holding.
//!
//! Rebalancing (condition-based, not time-locked):
//!   - Existing holdings are re-scored each quarter.
//!   - A holding is replaced when its current composite score falls below
//!     REPLACEMENT_SCORE_THRESHOLD AND its current news sentiment is "negative".
//!   - Both signals must align; either one alone is not enough.
//!
//! Endpoints:
//!   GET  /api/ai-portfolio          — public, returns current state
//!   POST /api/ai-portfolio/rebalance — protected by X-Rebalance-Secret header

use std::collections::{HashMap, HashSet};

use axum::{extract::State, http::HeaderMap, Json};
use chrono::{Datelike, Utc};
use futures::future::join_all;
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{AiHoldingDetail, AiPortfolioResponse, AiRebalanceResponse, HoldingPerformance, HoldingRow},
    routes::screener::{company_key, run_screener, DISCLAIMER},
    sectors,
    state::AppState,
};

const QUARTERLY_ALLOCATION: f64 = 1_500.0;
const MAX_HOLDINGS: usize = 20;
const MAX_PER_SECTOR: usize = 3;
const REPLACEMENT_SCORE_THRESHOLD: f64 = 40.0;
const SENTIMENT_VETO_SCORE_THRESHOLD: f64 = 50.0;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn current_cycle() -> String {
    let now = Utc::now();
    let q = (now.month0() / 3) + 1;
    format!("{}-Q{}", now.year(), q)
}

fn next_review_date() -> String {
    let now = Utc::now();
    let current_q = now.month0() / 3;
    let (next_year, next_month) = if current_q == 3 {
        (now.year() + 1, 1u32)
    } else {
        (now.year(), (current_q + 1) * 3 + 1)
    };
    format!("{}-{:02}-01", next_year, next_month)
}

struct Candidate {
    ticker: String,
    sector: String,
    composite_score: f64,
    score_a: f64,
    score_b: f64,
    score_c: f64,
    score_d: f64,
}

/// Calls the RAG app's lightweight sentiment endpoint. Returns None when the RAG
/// app is not configured, times out, or returns a non-200 response — callers
/// treat None as "neutral" (no veto applied).
async fn get_news_sentiment(
    http_client: &reqwest::Client,
    rag_url: &str,
    rag_secret: Option<&str>,
    ticker: &str,
) -> Option<String> {
    let url = format!("{}/api/sentiment/{}", rag_url.trim_end_matches('/'), ticker);
    let mut req = http_client
        .get(&url)
        .timeout(std::time::Duration::from_secs(12));
    if let Some(secret) = rag_secret {
        req = req.header("X-Rag-Secret", secret);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json["sentiment"].as_str().map(|s| s.to_owned())
}

/// Ensures the single AI portfolio exists, creating it if needed.
/// Returns the portfolio UUID.
async fn ensure_ai_portfolio(db: &sqlx::PgPool, owner_id: Uuid) -> Result<Uuid, AppError> {
    let existing = sqlx::query("SELECT id FROM portfolios WHERE is_ai_generated = true LIMIT 1")
        .fetch_optional(db)
        .await?;

    if let Some(row) = existing {
        return Ok(row.try_get("id").map_err(AppError::Db)?);
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO portfolios (user_id, name, is_public, is_ai_generated)
         VALUES ($1, 'AI-Managed Portfolio', true, true)
         RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(db)
    .await?;

    tracing::info!("Created AI portfolio {id}");
    Ok(id)
}

/// Runs all sector screeners in parallel and applies the hybrid selection algorithm.
/// Returns ordered candidates (best first).
async fn select_candidates(state: &AppState) -> Vec<Candidate> {
    let sectors = sectors::ALL_SECTOR_SLUGS;

    let tasks: Vec<_> = sectors
        .iter()
        .map(|sector| {
            let state = state.clone();
            let sector = sector.to_string();
            tokio::spawn(async move {
                run_screener(&state, &sector)
                    .await
                    .ok()
                    .map(|r| (sector, r))
            })
        })
        .collect();

    let outcomes = join_all(tasks).await;

    let mut by_sector: HashMap<String, Vec<Candidate>> = HashMap::new();
    for outcome in outcomes {
        if let Ok(Some((sector, resp))) = outcome {
            let candidates: Vec<Candidate> = resp
                .results
                .into_iter()
                .map(|e| Candidate {
                    ticker: e.ticker,
                    sector: sector.clone(),
                    composite_score: e.composite_score,
                    score_a: e.score_a,
                    score_b: e.score_b,
                    score_c: e.score_c,
                    score_d: e.score_d,
                })
                .collect();
            by_sector.insert(sector, candidates);
        }
    }

    // Build global ticker set for cross-sector company-key dedup (e.g. BRK-A/BRK-B
    // appearing in different sectors — rare but possible with broad sector lists).
    let all_tickers: HashSet<String> = by_sector.values().flatten()
        .map(|c| c.ticker.clone()).collect();

    let mut selected: Vec<Candidate> = Vec::new();
    let mut selected_tickers: HashSet<String> = HashSet::new();
    let mut selected_companies: HashSet<String> = HashSet::new();
    let mut per_sector_count: HashMap<String, usize> = HashMap::new();

    // Step 1 — guaranteed best-per-sector.
    for sector in sectors {
        if let Some(candidates) = by_sector.get(*sector) {
            if let Some(top) = candidates.first() {
                let co_key = company_key(&top.ticker, &all_tickers);
                if selected_tickers.insert(top.ticker.clone()) && selected_companies.insert(co_key) {
                    *per_sector_count.entry(top.sector.clone()).or_insert(0) += 1;
                    selected.push(Candidate {
                        ticker: top.ticker.clone(),
                        sector: top.sector.clone(),
                        composite_score: top.composite_score,
                        score_a: top.score_a,
                        score_b: top.score_b,
                        score_c: top.score_c,
                        score_d: top.score_d,
                    });
                }
            }
        }
    }

    // Step 2 — fill remaining slots cross-sector, soft cap at MAX_PER_SECTOR.
    let mut all_remaining: Vec<&Candidate> = by_sector
        .values()
        .flatten()
        .filter(|c| !selected_tickers.contains(&c.ticker))
        .collect();
    all_remaining
        .sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap_or(std::cmp::Ordering::Equal));

    for c in all_remaining {
        if selected.len() >= MAX_HOLDINGS {
            break;
        }
        if selected_tickers.contains(&c.ticker) {
            continue;
        }
        let co_key = company_key(&c.ticker, &all_tickers);
        if selected_companies.contains(&co_key) {
            continue;
        }
        if per_sector_count.get(&c.sector).copied().unwrap_or(0) >= MAX_PER_SECTOR {
            continue;
        }
        selected_tickers.insert(c.ticker.clone());
        selected_companies.insert(co_key);
        *per_sector_count.entry(c.sector.clone()).or_insert(0) += 1;
        selected.push(Candidate {
            ticker: c.ticker.clone(),
            sector: c.sector.clone(),
            composite_score: c.composite_score,
            score_a: c.score_a,
            score_b: c.score_b,
            score_c: c.score_c,
            score_d: c.score_d,
        });
    }

    selected
}

// ── GET /api/ai-portfolio ─────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/ai-portfolio",
    tag = "ai-portfolio",
    security(()),
    description = "Returns the current state of the AI-managed portfolio: holdings with live \
        performance, selection rationale, factor scores at time of selection, and portfolio-level stats.",
    responses(
        (status = 200, description = "AI portfolio state", body = AiPortfolioResponse),
        (status = 404, description = "AI portfolio has not been created yet"),
    )
)]
pub async fn get_ai_portfolio(
    State(state): State<AppState>,
) -> Result<Json<AiPortfolioResponse>, AppError> {
    let portfolio_row = sqlx::query(
        "SELECT id, name FROM portfolios WHERE is_ai_generated = true LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let portfolio_id: Uuid = portfolio_row.try_get("id").map_err(AppError::Db)?;
    let name: String = portfolio_row.try_get("name").map_err(AppError::Db)?;

    // Load holdings with selection metadata joined.
    let rows = sqlx::query(
        "SELECT
            h.id, h.ticker, h.price_at_add::FLOAT8 AS price_at_add,
            h.shares::FLOAT8 AS shares, h.added_at,
            s.sector, s.composite_score::FLOAT8 AS composite_score,
            s.news_sentiment, s.selection_rationale, s.cycle
         FROM portfolio_holdings h
         LEFT JOIN LATERAL (
             SELECT sector, composite_score, news_sentiment, selection_rationale, cycle
             FROM ai_portfolio_selections
             WHERE portfolio_id = $1 AND ticker = h.ticker
             ORDER BY selected_at DESC LIMIT 1
         ) s ON true
         WHERE h.portfolio_id = $1
         ORDER BY h.added_at DESC",
    )
    .bind(portfolio_id)
    .fetch_all(&state.db)
    .await?;

    if rows.is_empty() {
        return Ok(Json(AiPortfolioResponse {
            portfolio_id,
            name,
            holdings: vec![],
            total_return_pct: None,
            total_value: 0.0,
            total_invested: 0.0,
            current_cycle: current_cycle(),
            next_review: next_review_date(),
            quarterly_allocation: QUARTERLY_ALLOCATION,
            disclaimer: DISCLAIMER.to_owned(),
        }));
    }

    let tickers: Vec<String> = rows
        .iter()
        .map(|r| r.try_get::<String, _>("ticker").unwrap_or_default())
        .collect();

    let providers = state.providers_with_server_fmp();
    let price_tasks: Vec<_> = tickers
        .iter()
        .map(|t| {
            let providers = providers.clone();
            let t = t.clone();
            async move { (t.clone(), providers.quote_price(&t).await.ok()) }
        })
        .collect();
    let prices: HashMap<String, f64> = join_all(price_tasks)
        .await
        .into_iter()
        .filter_map(|(ticker, price)| price.map(|p| (ticker, p)))
        .collect();

    let mut holdings: Vec<AiHoldingDetail> = Vec::new();
    let mut total_value = 0.0f64;
    let mut total_invested = 0.0f64;

    for row in &rows {
        let ticker: String = row.try_get("ticker").map_err(AppError::Db)?;
        let price_at_add: f64 = row.try_get("price_at_add").map_err(AppError::Db)?;
        let shares: Option<f64> = row.try_get("shares").map_err(AppError::Db)?;
        let added_at = row.try_get("added_at").map_err(AppError::Db)?;
        let id: Uuid = row.try_get("id").map_err(AppError::Db)?;

        let current_price = prices.get(&ticker).copied().unwrap_or(price_at_add);
        let return_pct = (current_price / price_at_add - 1.0) * 100.0;

        if let Some(s) = shares {
            total_value += current_price * s;
            total_invested += price_at_add * s;
        }

        holdings.push(AiHoldingDetail {
            performance: HoldingPerformance {
                holding: HoldingRow { id, ticker, price_at_add, shares, added_at },
                current_price,
                return_pct,
            },
            sector: row.try_get("sector").ok(),
            composite_score_at_selection: row.try_get("composite_score").ok(),
            news_sentiment_at_selection: row.try_get("news_sentiment").ok(),
            selection_rationale: row.try_get("selection_rationale").ok(),
            cycle: row.try_get("cycle").ok(),
        });
    }

    let total_return_pct = if total_invested > 0.0 {
        Some((total_value / total_invested - 1.0) * 100.0)
    } else {
        None
    };

    Ok(Json(AiPortfolioResponse {
        portfolio_id,
        name,
        holdings,
        total_return_pct,
        total_value,
        total_invested,
        current_cycle: current_cycle(),
        next_review: next_review_date(),
        quarterly_allocation: QUARTERLY_ALLOCATION,
        disclaimer: DISCLAIMER.to_owned(),
    }))
}

// ── POST /api/ai-portfolio/rebalance ─────────────────────────────────────────

#[utoipa::path(
    post,
    path = "/api/ai-portfolio/rebalance",
    tag = "ai-portfolio",
    description = "Runs the quarterly selection algorithm and rebalances the AI portfolio. \
        Requires X-Rebalance-Secret header matching the AI_REBALANCE_SECRET env var. \
        AI_PORTFOLIO_OWNER_ID env var must be set to a valid user UUID.",
    responses(
        (status = 200, description = "Rebalance summary", body = AiRebalanceResponse),
        (status = 403, description = "Missing or invalid secret"),
        (status = 503, description = "AI_PORTFOLIO_OWNER_ID not configured"),
    )
)]
pub async fn post_rebalance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AiRebalanceResponse>, AppError> {
    // Verify secret.
    let expected = std::env::var("AI_REBALANCE_SECRET").ok();
    if let Some(expected) = &expected {
        let provided = headers
            .get("X-Rebalance-Secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected {
            return Err(AppError::Forbidden);
        }
    }

    let owner_id_str = std::env::var("AI_PORTFOLIO_OWNER_ID").map_err(|_| {
        AppError::Internal("AI_PORTFOLIO_OWNER_ID env var not set".into())
    })?;
    let owner_id = Uuid::parse_str(&owner_id_str)
        .map_err(|_| AppError::Internal("AI_PORTFOLIO_OWNER_ID is not a valid UUID".into()))?;

    let portfolio_id = ensure_ai_portfolio(&state.db, owner_id).await?;
    let cycle = current_cycle();

    tracing::info!("AI rebalance starting for cycle {cycle}");

    // ── Run selection ─────────────────────────────────────────────────────────
    let candidates = select_candidates(&state).await;
    tracing::info!("Selected {} candidates before sentiment veto", candidates.len());

    // ── News sentiment veto (parallel, best-effort) ───────────────────────────
    let rag_url = state.rag_url.clone();
    let rag_secret = state.rag_secret.clone();

    #[derive(Serialize)]
    struct CandidateWithSentiment {
        ticker: String,
        sector: String,
        composite_score: f64,
        score_a: f64,
        score_b: f64,
        score_c: f64,
        score_d: f64,
        sentiment: Option<String>,
    }

    let sentiment_tasks: Vec<_> = candidates
        .into_iter()
        .map(|c| {
            let client = state.http_client.clone();
            let rag_url = rag_url.clone();
            let rag_secret = rag_secret.clone();
            async move {
                let sentiment = if let Some(url) = &rag_url {
                    get_news_sentiment(
                        &client,
                        url,
                        rag_secret.as_deref(),
                        &c.ticker,
                    )
                    .await
                } else {
                    None
                };
                CandidateWithSentiment {
                    ticker: c.ticker,
                    sector: c.sector,
                    composite_score: c.composite_score,
                    score_a: c.score_a,
                    score_b: c.score_b,
                    score_c: c.score_c,
                    score_d: c.score_d,
                    sentiment,
                }
            }
        })
        .collect();

    let all_with_sentiment = join_all(sentiment_tasks).await;

    // Apply veto: negative news + score below threshold.
    let vetted: Vec<CandidateWithSentiment> = all_with_sentiment
        .into_iter()
        .filter(|c| {
            let is_negative = c.sentiment.as_deref() == Some("negative");
            let is_low_score = c.composite_score < SENTIMENT_VETO_SCORE_THRESHOLD;
            if is_negative && is_low_score {
                tracing::info!("Vetoed {} (score={:.1}, sentiment=negative)", c.ticker, c.composite_score);
                false
            } else {
                true
            }
        })
        .collect();

    // ── Load existing holdings and score them ─────────────────────────────────
    let existing_rows = sqlx::query(
        "SELECT id, ticker, price_at_add::FLOAT8 AS price_at_add, shares::FLOAT8 AS shares
         FROM portfolio_holdings WHERE portfolio_id = $1",
    )
    .bind(portfolio_id)
    .fetch_all(&state.db)
    .await?;

    let existing_tickers: HashSet<String> = existing_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ticker").ok())
        .collect();

    // Map current screener scores for existing holdings (best-effort from vetted list).
    let score_map: HashMap<&str, f64> = vetted
        .iter()
        .map(|c| (c.ticker.as_str(), c.composite_score))
        .collect();

    let sentiment_map: HashMap<&str, Option<String>> = vetted
        .iter()
        .map(|c| (c.ticker.as_str(), c.sentiment.clone()))
        .collect();

    // Determine which existing holdings to replace.
    let to_remove: Vec<String> = existing_tickers
        .iter()
        .filter(|t| {
            let score = score_map.get(t.as_str()).copied().unwrap_or(50.0);
            let sentiment = sentiment_map.get(t.as_str()).and_then(|s| s.as_deref());
            let low_score = score < REPLACEMENT_SCORE_THRESHOLD;
            let negative = sentiment == Some("negative");
            low_score && negative
        })
        .cloned()
        .collect();

    // ── Determine new picks (in vetted order, not already held and not removed) ──
    let mut holdings_held: Vec<String> = existing_tickers
        .iter()
        .filter(|t| !to_remove.contains(t))
        .cloned()
        .collect();
    holdings_held.sort();

    let held_set: HashSet<&str> = holdings_held.iter().map(|s| s.as_str()).collect();

    let new_picks: Vec<&CandidateWithSentiment> = vetted
        .iter()
        .filter(|c| !held_set.contains(c.ticker.as_str()))
        .collect();

    let n_new = new_picks.len().min(MAX_HOLDINGS.saturating_sub(holdings_held.len()));
    let new_picks = &new_picks[..n_new];

    let per_holding_allocation = if n_new > 0 {
        QUARTERLY_ALLOCATION / n_new as f64
    } else {
        0.0
    };

    let providers = state.providers_with_server_fmp();

    // ── Execute: remove holdings marked for replacement ───────────────────────
    for ticker in &to_remove {
        // Find the holding rows for this ticker in this portfolio.
        let holding_rows = sqlx::query(
            "SELECT id, shares::FLOAT8 AS shares, price_at_add::FLOAT8 AS price_at_add
             FROM portfolio_holdings WHERE portfolio_id = $1 AND ticker = $2",
        )
        .bind(portfolio_id)
        .bind(ticker)
        .fetch_all(&state.db)
        .await?;

        for hr in holding_rows {
            let holding_id: Uuid = hr.try_get("id").map_err(AppError::Db)?;
            let shares: Option<f64> = hr.try_get("shares").map_err(AppError::Db)?;
            let price_at_add: f64 = hr.try_get("price_at_add").map_err(AppError::Db)?;

            let current_price = providers.quote_price(ticker).await.unwrap_or(price_at_add);
            let sale_shares = shares.unwrap_or(1.0);
            let realized_gain = (current_price - price_at_add) * sale_shares;

            let mut tx = state.db.begin().await?;
            sqlx::query(
                "INSERT INTO realized_gains (portfolio_id, ticker, shares, cost_per_share, sale_price, realized_gain)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(portfolio_id)
            .bind(ticker)
            .bind(sale_shares)
            .bind(price_at_add)
            .bind(current_price)
            .bind(realized_gain)
            .execute(&mut *tx)
            .await?;

            sqlx::query("DELETE FROM portfolio_holdings WHERE id = $1")
                .bind(holding_id)
                .execute(&mut *tx)
                .await?;

            tx.commit().await?;
        }

        tracing::info!("Removed {ticker} from AI portfolio (score+sentiment trigger)");
    }

    // ── Execute: add new picks ────────────────────────────────────────────────
    let mut holdings_added: Vec<String> = Vec::new();
    let mut total_deployed = 0.0f64;

    for pick in new_picks {
        let current_price = match providers.quote_price(&pick.ticker).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Could not price {} for AI portfolio: {e}", pick.ticker);
                continue;
            }
        };

        let shares = per_holding_allocation / current_price;

        sqlx::query(
            "INSERT INTO portfolio_holdings (portfolio_id, ticker, price_at_add, shares)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(portfolio_id)
        .bind(&pick.ticker)
        .bind(current_price)
        .bind(shares)
        .execute(&state.db)
        .await?;

        let rationale = format!(
            "Selected in {} — sector rank composite {:.1}; sentiment: {}",
            cycle,
            pick.composite_score,
            pick.sentiment.as_deref().unwrap_or("unknown"),
        );

        sqlx::query(
            "INSERT INTO ai_portfolio_selections
             (portfolio_id, ticker, sector, composite_score, score_a, score_b, score_c, score_d,
              news_sentiment, selection_rationale, cycle)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(portfolio_id)
        .bind(&pick.ticker)
        .bind(&pick.sector)
        .bind(pick.composite_score)
        .bind(pick.score_a)
        .bind(pick.score_b)
        .bind(pick.score_c)
        .bind(pick.score_d)
        .bind(pick.sentiment.as_deref())
        .bind(&rationale)
        .bind(&cycle)
        .execute(&state.db)
        .await?;

        total_deployed += per_holding_allocation;
        holdings_added.push(pick.ticker.clone());
        tracing::info!("Added {} to AI portfolio ({:.4} shares @ ${:.2})", pick.ticker, shares, current_price);
    }

    Ok(Json(AiRebalanceResponse {
        cycle,
        holdings_added,
        holdings_removed: to_remove,
        holdings_held,
        total_deployed,
        portfolio_id,
    }))
}
