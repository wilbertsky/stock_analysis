use axum::{
    extract::{Path, State},
    Json,
};
use std::collections::HashMap;

use crate::{
    calculations,
    error::AppError,
    models::*,
    state::AppState,
};

// ── Shared data fetcher ──────────────────────────────────────────────────────

/// Fetch and align fundamentals from FMP.
/// Returns years sorted oldest → newest, plus the latest P/E ratio.
///
/// Data sources:
/// - income-statement: revenue, EPS
/// - ratios:           book value/share, FCF/share, P/E ratio
/// - key-metrics:      ROIC
pub async fn fetch_fundamentals(
    state: &AppState,
    ticker: &str,
) -> Result<(Vec<FundamentalsYear>, Option<f64>), AppError> {
    let limit = 5;

    let (income_list, ratio_list, km_list) = tokio::try_join!(
        state.fmp.income_statements(ticker, limit),
        state.fmp.ratios(ticker, limit),
        state.fmp.key_metrics(ticker, limit),
    )?;

    // Index supplementary data by date for alignment (all lists are newest-first)
    let ratio_by_date: HashMap<&str, &crate::fmp::Ratio> =
        ratio_list.iter().map(|r| (r.date.as_str(), r)).collect();
    let km_by_date: HashMap<&str, &crate::fmp::KeyMetrics> =
        km_list.iter().map(|km| (km.date.as_str(), km)).collect();

    let mut years: Vec<FundamentalsYear> = income_list
        .iter()
        .map(|inc| {
            let ratio = ratio_by_date.get(inc.date.as_str());
            let km = km_by_date.get(inc.date.as_str());
            FundamentalsYear {
                fiscal_year: inc.date.get(..4).unwrap_or(&inc.date).to_owned(),
                revenue: inc.revenue,
                eps: inc.eps,
                book_value_per_share: ratio.and_then(|r| r.book_value_per_share),
                free_cash_flow_per_share: ratio.and_then(|r| r.free_cash_flow_per_share),
                roic: km.and_then(|km| km.return_on_invested_capital),
            }
        })
        .collect();

    years.reverse(); // oldest → newest

    // Most recent P/E comes from the first entry (newest) in ratio_list
    let latest_pe = ratio_list.first().and_then(|r| r.price_to_earnings_ratio);

    Ok((years, latest_pe))
}

// ── Handlers ─────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/fundamentals",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Returns up to 5 years of the Rule #1 'Big Five' fundamental metrics for a stock: \
        Revenue, EPS (Earnings Per Share), Book Value Per Share, Free Cash Flow Per Share, and ROIC \
        (Return on Invested Capital). These are the core numbers Phil Town's Rule #1 investing \
        framework uses to assess whether a company consistently grows in value. Consistent upward \
        trends across all five metrics over time indicate a company with a durable competitive \
        advantage — what Rule #1 calls a 'moat'. Data is sorted oldest to newest.",
    responses(
        (status = 200, description = "Big Five fundamentals sorted oldest to newest", body = FundamentalsResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_fundamentals(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<FundamentalsResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, _) = fetch_fundamentals(&state, &ticker).await?;
    Ok(Json(FundamentalsResponse { ticker, years }))
}

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/growth-rates",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Calculates the Compound Annual Growth Rate (CAGR) for each of the Big Five \
        metrics over 1-year, 5-year, and 10-year windows. In Rule #1 investing, you want to see \
        all five metrics growing at 10% or more per year — ideally 15–20%+. \
        ROIC is the most important: consistently above 10% means the company is efficiently \
        turning invested capital into profit, which is the hallmark of a true moat business. \
        Windows with insufficient data history return null.",
    responses(
        (status = 200, description = "CAGR for each Big Five metric across available time windows", body = GrowthRatesResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_growth_rates(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<GrowthRatesResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, _) = fetch_fundamentals(&state, &ticker).await?;
    Ok(Json(calculations::build_growth_rates(&ticker, &years)))
}

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/rule-number-one",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Computes Phil Town's Rule #1 sticker price and margin of safety price. \
        The sticker price is the fair value of the stock today based on projected future growth. \
        Steps: (1) Estimate future EPS in 10 years using historical EPS CAGR. \
        (2) Multiply by a default P/E ratio of 2× the growth rate percentage (e.g. 15% growth → P/E of 30). \
        (3) Discount that future price back to today at a 15% minimum acceptable rate of return (MARR) — \
        the rate at which your money doubles roughly every 5 years. \
        (4) The margin of safety price is 50% of the sticker price, giving you a buffer against \
        uncertainty. Rule #1: never pay more than the margin of safety price. \
        The growth rate is capped at 25% to avoid overoptimistic projections.",
    responses(
        (status = 200, description = "Rule #1 sticker price and margin of safety", body = StickerPriceResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 422, description = "Cannot compute — EPS is zero/negative or insufficient history", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_rule_number_one(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<StickerPriceResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, _) = fetch_fundamentals(&state, &ticker).await?;
    let growth = calculations::build_growth_rates(&ticker, &years);

    let growth_rate = growth
        .eps
        .cagr_5yr
        .or(growth.eps.cagr_1yr)
        .ok_or(AppError::InsufficientData { needed: 2, have: years.len() })?;

    let current_eps = years
        .last()
        .and_then(|y| y.eps)
        .ok_or_else(|| AppError::Unprocessable("No EPS data available".into()))?;

    Ok(Json(calculations::rule1_sticker_price(&ticker, current_eps, growth_rate, 0.15)?))
}

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/graham-number",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Calculates Benjamin Graham's intrinsic value estimate: sqrt(22.5 × EPS × Book Value Per Share). \
        Graham was Warren Buffett's mentor and the father of value investing. \
        This formula assumes a fair P/E of 15 and a fair Price-to-Book of 1.5 (15 × 1.5 = 22.5). \
        If the current stock price is below the Graham Number, the stock may be undervalued \
        on a pure asset and earnings basis. It is a conservative, balance-sheet-focused estimate \
        and works best for stable, asset-heavy companies. It tends to undervalue high-growth \
        businesses with low book value (e.g. software companies).",
    responses(
        (status = 200, description = "Graham Number intrinsic value", body = GrahamNumberResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 422, description = "Cannot compute — EPS or book value is zero/negative", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_graham_number(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<GrahamNumberResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, _) = fetch_fundamentals(&state, &ticker).await?;
    let latest = years.last().ok_or(AppError::NotFound)?;

    let eps = latest
        .eps
        .ok_or_else(|| AppError::Unprocessable("No EPS data available".into()))?;
    let bvps = latest
        .book_value_per_share
        .ok_or_else(|| AppError::Unprocessable("No book value per share data available".into()))?;

    Ok(Json(calculations::graham_number(&ticker, eps, bvps)?))
}

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/peg",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Calculates the PEG ratio (Price/Earnings to Growth), popularized by Peter Lynch. \
        Formula: P/E ratio ÷ EPS growth rate (expressed as a percentage). \
        The PEG ratio adjusts the P/E for growth speed, making it easier to compare \
        companies growing at different rates. \
        Interpretation: PEG < 1.0 suggests the stock may be undervalued relative to its growth. \
        PEG = 1.0 is considered fairly valued. PEG > 1.0 suggests the market is pricing in \
        a premium above the growth rate — possibly overvalued, or the market expects acceleration. \
        Peter Lynch considered anything under 0.5 a potential bargain. \
        Uses the 5-year EPS CAGR where available, falling back to 1-year.",
    responses(
        (status = 200, description = "PEG ratio", body = PegRatioResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 422, description = "Cannot compute — growth rate is zero or negative", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_peg(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<PegRatioResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, latest_pe) = fetch_fundamentals(&state, &ticker).await?;

    let pe = latest_pe
        .ok_or_else(|| AppError::Unprocessable("P/E ratio unavailable for this ticker".into()))?;

    let growth = calculations::build_growth_rates(&ticker, &years);
    let growth_rate = growth
        .eps
        .cagr_5yr
        .or(growth.eps.cagr_1yr)
        .ok_or(AppError::InsufficientData { needed: 2, have: years.len() })?;

    Ok(Json(calculations::peg_ratio(&ticker, pe, growth_rate)?))
}

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/summary",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Returns all available valuations in a single response: Big Five fundamentals, \
        growth rates, Rule #1 sticker price, Graham Number, and PEG ratio. \
        Use this endpoint to get a complete picture of a stock's health and estimated fair value \
        without making multiple calls. \
        How to read the summary: start with growth_rates to confirm the company consistently grows \
        its Big Five metrics. Then compare the current market price against sticker_price \
        (Rule #1 fair value), graham_number (conservative asset-based value), and the peg ratio \
        (growth-adjusted valuation). A stock trading below both the margin of safety price and \
        the Graham Number, with a PEG under 1.0 and strong Big Five growth, is a strong candidate \
        for further due diligence.",
    responses(
        (status = 200, description = "Complete valuation summary", body = SummaryResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 422, description = "One or more valuations could not be computed", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_summary(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<SummaryResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (years, latest_pe) = fetch_fundamentals(&state, &ticker).await?;

    let growth = calculations::build_growth_rates(&ticker, &years);
    let latest = years.last().ok_or(AppError::NotFound)?;

    let growth_rate = growth
        .eps
        .cagr_5yr
        .or(growth.eps.cagr_1yr)
        .ok_or(AppError::InsufficientData { needed: 2, have: years.len() })?;

    let eps = latest
        .eps
        .ok_or_else(|| AppError::Unprocessable("No EPS data available".into()))?;
    let bvps = latest
        .book_value_per_share
        .ok_or_else(|| AppError::Unprocessable("No book value per share data available".into()))?;
    let pe = latest_pe
        .ok_or_else(|| AppError::Unprocessable("P/E ratio unavailable for this ticker".into()))?;

    let sticker = calculations::rule1_sticker_price(&ticker, eps, growth_rate, 0.15)?;
    let graham = calculations::graham_number(&ticker, eps, bvps)?;
    let peg = calculations::peg_ratio(&ticker, pe, growth_rate)?;

    let fundamentals = FundamentalsResponse { ticker: ticker.clone(), years };

    Ok(Json(SummaryResponse {
        ticker,
        fundamentals,
        growth_rates: growth,
        sticker_price: sticker,
        graham_number: graham,
        peg,
    }))
}
