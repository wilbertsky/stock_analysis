//! Small/mid-cap "near miss" discovery screener.
//!
//! The sector screener (`screener.rs`) only ever sees large-cap names (market cap ≥
//! $10B) — anything smaller never enters its candidate pool, regardless of how it would
//! score. This is a separate, additive screener: it sources a small/mid-cap universe
//! directly from FMP's company-screener, computes the same DCF intrinsic value and
//! quality signals already used elsewhere in this app, and surfaces candidates that are
//! close to fair value (not necessarily deeply undervalued) with fundamentals that clear
//! a quality *floor* rather than requiring a *ceiling*. See `calculations.rs`'s
//! `DISCOVERY_*` constants for the reasoning behind each threshold.
//!
//! Deliberately does not touch `screener.rs` or its composite scoring model.

use std::{collections::HashMap, sync::Arc};

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
    fmp::ScreenerCandidate,
    models::{DiscoveryEntry, DiscoveryResponse, FundamentalsYear},
    providers::Providers,
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

/// Candidates fetched from FMP per request. Each candidate triggers several downstream
/// fundamentals calls, so this is deliberately smaller than the sector screener's
/// per-sector fetch — keeps response time in a similar 15–30s ballpark.
const FETCH_LIMIT: u32 = 40;

#[derive(Debug, Deserialize)]
pub struct DiscoveryQuery {
    /// Optional sector slug (e.g. "technology"). Omit to screen across all sectors.
    pub sector: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/discovery",
    tag = "discovery",
    params(("sector" = Option<String>, Query, description = "Optional sector slug, e.g. technology. Omit to screen all sectors.")),
    description = "Screens small/mid-cap stocks (market cap $300M–$5B, sourced from FMP's \
        company screener) for candidates close to their DCF intrinsic value with fundamentals \
        that clear a quality floor — not the same as the sector screener's blue-chip-biased \
        composite ranking. \
        These are names too small to ever appear in the sector screener, which only evaluates \
        large-cap stocks (market cap ≥ $10B). A candidate qualifies when: (1) its current price \
        is within ±20% of its DCF intrinsic value estimate — either slightly undervalued or \
        slightly above, both are surfaced since the DCF formula tends to be conservative for \
        asset-light, high-growth names; and (2) it clears a quality floor — quality score ≥ 40, \
        debt safety score ≥ 40, Piotroski F-Score ≥ 4 — chosen to exclude the distress zone, \
        not to require best-in-class fundamentals. Results are sorted by closeness to intrinsic \
        value. Expect 15–30 seconds response time. A disclaimer field is included in every response.",
    responses(
        (status = 200, description = "Near-miss small/mid-cap candidates", body = DiscoveryResponse),
        (status = 422, description = "Unknown sector name", body = crate::error::ErrorBody),
        (status = 502, description = "Data provider error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_discovery(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<DiscoveryQuery>,
) -> Result<Json<DiscoveryResponse>, AppError> {
    let fmp_sector = match &params.sector {
        Some(s) => Some(sectors::slug_to_fmp_sector(s).ok_or_else(|| {
            AppError::Unprocessable(format!(
                "Unknown sector '{}'. Supported: {}",
                s,
                sectors::SUPPORTED_SECTORS
            ))
        })?),
        None => None,
    };

    let candidates = state
        .fmp
        .company_screener(SMALL_MID_CAP_FLOOR, Some(SMALL_MID_CAP_CEILING), fmp_sector, FETCH_LIMIT)
        .await?;
    let candidates_screened = candidates.len();

    // Same provider chain (EDGAR/Yahoo primary, server-level FMP fallback) used
    // everywhere else in this app for per-ticker fundamentals.
    let providers = Arc::new(state.providers.with_fmp(state.fmp.clone()));

    let sem = Arc::new(tokio::sync::Semaphore::new(5));
    let mut set = JoinSet::new();

    for candidate in candidates {
        let providers = providers.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            score_candidate(&providers, &candidate).await
        });
    }

    let mut results: Vec<DiscoveryEntry> = Vec::new();
    while let Some(outcome) = set.join_next().await {
        if let Ok(Some(entry)) = outcome {
            results.push(entry);
        }
    }

    results.sort_by(|a, b| {
        a.deviation_from_intrinsic_value_pct
            .abs()
            .partial_cmp(&b.deviation_from_intrinsic_value_pct.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(DiscoveryResponse {
        sector: params.sector,
        market_cap_floor: SMALL_MID_CAP_FLOOR as f64,
        market_cap_ceiling: SMALL_MID_CAP_CEILING as f64,
        deviation_band_pct: calculations::DISCOVERY_DEVIATION_BAND_PCT,
        candidates_screened,
        results,
        disclaimer: DISCLAIMER.to_owned(),
    }))
}

/// Fetches fundamentals for one candidate and applies the near-miss + quality-floor
/// filter. Returns `None` if required data is unavailable or the candidate doesn't
/// qualify — both are expected, not errors, since this runs across an unscreened
/// small/mid-cap universe where data completeness varies more than it does for
/// large caps.
async fn score_candidate(
    providers: &Providers,
    candidate: &ScreenerCandidate,
) -> Option<DiscoveryEntry> {
    let ticker = candidate.symbol.as_str();

    let current_price = candidate.price?;
    if current_price <= 0.0 {
        return None;
    }

    let (income_r, balance_r, cashflow_r, ratios_r, km_r) = tokio::join!(
        providers.income_statements(ticker, 5),
        providers.balance_sheets(ticker, 2),
        providers.cash_flow_statements(ticker, 2),
        providers.ratios(ticker, 5),
        providers.key_metrics(ticker, 5),
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

    // Build FundamentalsYear slice (oldest → newest) for the DCF calc, same alignment
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

    let growth = calculations::build_growth_rates(ticker, &years);
    let growth_rate = growth.eps.cagr_5yr.or(growth.eps.cagr_1yr)?;
    let eps = years.last().and_then(|y| y.eps).filter(|&e| e > 0.0)?;

    let dcf = calculations::growth_dcf_valuation(ticker, eps, growth_rate, 0.15).ok()?;
    let deviation_pct = calculations::dcf_deviation_pct(current_price, dcf.estimated_intrinsic_value)?;

    if !calculations::is_near_miss(deviation_pct) {
        return None;
    }

    let quality = calculations::quality_score(ticker, &income, &ratios, &km).quality_score;
    let debt_safety = calculations::debt_safety_score(&ratios);
    let piotroski = calculations::piotroski_f_score(ticker, &income, &balance, &cashflow).score;

    if !calculations::clears_discovery_quality_floor(quality, debt_safety, piotroski) {
        return None;
    }

    Some(DiscoveryEntry {
        ticker: ticker.to_owned(),
        company_name: candidate.company_name.clone(),
        sector: candidate.sector.clone(),
        market_cap: candidate.market_cap,
        current_price,
        estimated_intrinsic_value: dcf.estimated_intrinsic_value,
        deviation_from_intrinsic_value_pct: deviation_pct,
        quality_score: quality,
        debt_safety_score: debt_safety,
        piotroski_score: piotroski,
    })
}
