use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, State},
    Json,
};
use tokio::task::JoinSet;

use crate::{
    calculations,
    error::AppError,
    models::{FundamentalsYear, ScreenerEntry, SectorScreenerResponse},
    sectors,
    state::AppState,
};

#[utoipa::path(
    get,
    path = "/api/screener/{sector}",
    tag = "screener",
    params(("sector" = String, Path, description = "Sector name, e.g. technology, healthcare, financials")),
    description = "Screens a curated list of large-cap stocks within a sector and ranks them by a \
        weighted composite score that combines four factor investing signals: \
        Piotroski F-Score (30%) — accounting-based quality; \
        Business Quality (25%) — gross margin, ROE, and debt levels; \
        Rule #1 Value Signal (25%) — how the current price compares to the calculated sticker price \
        and margin of safety price; \
        Momentum (20%) — relative price performance vs. the S&P 500 over 3/6/12 months. \
        Supported sectors: technology, healthcare, financials, energy, consumer-staples, \
        consumer-discretionary, industrials, materials, real-estate, communication, utilities. \
        Each sector screens 10 representative large-cap stocks. Stocks for which data is \
        unavailable are omitted from results. Expect this endpoint to take 10–20 seconds \
        as it fetches data for multiple tickers concurrently.",
    responses(
        (status = 200, description = "Ranked stock picks for the sector", body = SectorScreenerResponse),
        (status = 422, description = "Unknown sector name", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_sector_top_picks(
    State(state): State<AppState>,
    Path(sector): Path<String>,
) -> Result<Json<SectorScreenerResponse>, AppError> {
    let tickers = sectors::tickers_for_sector(&sector).ok_or_else(|| {
        AppError::Unprocessable(format!(
            "Unknown sector '{}'. Supported: {}",
            sector,
            sectors::SUPPORTED_SECTORS
        ))
    })?;

    // Fetch SPY prices once — shared benchmark for all momentum calculations.
    let spy_prices = state.fmp.historical_prices("SPY", 260).await.unwrap_or_default();
    let spy_prices = Arc::new(spy_prices);

    // Score each ticker concurrently, limiting to 3 in-flight at a time.
    let sem = Arc::new(tokio::sync::Semaphore::new(3));
    let mut set = JoinSet::new();

    for &ticker in tickers {
        let state = state.clone();
        let spy_prices = spy_prices.clone();
        let sem = sem.clone();

        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            score_ticker(&state, ticker, &spy_prices).await
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
    Ok(Json(SectorScreenerResponse {
        sector,
        stocks_analyzed,
        results,
    }))
}

/// Fetch and score a single ticker. Returns None if any required data is unavailable.
async fn score_ticker(
    state: &AppState,
    ticker: &str,
    spy_prices: &[crate::fmp::HistoricalPrice],
) -> Option<ScreenerEntry> {
    let (income_r, balance_r, cashflow_r, ratios_r, km_r, prices_r) = tokio::join!(
        state.fmp.income_statements(ticker, 5),
        state.fmp.balance_sheets(ticker, 2),
        state.fmp.cash_flow_statements(ticker, 2),
        state.fmp.ratios(ticker, 5),
        state.fmp.key_metrics(ticker, 5),
        state.fmp.historical_prices(ticker, 260),
    );

    let income = income_r.ok()?;
    let balance = balance_r.ok()?;
    let cashflow = cashflow_r.ok()?;
    let ratios = ratios_r.unwrap_or_default();
    let km = km_r.unwrap_or_default();
    let prices = prices_r.ok()?;

    // Build aligned FundamentalsYear for value signal (oldest → newest)
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

    let current_price = prices.first()?.price?;

    let piotroski = calculations::piotroski_f_score(ticker, &income, &balance, &cashflow);
    let quality = calculations::quality_score(ticker, &income, &ratios, &km);
    let momentum = calculations::momentum_score(ticker, &prices, spy_prices);
    let val_signal = calculations::value_signal(ticker, &years, current_price);

    let piotroski_score = piotroski.score;
    let quality_score = quality.quality_score;
    let momentum_score = momentum.momentum_score;

    // Weighted composite: piotroski 30% + quality 25% + value 25% + momentum 20%
    let composite_score = (piotroski_score as f64 / 9.0) * 100.0 * 0.30
        + quality_score * 0.25
        + val_signal * 0.25
        + momentum_score * 0.20;

    let signal = if composite_score >= 70.0 {
        "Strong Buy".to_owned()
    } else if composite_score >= 55.0 {
        "Buy".to_owned()
    } else if composite_score >= 40.0 {
        "Hold".to_owned()
    } else {
        "Avoid".to_owned()
    };

    Some(ScreenerEntry {
        ticker: ticker.to_owned(),
        piotroski_score,
        quality_score,
        momentum_score,
        value_signal: val_signal,
        composite_score,
        signal,
    })
}
