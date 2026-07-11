//! Small/mid-cap "near miss" discovery screener.
//!
//! The sector screener (`screener.rs`) only ever sees large-cap names (market cap ≥
//! $10B) — anything smaller never enters its candidate pool, regardless of how it would
//! score. This is a separate, additive screener: it sources a small/mid-cap universe
//! directly from FMP's company-screener, computes Graham Number and the same quality
//! signals already used elsewhere in this app, and surfaces candidates that are close
//! to fair value (not necessarily deeply undervalued) with fundamentals that clear a
//! quality *floor* rather than requiring a *ceiling*. See `calculations.rs`'s
//! `DISCOVERY_*` constants for the reasoning behind each threshold.
//!
//! Uses Graham Number rather than the DCF intrinsic value used elsewhere in this app
//! (`growth_dcf_valuation`) — verified live that DCF's 10-year EPS-growth compounding
//! produces wildly unstable values across a broad small/mid-cap universe (deviations of
//! -90%+ and +300%+ were common), whereas Graham Number's EPS×BVPS calculation has no
//! compounding and stayed far more bounded.
//!
//! Deliberately does not touch `screener.rs` or its composite scoring model.

use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use tokio::task::JoinSet;

use crate::{
    auth::middleware::AuthUser,
    calculations,
    error::AppError,
    fmp::{FmpClient, ScreenerCandidate},
    models::{DiscoveryEntry, DiscoveryResponse, FundamentalsYear},
    routes::screener::DISCLAIMER,
    sectors,
    state::AppState,
};

/// Small/mid-cap band: standard convention places small-cap at roughly $300M–$2B and
/// mid-cap at $2B–$10B. We use $300M–$5B — wide enough to catch the "falls through the
/// cracks" segment discussed (asset-light, high-ROIC names too small to be carried by
/// the reputation factors that prop up mega-cap composites) without drifting into the
/// micro-cap names whose financial data quality is much less reliable.
const SMALL_MID_CAP_FLOOR: u64 = 300_000_000;
const SMALL_MID_CAP_CEILING: u64 = 5_000_000_000;

/// Number of equal-width sub-ranges to split the market-cap band into when querying
/// FMP. A single company-screener call with a wide marketCapMoreThan/LowerThan range
/// and a `limit` does NOT return a spread across that range — verified live: a single
/// $300M–$5B query with limit=40 returned candidates clustered entirely within
/// $4.86B–$5B, a 3%-wide sliver at the top of the band, never reaching anywhere near
/// the $300M floor. Querying each sub-range separately is what actually guarantees
/// coverage across the whole band, which is the entire point of this screener.
const MARKET_CAP_BUCKETS: u64 = 8;

/// Candidates fetched from FMP per bucket. 8 buckets × 30 ≈ 240 candidates total.
/// FMP-only fundamentals calls keep response time within ~5s even at this budget.
const PER_BUCKET_LIMIT: u32 = 30;

/// How long a cached discovery response stays fresh. Graham Numbers and quality scores
/// are anchored to annual filings, so 12h is a reasonable staleness window.
const CACHE_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug, Deserialize)]
pub struct DiscoveryQuery {
    /// Required sector slug (e.g. "technology"). Kept as `Option` here (rather than a
    /// non-Option field) so a missing param produces a normal `AppError` JSON response
    /// from the handler instead of falling through to Axum's raw query-rejection format.
    pub sector: Option<String>,
    /// Exchange slug to screen. Defaults to "us" (NYSE/NASDAQ). Supported: us, lse.
    pub exchange: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/discovery",
    tag = "discovery",
    params(
        ("sector" = String, Query, description = "Required sector slug, e.g. technology."),
        ("exchange" = Option<String>, Query, description = "Exchange slug: 'us' (default) or 'lse'"),
    ),
    description = "Screens small/mid-cap stocks (market cap $300M–$5B, sourced from FMP's \
        company screener) for candidates close to their Graham Number with fundamentals \
        that clear a quality floor — not the same as the sector screener's blue-chip-biased \
        composite ranking. \
        These are names too small to ever appear in the sector screener, which only evaluates \
        large-cap stocks (market cap ≥ $10B). A candidate qualifies when: (1) its current price \
        is within ±20% of its Graham Number — either slightly undervalued or slightly above, \
        both are surfaced since the Graham Number formula tends to be conservative for \
        asset-light, high-growth names; and (2) it clears a quality floor — quality score ≥ 40, \
        debt safety score ≥ 40, Piotroski F-Score ≥ 4 — chosen to exclude the distress zone, \
        not to require best-in-class fundamentals — unless the underlying data needed to check \
        it (debt-to-equity, gross margin, or return on equity) is unavailable from both EDGAR \
        and FMP, in which case the candidate is included anyway rather than excluded, since \
        missing data isn't evidence of a problem; check `missing_data_fields` on each entry. \
        Results are sorted by closeness to the Graham Number. A sector is required — \
        screening across all sectors at once was tried and dropped: with the same fixed \
        per-bucket candidate budget spread across the whole market instead of concentrated \
        in one sector, it consistently surfaced fewer (often just one) results than picking \
        any individual sector, which was confusing rather than useful. Expect 15–30 seconds \
        response time. A disclaimer field is included in every response.",
    responses(
        (status = 200, description = "Near-miss small/mid-cap candidates", body = DiscoveryResponse),
        (status = 400, description = "Missing sector parameter", body = crate::error::ErrorBody),
        (status = 422, description = "Unknown sector name", body = crate::error::ErrorBody),
        (status = 502, description = "Data provider error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_discovery(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<DiscoveryQuery>,
) -> Result<Json<DiscoveryResponse>, AppError> {
    let sector = params
        .sector
        .ok_or_else(|| AppError::BadRequest("sector query parameter is required".into()))?;
    let fmp_sector = sectors::slug_to_fmp_sector(&sector).ok_or_else(|| {
        AppError::Unprocessable(format!(
            "Unknown sector '{}'. Supported: {}",
            sector,
            sectors::SUPPORTED_SECTORS
        ))
    })?;
    let exchange = params.exchange.as_deref().unwrap_or("us").to_lowercase();
    if !sectors::is_valid_exchange(&exchange) {
        return Err(AppError::Unprocessable(format!(
            "Unknown exchange '{}'. Supported: {}",
            exchange, sectors::SUPPORTED_EXCHANGES
        )));
    }

    let cache_key = crate::cache::cache_key(&exchange, &sector);
    {
        let cache = state.discovery_cache.read().await;
        if let Some((cached_at, response)) = cache.get(&cache_key) {
            if cached_at.elapsed() < CACHE_TTL {
                return Ok(Json(response.clone()));
            }
        }
    }

    let fmp_exchange = sectors::exchange_fmp_code(&exchange);
    let response = run_discovery(&state.fmp, &sector, &exchange, fmp_sector, fmp_exchange).await?;
    crate::cache::persist_discovery(&state.db, &exchange, &sector, &response).await;
    state.discovery_cache.write().await.insert(cache_key, (Instant::now(), response.clone()));
    Ok(Json(response))
}

/// Core discovery logic — separated from the HTTP handler so the background refresh
/// task can call it directly without going through HTTP.
///
/// `fmp_exchange`: `None` for US (uses `country=US`), `Some("LSE")` etc. for other exchanges.
pub async fn run_discovery(
    fmp: &Arc<FmpClient>,
    sector: &str,
    exchange: &str,
    fmp_sector: &str,
    fmp_exchange: Option<&str>,
) -> Result<DiscoveryResponse, AppError> {
    let fmp_sector = fmp_sector.to_owned();
    let fmp_exchange = fmp_exchange.map(str::to_owned);
    let bucket_width = (SMALL_MID_CAP_CEILING - SMALL_MID_CAP_FLOOR) / MARKET_CAP_BUCKETS;
    let bucket_sem = Arc::new(tokio::sync::Semaphore::new(3));
    let mut bucket_set = JoinSet::new();
    for i in 0..MARKET_CAP_BUCKETS {
        let floor = SMALL_MID_CAP_FLOOR + i * bucket_width;
        let ceiling = if i + 1 == MARKET_CAP_BUCKETS {
            SMALL_MID_CAP_CEILING
        } else {
            floor + bucket_width
        };
        let fmp = fmp.clone();
        let fmp_sector = fmp_sector.clone();
        let fmp_exchange = fmp_exchange.clone();
        let bucket_sem = bucket_sem.clone();
        bucket_set.spawn(async move {
            let _permit = bucket_sem.acquire().await.unwrap();
            (i, fmp.company_screener(floor, Some(ceiling), Some(&fmp_sector), PER_BUCKET_LIMIT, fmp_exchange.as_deref()).await)
        });
    }

    let mut by_symbol: HashMap<String, ScreenerCandidate> = HashMap::new();
    while let Some(outcome) = bucket_set.join_next().await {
        match outcome {
            Ok((_i, Ok(bucket_candidates))) => {
                for c in bucket_candidates {
                    by_symbol.insert(c.symbol.clone(), c);
                }
            }
            Ok((i, Err(e))) => tracing::warn!("discovery: market-cap bucket {i} fetch failed: {e}"),
            Err(e) => tracing::warn!("discovery: bucket task panicked: {e}"),
        }
    }
    let candidates: Vec<ScreenerCandidate> = by_symbol.into_values().collect();
    let candidates_screened = candidates.len();

    let sem = Arc::new(tokio::sync::Semaphore::new(5));
    let mut set = JoinSet::new();

    for candidate in candidates {
        let fmp = fmp.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            score_candidate(&fmp, &candidate).await
        });
    }

    let mut results: Vec<DiscoveryEntry> = Vec::new();
    while let Some(outcome) = set.join_next().await {
        if let Ok(Some(entry)) = outcome {
            results.push(entry);
        }
    }

    results.sort_by(|a, b| {
        a.deviation_from_graham_number_pct
            .abs()
            .partial_cmp(&b.deviation_from_graham_number_pct.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(DiscoveryResponse {
        sector: sector.to_owned(),
        exchange: exchange.to_owned(),
        currency: sectors::exchange_currency(exchange).to_owned(),
        market_cap_floor: SMALL_MID_CAP_FLOOR as f64,
        market_cap_ceiling: SMALL_MID_CAP_CEILING as f64,
        deviation_band_pct: calculations::DISCOVERY_DEVIATION_BAND_PCT,
        candidates_screened,
        results,
        disclaimer: DISCLAIMER.to_owned(),
    })
}

/// Fetches fundamentals for one candidate and applies the near-miss + quality-floor
/// filter. Returns `None` if required data is unavailable or the candidate doesn't
/// qualify — both are expected, not errors, since this runs across an unscreened
/// small/mid-cap universe where data completeness varies more than it does for
/// large caps.
async fn score_candidate(
    fmp: &FmpClient,
    candidate: &ScreenerCandidate,
) -> Option<DiscoveryEntry> {
    let ticker = candidate.symbol.as_str();

    let current_price = candidate.price?;
    if current_price <= 0.0 {
        return None;
    }

    let (income_r, balance_r, cashflow_r, ratios_r, km_r) = tokio::join!(
        fmp.income_statements(ticker, 5),
        fmp.balance_sheets(ticker, 2),
        fmp.cash_flow_statements(ticker, 2),
        fmp.ratios(ticker, 5),
        fmp.key_metrics(ticker, 5),
    );

    let income = match income_r {
        Ok(d) if !d.is_empty() => d,
        _ => { tracing::warn!("{ticker}: income statements unavailable for discovery"); return None; }
    };
    let balance = match balance_r {
        Ok(d) if !d.is_empty() => d,
        _ => { tracing::warn!("{ticker}: balance sheets unavailable for discovery"); return None; }
    };
    let cashflow = match cashflow_r {
        Ok(d) if !d.is_empty() => d,
        _ => { tracing::warn!("{ticker}: cash flows unavailable for discovery"); return None; }
    };
    let ratios = ratios_r.unwrap_or_default();
    let km = km_r.unwrap_or_default();

    // Build FundamentalsYear slice (oldest → newest) for the Graham Number calc, same alignment
    // approach as screener.rs::score_ticker.
    let ratio_by_date: HashMap<&str, &crate::fmp::Ratio> =
        ratios.iter().map(|r| (r.date.as_str(), r)).collect();
    let km_by_date: HashMap<&str, &crate::fmp::KeyMetrics> =
        km.iter().map(|k| (k.date.as_str(), k)).collect();

    let mut years: Vec<FundamentalsYear> = income
        .iter()
        .map(|inc| {
            let ratio = ratio_by_date.get(inc.date.as_str());
            let k = km_by_date.get(inc.date.as_str());
            FundamentalsYear {
                fiscal_year: inc.date.get(..4).unwrap_or(&inc.date).to_owned(),
                revenue: inc.revenue,
                eps: inc.eps,
                book_value_per_share: ratio.and_then(|r| r.book_value_per_share),
                free_cash_flow_per_share: ratio.and_then(|r| r.free_cash_flow_per_share),
                roic: k.and_then(|k| k.return_on_invested_capital),
            }
        })
        .collect();
    years.reverse(); // oldest → newest

    // Graham Number only needs the latest year's EPS and book value per share — no
    // growth-rate compounding, which is exactly why it was chosen over the DCF
    // intrinsic value (see calculations.rs::value_deviation_pct doc comment).
    let eps = years.last().and_then(|y| y.eps).filter(|&e| e > 0.0)?;
    let bvps = years.last().and_then(|y| y.book_value_per_share).filter(|&b| b > 0.0)?;

    let graham = calculations::graham_number(ticker, eps, bvps).ok()?;
    let deviation_pct = calculations::value_deviation_pct(current_price, graham.graham_number)?;

    if !calculations::is_near_miss(deviation_pct) {
        return None;
    }

    let quality = calculations::quality_score(ticker, &income, &ratios, &km).quality_score;
    let debt_safety = calculations::debt_safety_score(&ratios);
    let piotroski = calculations::piotroski_f_score(ticker, &income, &balance, &cashflow).score;

    // Track which raw fields the quality floor actually depends on are still missing
    // after providers.rs's EDGAR→FMP backfill — both sources, not just one, came up
    // empty. quality_score/debt_safety_score treat a missing field the same as a bad
    // one (e.g. missing debt_to_equity_ratio scores identically to maximum leverage),
    // which is misleading: absence of data isn't evidence of a problem. So a candidate
    // is only hard-excluded when it fails the floor AND the data behind that failure
    // was actually present — if data is missing, it's included anyway with a flag
    // rather than silently dropped as if it had proven itself unfit.
    let mut missing_data_fields = Vec::new();
    if ratios.first().and_then(|r| r.debt_to_equity_ratio).is_none() {
        missing_data_fields.push("debt_to_equity".to_owned());
    }
    if income.first().and_then(|i| i.gross_profit).is_none() {
        missing_data_fields.push("gross_margin".to_owned());
    }
    if km.first().and_then(|k| k.return_on_equity).is_none() {
        missing_data_fields.push("return_on_equity".to_owned());
    }
    let has_complete_data = missing_data_fields.is_empty();

    if !calculations::clears_discovery_quality_floor(quality, debt_safety, piotroski) && has_complete_data {
        return None;
    }

    Some(DiscoveryEntry {
        ticker: ticker.to_owned(),
        company_name: candidate.company_name.clone(),
        sector: candidate.sector.clone(),
        market_cap: candidate.market_cap,
        current_price,
        graham_number: graham.graham_number,
        deviation_from_graham_number_pct: deviation_pct,
        quality_score: quality,
        debt_safety_score: debt_safety,
        piotroski_score: piotroski,
        missing_data_fields,
    })
}
