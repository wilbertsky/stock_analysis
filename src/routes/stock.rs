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
    description = "Returns up to 5 years of core fundamental metrics for a stock: \
        Revenue, EPS (Earnings Per Share), Book Value Per Share, Free Cash Flow Per Share, and ROIC \
        (Return on Invested Capital). These five metrics are the foundation of growth-based value \
        investing — consistently rising numbers across all five over time indicate a company with \
        durable pricing power and a genuine competitive advantage. Data is sorted oldest to newest.",
    responses(
        (status = 200, description = "Core fundamental metrics sorted oldest to newest", body = FundamentalsResponse),
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
    description = "Calculates the Compound Annual Growth Rate (CAGR) for each of the five core \
        fundamental metrics over 1-year, 5-year, and 10-year windows. \
        Strong growth investors look for all five metrics growing at 10% or more per year — \
        ideally 15–20%+. ROIC is the most telling: consistently above 10% means the company is \
        efficiently converting invested capital into profit, a hallmark of durable competitive advantage. \
        Windows with insufficient data history return null.",
    responses(
        (status = 200, description = "CAGR for each fundamental metric across available time windows", body = GrowthRatesResponse),
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
    path = "/api/stock/{ticker}/intrinsic-value",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Estimates intrinsic value using a simplified Discounted Cash Flow (DCF) approach \
        based on earnings growth. This methodology traces back to Benjamin Graham and Warren Buffett \
        and is grounded in the principle that a stock is worth the present value of its future earnings. \
        Steps: (1) Estimate future EPS in 10 years using the historical EPS CAGR. \
        (2) Apply a growth-adjusted P/E of 2× the growth rate percentage \
        (e.g. 15% growth → P/E of 30) — a proxy for what the market typically pays for that growth speed. \
        (3) Discount that future price back to today at a 15% minimum required rate of return, \
        representing the return you demand to justify the investment risk. \
        (4) The margin of safety price is 50% of the intrinsic value estimate, providing a buffer \
        against uncertainty in the assumptions — a concept introduced by Benjamin Graham. \
        The growth rate is capped at 25% to avoid overoptimistic projections.",
    responses(
        (status = 200, description = "DCF intrinsic value estimate and margin of safety", body = IntrinsicValueResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 422, description = "Cannot compute — EPS is zero/negative or insufficient history", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_intrinsic_value(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<IntrinsicValueResponse>, AppError> {
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

    Ok(Json(calculations::growth_dcf_valuation(&ticker, current_eps, growth_rate, 0.15)?))
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
    description = "Returns a complete analysis in a single response: core fundamental metrics, \
        growth rates, DCF intrinsic value estimate, Graham Number, PEG ratio, and price momentum \
        relative to the S&P 500. \
        How to read the summary: start with growth_rates to confirm the company consistently grows \
        its five core metrics over time. Then compare the current market price against \
        intrinsic_value (DCF fair value estimate) and graham_number (conservative asset-based value), \
        and check the peg ratio for growth-adjusted valuation. Finally, review momentum to understand \
        whether the market is currently rewarding or ignoring the stock relative to the S&P 500. \
        A stock trading below both the margin of safety price and the Graham Number, with a PEG \
        under 1.0, strong fundamental growth, and positive momentum, is a strong candidate for \
        further due diligence.",
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

    let ((years, latest_pe), prices, spy_prices) = tokio::try_join!(
        fetch_fundamentals(&state, &ticker),
        state.fmp.historical_prices(&ticker, 260),
        state.fmp.historical_prices("SPY", 260),
    )?;

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

    let intrinsic_value = calculations::growth_dcf_valuation(&ticker, eps, growth_rate, 0.15)?;
    let graham = calculations::graham_number(&ticker, eps, bvps)?;
    let peg = calculations::peg_ratio(&ticker, pe, growth_rate)?;
    let momentum = calculations::momentum_score(&ticker, &prices, &spy_prices);

    let fundamentals = FundamentalsResponse { ticker: ticker.clone(), years };

    Ok(Json(SummaryResponse {
        ticker,
        fundamentals,
        growth_rates: growth,
        intrinsic_value,
        graham_number: graham,
        peg,
        momentum,
    }))
}

// ── Piotroski F-Score ─────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/piotroski",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Calculates the Piotroski F-Score (0–9), a nine-point accounting-based scoring \
        system developed by Stanford professor Joseph Piotroski to identify financially strong companies. \
        The score is built from three groups of signals: \
        Profitability (F1–F4): positive ROA, positive operating cash flow, improving ROA year-over-year, \
        and cash-backed earnings (operating cash flow > net income). \
        Leverage & Liquidity (F5–F7): decreasing long-term leverage, improving current ratio, \
        and no share dilution. \
        Operating Efficiency (F8–F9): improving gross margin and improving asset turnover. \
        Scores ≥7 indicate a financially healthy company. Scores ≤2 indicate potential distress. \
        Originally validated on value stocks but widely applicable as a quality filter.",
    responses(
        (status = 200, description = "Piotroski F-Score with individual signal breakdown", body = PiotroskiResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_piotroski(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<PiotroskiResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (income, balance, cashflow) = tokio::try_join!(
        state.fmp.income_statements(&ticker, 2),
        state.fmp.balance_sheets(&ticker, 2),
        state.fmp.cash_flow_statements(&ticker, 2),
    )?;
    Ok(Json(calculations::piotroski_f_score(&ticker, &income, &balance, &cashflow)))
}

// ── Dividend Metrics ──────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/dividends",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Returns dividend health and sustainability metrics including yield, payout ratio, \
        dividend per share, and 1-year dividend growth rate. \
        The payout ratio (dividends paid ÷ net income) is key to sustainability: \
        below 60% is generally considered safe; above 80% may indicate the company is \
        returning more than it comfortably earns — a potential cut risk. \
        In growth-based value investing, dividends are a secondary signal — consistent and growing \
        dividends can reinforce competitive strength, but a high payout ratio at the expense of \
        reinvestment may indicate slowing growth. The best dividend stocks grow their dividend \
        every year while maintaining a healthy payout ratio.",
    responses(
        (status = 200, description = "Dividend yield, payout ratio, and sustainability assessment", body = DividendMetricsResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_dividends(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<DividendMetricsResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    // fetch_list_or_empty so non-dividend-paying tickers return empty vec, not 404
    let ratios = state.fmp.ratios(&ticker, 2).await?;
    Ok(Json(calculations::dividend_metrics(&ticker, &ratios)))
}

// ── Quality Score ─────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/quality",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Composite business quality score (0–100) assessing three pillars of durability: \
        Gross margin (how much profit remains after cost of goods — high margins indicate pricing power \
        and competitive moat; >40% is excellent, >30% is reasonable); \
        Return on Equity — how efficiently the company earns returns for shareholders (>15% is strong); \
        and Debt-to-Equity ratio (lower is safer; <0.5 is conservative, >2 is leveraged). \
        High quality companies typically have wide margins, high ROE, and manageable debt — \
        the classic combination associated with durable competitive advantages. \
        Scoring: ROE contributes up to 35 points, gross margin up to 35 points plus 10 for improving trend, \
        and low D/E up to 20 points.",
    responses(
        (status = 200, description = "Business quality assessment with composite score", body = QualityScoreResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_quality(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<QualityScoreResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (income, ratios, km) = tokio::try_join!(
        state.fmp.income_statements(&ticker, 2),
        state.fmp.ratios(&ticker, 2),
        state.fmp.key_metrics(&ticker, 2),
    )?;
    Ok(Json(calculations::quality_score(&ticker, &income, &ratios, &km)))
}

// ── Momentum Score ────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/stock/{ticker}/momentum",
    tag = "stock",
    params(("ticker" = String, Path, description = "Ticker symbol, e.g. AAPL")),
    description = "Price momentum score (0–100) measuring how the stock has performed relative to \
        the S&P 500 (SPY) over the past 3, 6, and 12 months. \
        Momentum investing is grounded in decades of academic research showing that stocks which \
        have recently outperformed tend to continue outperforming in the near term. \
        Returns are calculated from daily closing prices: 3-month ≈ 63 trading days, \
        6-month ≈ 126, 12-month ≈ 252. \
        The relative strength for each period (stock return minus SPY return) is combined into \
        a composite score starting at 50 (neutral). Outperforming SPY pushes the score above 50; \
        underperforming pulls it below. Scores ≥65 indicate strong positive momentum; \
        scores ≤40 indicate the stock is lagging the market. \
        Used in factor investing alongside quality and value signals.",
    responses(
        (status = 200, description = "Momentum score with 3/6/12-month returns vs SPY", body = MomentumResponse),
        (status = 404, description = "Ticker not found", body = crate::error::ErrorBody),
        (status = 502, description = "FMP API error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_momentum(
    State(state): State<AppState>,
    Path(ticker): Path<String>,
) -> Result<Json<MomentumResponse>, AppError> {
    let ticker = ticker.to_uppercase();
    let (prices, spy_prices) = tokio::try_join!(
        state.fmp.historical_prices(&ticker, 260),
        state.fmp.historical_prices("SPY", 260),
    )?;

    Ok(Json(calculations::momentum_score(&ticker, &prices, &spy_prices)))
}
