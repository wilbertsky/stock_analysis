use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use tokio::task::JoinSet;

use crate::{
    auth::middleware::AuthUser,
    calculations,
    error::AppError,
    models::{FundamentalsYear, ScreenerEntry, SectorScreenerResponse},
    providers::Providers,
    sectors,
    state::AppState,
};

const CACHE_TTL: Duration = Duration::from_secs(12 * 3600);

/// Shared educational disclaimer — also used by `routes::discovery`.
pub(crate) const DISCLAIMER: &str = "Scores are calculated from publicly available financial data for \
    educational purposes only. They do not constitute investment advice, a recommendation to \
    buy or sell any security, or a guarantee of future performance. Always conduct your own \
    research and consult a licensed financial advisor before making investment decisions.";

/// Maximum number of tickers to score per sector after market-cap pre-filtering.
const SCORE_TOP_N: usize = 20;

// ── Sector model definitions ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
enum SectorModel {
    /// Piotroski 30% + Quality 25% + DCF Value 25% + Momentum 20%
    Standard,
    /// ROE Quality 35% + P/B Value 25% + Momentum 25% + Debt Safety 15%
    Financials,
    /// Dividend Quality 35% + P/B Value 25% + Momentum 25% + Debt Safety 15%
    RealEstate,
    /// Piotroski 25% + FCF Yield 30% + Momentum 30% + Quality 15%
    Energy,
    /// Dividend Quality 30% + Quality 25% + DCF Value 25% + Momentum 20%
    Dividend,
}

impl SectorModel {
    fn name(&self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Financials => "Financials",
            Self::RealEstate => "Real Estate",
            Self::Energy => "Energy",
            Self::Dividend => "Dividend",
        }
    }

    fn labels(&self) -> Vec<String> {
        let labels: &[&str] = match self {
            Self::Standard => &["Piotroski F-Score", "Business Quality", "DCF Value", "Momentum vs SPY"],
            Self::Financials => &["Return on Equity", "Price-to-Book Value", "Momentum vs SPY", "Debt Safety"],
            Self::RealEstate => &["Dividend Quality", "Price-to-Book Value", "Momentum vs SPY", "Debt Safety"],
            Self::Energy => &["Piotroski F-Score", "FCF Yield", "Momentum vs SPY", "Business Quality"],
            Self::Dividend => &["Dividend Quality", "Business Quality", "DCF Value", "Momentum vs SPY"],
        };
        labels.iter().map(|s| s.to_string()).collect()
    }

    fn weights(&self) -> Vec<String> {
        let weights: &[&str] = match self {
            Self::Standard  => &["30%", "25%", "25%", "20%"],
            Self::Financials => &["35%", "25%", "25%", "15%"],
            Self::RealEstate => &["35%", "25%", "25%", "15%"],
            Self::Energy     => &["25%", "30%", "30%", "15%"],
            Self::Dividend   => &["30%", "25%", "25%", "20%"],
        };
        weights.iter().map(|s| s.to_string()).collect()
    }
}

fn sector_model(sector: &str) -> SectorModel {
    match sector {
        "financials" => SectorModel::Financials,
        "real-estate" => SectorModel::RealEstate,
        "energy" => SectorModel::Energy,
        "consumer-staples" | "utilities" => SectorModel::Dividend,
        _ => SectorModel::Standard,
    }
}

// ── Route handler ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ScreenerQuery {
    /// Exchange slug to screen. Defaults to "us" (NYSE/NASDAQ). Supported: us, lse.
    pub exchange: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/screener/{sector}",
    tag = "screener",
    params(
        ("sector" = String, Path, description = "Sector name, e.g. technology, healthcare, financials"),
        ("exchange" = Option<String>, Query, description = "Exchange slug: 'us' (default) or 'lse'"),
    ),
    description = "Screens large-cap stocks (market cap ≥ $10B, sourced from FMP's company \
        screener) within a sector and ranks them by a weighted composite score. \
        The scoring model adapts to the sector: \
        Standard sectors (technology, healthcare, industrials, etc.) use Piotroski F-Score (30%), \
        Business Quality (25%), DCF Value Signal (25%), and Momentum vs SPY (20%). \
        Financials use Return on Equity (35%), Price-to-Book (25%), Momentum (25%), Debt Safety (15%). \
        Real Estate uses Dividend Quality (35%), Price-to-Book (25%), Momentum (25%), Debt Safety (15%). \
        Energy uses Piotroski (25%), FCF Yield (30%), Momentum (30%), Quality (15%). \
        Consumer Staples and Utilities use Dividend Quality (30%), Business Quality (25%), DCF Value (25%), Momentum (20%). \
        Supported sectors: technology, healthcare, financials, energy, consumer-staples, \
        consumer-discretionary, industrials, materials, real-estate, communication, utilities. \
        Stocks are pre-filtered to the top market-cap names, then scored. \
        Expect this endpoint to take 15–30 seconds. A disclaimer field is included in every response.",
    responses(
        (status = 200, description = "Ranked stock picks for the sector", body = SectorScreenerResponse),
        (status = 422, description = "Unknown sector name", body = crate::error::ErrorBody),
        (status = 502, description = "Data provider error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_sector_top_picks(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(sector): Path<String>,
    Query(params): Query<ScreenerQuery>,
) -> Result<Json<SectorScreenerResponse>, AppError> {
    let exchange = params.exchange.as_deref().unwrap_or("us").to_lowercase();
    if !sectors::is_valid_exchange(&exchange) {
        return Err(AppError::Unprocessable(format!(
            "Unknown exchange '{}'. Supported: {}",
            exchange, sectors::SUPPORTED_EXCHANGES
        )));
    }

    let cache_key = crate::cache::cache_key(&exchange, &sector);
    {
        let cache = state.screener_cache.read().await;
        if let Some((cached_at, response)) = cache.get(&cache_key) {
            if cached_at.elapsed() < CACHE_TTL {
                return Ok(Json(response.clone()));
            }
        }
    }

    let response = run_screener(&state, &sector, &exchange).await?;
    crate::cache::persist_screener(&state.db, &exchange, &sector, &response).await;
    state.screener_cache.write().await.insert(cache_key, (Instant::now(), response.clone()));
    Ok(Json(response))
}

/// Large-cap floor used when fetching LSE candidates from FMP.
/// FMP normalises market caps to USD, so the same $10B threshold applies.
const LSE_LARGE_CAP_FLOOR: u64 = 5_000_000_000;

/// Core screener logic — separated from the HTTP handler so the background refresh
/// task can call it directly without going through HTTP.
pub async fn run_screener(state: &AppState, sector: &str, exchange: &str) -> Result<SectorScreenerResponse, AppError> {
    let candidate_tickers: Vec<String> = if exchange == "us" {
        // US: use the pre-loaded Sp500 large-cap universe (falls back to hardcoded list
        // if FMP rate-limited a sector during boot).
        if state.sp500.is_loaded() {
            state
                .sp500
                .tickers_for_sector(sector)
                .map(|s| s.to_vec())
                .or_else(|| {
                    sectors::tickers_for_sector(sector)
                        .map(|s| s.iter().map(|t| t.to_string()).collect())
                })
                .ok_or_else(|| {
                    AppError::Unprocessable(format!(
                        "Unknown sector '{}'. Supported: {}",
                        sector,
                        sectors::SUPPORTED_SECTORS
                    ))
                })?
        } else {
            sectors::tickers_for_sector(sector)
                .ok_or_else(|| {
                    AppError::Unprocessable(format!(
                        "Unknown sector '{}'. Supported: {}",
                        sector,
                        sectors::SUPPORTED_SECTORS
                    ))
                })?
                .iter()
                .map(|s| s.to_string())
                .collect()
        }
    } else {
        // Non-US exchange: fetch large-cap candidates from FMP filtered by exchange and sector.
        let fmp_sector = sectors::slug_to_fmp_sector(sector).ok_or_else(|| {
            AppError::Unprocessable(format!(
                "Unknown sector '{}'. Supported: {}",
                sector,
                sectors::SUPPORTED_SECTORS
            ))
        })?;
        let fmp_exchange = sectors::exchange_fmp_code(exchange);
        let candidates = state
            .fmp
            .company_screener(LSE_LARGE_CAP_FLOOR, None, Some(fmp_sector), 100, fmp_exchange)
            .await
            .unwrap_or_default();
        if candidates.is_empty() {
            tracing::warn!(sector, exchange, "No large-cap candidates from FMP for exchange");
        }
        candidates.into_iter().map(|c| c.symbol).collect()
    };

    let model = sector_model(sector);
    let providers = Arc::new(state.providers.with_fmp(state.fmp.clone()));
    let tickers_to_score = top_by_market_cap(&providers, &candidate_tickers).await;
    let spy_prices = providers.historical_prices("SPY", 260).await.unwrap_or_default();
    let spy_prices = Arc::new(spy_prices);

    let sem = Arc::new(tokio::sync::Semaphore::new(5));
    let mut set = JoinSet::new();

    for ticker in tickers_to_score {
        let providers = providers.clone();
        let spy_prices = spy_prices.clone();
        let sem = sem.clone();

        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            score_ticker(&providers, &ticker, &spy_prices, model).await
        });
    }

    let mut results: Vec<ScreenerEntry> = Vec::new();
    while let Some(outcome) = set.join_next().await {
        if let Ok(Some(entry)) = outcome {
            results.push(entry);
        }
    }

    results.sort_by(|a, b| {
        b.composite_score
            .partial_cmp(&a.composite_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let stocks_analyzed = results.len();
    Ok(SectorScreenerResponse {
        sector: sector.to_owned(),
        exchange: exchange.to_owned(),
        currency: sectors::exchange_currency(exchange).to_owned(),
        scoring_model: model.name().to_owned(),
        score_labels: model.labels(),
        score_weights: model.weights(),
        stocks_analyzed,
        results,
        disclaimer: DISCLAIMER.to_owned(),
    })
}

/// Returns up to `SCORE_TOP_N` tickers sorted by market cap descending.
async fn top_by_market_cap(providers: &Providers, tickers: &[String]) -> Vec<String> {
    let caps = providers.batch_market_caps(tickers).await;

    if caps.is_empty() {
        return tickers.iter().take(SCORE_TOP_N).cloned().collect();
    }

    let mut with_caps: Vec<(&String, u64)> = tickers
        .iter()
        .map(|t| (t, caps.get(t).copied().unwrap_or(0)))
        .collect();

    with_caps.sort_by(|a, b| b.1.cmp(&a.1));
    with_caps
        .into_iter()
        .take(SCORE_TOP_N)
        .map(|(t, _)| t.clone())
        .collect()
}

/// Fetch and score a single ticker using the given sector model. Returns None if required data
/// is unavailable.
async fn score_ticker(
    providers: &Providers,
    ticker: &str,
    spy_prices: &[crate::fmp::HistoricalPrice],
    model: SectorModel,
) -> Option<ScreenerEntry> {
    let (income_r, balance_r, cashflow_r, ratios_r, km_r, prices_r) = tokio::join!(
        providers.income_statements(ticker, 5),
        providers.balance_sheets(ticker, 2),
        providers.cash_flow_statements(ticker, 2),
        providers.ratios(ticker, 5),
        providers.key_metrics(ticker, 5),
        providers.historical_prices(ticker, 260),
    );

    let income = match income_r {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => { tracing::warn!("{ticker}: income statements empty"); return None; }
        Err(e) => { tracing::warn!("{ticker}: income statements error: {e}"); return None; }
    };
    let balance = match balance_r {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => { tracing::warn!("{ticker}: balance sheets empty"); return None; }
        Err(e) => { tracing::warn!("{ticker}: balance sheets error: {e}"); return None; }
    };
    let cashflow = match cashflow_r {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => { tracing::warn!("{ticker}: cash flows empty"); return None; }
        Err(e) => { tracing::warn!("{ticker}: cash flows error: {e}"); return None; }
    };
    let ratios = ratios_r.unwrap_or_default();
    let km = km_r.unwrap_or_default();
    let prices = match prices_r {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => { tracing::warn!("{ticker}: prices empty"); return None; }
        Err(e) => { tracing::warn!("{ticker}: prices error: {e}"); return None; }
    };

    let current_price = match prices.first().and_then(|p| p.price) {
        Some(p) => p,
        None => { tracing::warn!("{ticker}: first price entry has no price"); return None; }
    };

    // Build FundamentalsYear slice (oldest → newest) for DCF value signal.
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

    // Compute all individual signals we might need across models.
    let piotroski_0_100 = {
        let p = calculations::piotroski_f_score(ticker, &income, &balance, &cashflow);
        p.score as f64 / 9.0 * 100.0
    };
    let quality = calculations::quality_score(ticker, &income, &ratios, &km).quality_score;
    let momentum = calculations::momentum_score(ticker, &prices, spy_prices).momentum_score;
    let dcf_value = calculations::value_signal(ticker, &years, current_price);
    let roe_quality = calculations::roe_quality_score(&km);
    let pb_value = calculations::pb_value_score(&ratios, current_price);
    let debt_safety = calculations::debt_safety_score(&ratios);
    let dividend_quality = calculations::dividend_quality_score(&ratios);
    let fcf_yield = calculations::fcf_yield_score(&ratios, current_price);

    // Assign score slots and compute weighted composite per model.
    let (score_a, score_b, score_c, score_d, composite_score) = match model {
        SectorModel::Standard => {
            let a = piotroski_0_100;
            let b = quality;
            let c = dcf_value;
            let d = momentum;
            let comp = a * 0.30 + b * 0.25 + c * 0.25 + d * 0.20;
            (a, b, c, d, comp)
        }
        SectorModel::Financials => {
            let a = roe_quality;
            let b = pb_value;
            let c = momentum;
            let d = debt_safety;
            let comp = a * 0.35 + b * 0.25 + c * 0.25 + d * 0.15;
            (a, b, c, d, comp)
        }
        SectorModel::RealEstate => {
            let a = dividend_quality;
            let b = pb_value;
            let c = momentum;
            let d = debt_safety;
            let comp = a * 0.35 + b * 0.25 + c * 0.25 + d * 0.15;
            (a, b, c, d, comp)
        }
        SectorModel::Energy => {
            let a = piotroski_0_100;
            let b = fcf_yield;
            let c = momentum;
            let d = quality;
            let comp = a * 0.25 + b * 0.30 + c * 0.30 + d * 0.15;
            (a, b, c, d, comp)
        }
        SectorModel::Dividend => {
            let a = dividend_quality;
            let b = quality;
            let c = dcf_value;
            let d = momentum;
            let comp = a * 0.30 + b * 0.25 + c * 0.25 + d * 0.20;
            (a, b, c, d, comp)
        }
    };

    let score_tier = if composite_score >= 70.0 {
        "High".to_owned()
    } else if composite_score >= 55.0 {
        "Above Average".to_owned()
    } else if composite_score >= 40.0 {
        "Average".to_owned()
    } else {
        "Below Average".to_owned()
    };

    Some(ScreenerEntry {
        ticker: ticker.to_owned(),
        score_a,
        score_b,
        score_c,
        score_d,
        composite_score,
        score_tier,
    })
}
