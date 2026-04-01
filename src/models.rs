use serde::Serialize;
use utoipa::ToSchema;

/// One year of the Big Five fundamentals.
#[derive(Debug, Serialize, ToSchema)]
pub struct FundamentalsYear {
    pub fiscal_year: String,
    pub revenue: Option<f64>,
    pub eps: Option<f64>,
    pub book_value_per_share: Option<f64>,
    pub free_cash_flow_per_share: Option<f64>,
    /// Return on Invested Capital (decimal, e.g. 0.15 = 15%)
    pub roic: Option<f64>,
}

/// Up to 10 years of Big Five fundamentals for a ticker.
#[derive(Debug, Serialize, ToSchema)]
pub struct FundamentalsResponse {
    pub ticker: String,
    /// Sorted oldest → newest
    pub years: Vec<FundamentalsYear>,
}

/// CAGR results for a single metric across multiple time windows.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricCagr {
    pub cagr_1yr: Option<f64>,
    pub cagr_5yr: Option<f64>,
    pub cagr_10yr: Option<f64>,
}

/// Rule #1 Big Five growth rates.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrowthRatesResponse {
    pub ticker: String,
    pub revenue: MetricCagr,
    pub eps: MetricCagr,
    pub book_value_per_share: MetricCagr,
    pub free_cash_flow_per_share: MetricCagr,
    pub roic: MetricCagr,
}

/// Rule #1 Phil Town sticker price calculation.
#[derive(Debug, Serialize, ToSchema)]
pub struct StickerPriceResponse {
    pub ticker: String,
    /// Annual EPS growth rate used (decimal)
    pub growth_rate_used: f64,
    /// Default P/E = 2 × growth_rate_used × 100
    pub pe_ratio_used: f64,
    /// Estimated EPS 10 years from now
    pub future_eps: f64,
    /// Estimated stock price 10 years from now
    pub future_price: f64,
    /// Estimated current sticker price: future_price discounted at 15% MARR for 10 years
    pub estimated_current_sticker_price: f64,
    /// Margin of safety price: 50% of sticker price
    pub margin_of_safety_price: f64,
}

/// Graham number intrinsic value estimate.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrahamNumberResponse {
    pub ticker: String,
    pub eps: f64,
    pub book_value_per_share: f64,
    /// sqrt(22.5 × EPS × BVPS)
    pub graham_number: f64,
}

/// PEG ratio.
#[derive(Debug, Serialize, ToSchema)]
pub struct PegRatioResponse {
    pub ticker: String,
    pub pe_ratio: f64,
    /// EPS growth rate as a percentage (e.g. 15.0 for 15%)
    pub earnings_growth_rate_pct: f64,
    /// P/E ÷ earnings_growth_rate_pct
    pub peg_ratio: f64,
}

/// All valuations combined in a single response.
#[derive(Debug, Serialize, ToSchema)]
pub struct SummaryResponse {
    pub ticker: String,
    pub fundamentals: FundamentalsResponse,
    pub growth_rates: GrowthRatesResponse,
    pub sticker_price: StickerPriceResponse,
    pub graham_number: GrahamNumberResponse,
    pub peg: PegRatioResponse,
    pub momentum: MomentumResponse,
}

/// Health check response.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

// ── Piotroski F-Score ────────────────────────────────────────────────────────

/// Piotroski F-Score (0–9) with individual signal breakdown.
#[derive(Debug, Serialize, ToSchema)]
pub struct PiotroskiResponse {
    pub ticker: String,
    /// Total score 0–9. ≥7 = strong, 3–6 = moderate, ≤2 = weak.
    pub score: u8,
    // Profitability signals
    pub f1_roa_positive: bool,
    pub f2_ocf_positive: bool,
    pub f3_roa_increasing: bool,
    pub f4_accrual_quality: bool,
    // Leverage / liquidity signals
    pub f5_leverage_decreasing: bool,
    pub f6_current_ratio_improving: bool,
    pub f7_no_dilution: bool,
    // Operating efficiency signals
    pub f8_gross_margin_improving: bool,
    pub f9_asset_turnover_improving: bool,
    pub interpretation: String,
}

// ── Dividend Metrics ─────────────────────────────────────────────────────────

/// Dividend health and sustainability metrics.
#[derive(Debug, Serialize, ToSchema)]
pub struct DividendMetricsResponse {
    pub ticker: String,
    /// Annual dividend yield as a percentage (e.g. 2.5 = 2.5%)
    pub dividend_yield_pct: Option<f64>,
    /// Fraction of earnings paid as dividends (e.g. 0.40 = 40%)
    pub payout_ratio: Option<f64>,
    /// Annual dividend per share (USD)
    pub dividend_per_share: Option<f64>,
    /// 1-year CAGR of dividend per share
    pub dividend_growth_rate_1yr: Option<f64>,
    /// True if payout ratio < 60% — generally considered sustainable
    pub is_sustainable: Option<bool>,
    pub interpretation: String,
}

// ── Quality Score ─────────────────────────────────────────────────────────────

/// Quality score assessing business durability (0–100).
#[derive(Debug, Serialize, ToSchema)]
pub struct QualityScoreResponse {
    pub ticker: String,
    /// Gross profit / revenue for most recent year
    pub gross_margin: Option<f64>,
    /// "improving", "stable", or "declining" year-over-year
    pub gross_margin_trend: Option<String>,
    /// Return on Equity (decimal, e.g. 0.20 = 20%)
    pub return_on_equity: Option<f64>,
    /// Total debt / total equity ratio
    pub debt_to_equity: Option<f64>,
    /// Composite quality score 0–100
    pub quality_score: f64,
    pub interpretation: String,
}

// ── Momentum Score ────────────────────────────────────────────────────────────

/// Price momentum relative to the S&P 500 (SPY benchmark).
#[derive(Debug, Serialize, ToSchema)]
pub struct MomentumResponse {
    pub ticker: String,
    pub return_3m: Option<f64>,
    pub return_6m: Option<f64>,
    pub return_12m: Option<f64>,
    pub spy_return_3m: Option<f64>,
    pub spy_return_6m: Option<f64>,
    pub spy_return_12m: Option<f64>,
    /// Stock return minus SPY return (positive = outperforming)
    pub relative_strength_3m: Option<f64>,
    pub relative_strength_6m: Option<f64>,
    pub relative_strength_12m: Option<f64>,
    /// Composite momentum score 0–100
    pub momentum_score: f64,
    pub interpretation: String,
}

// ── Sector Screener ───────────────────────────────────────────────────────────

/// A single stock's scores within a sector screener result.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScreenerEntry {
    pub ticker: String,
    pub piotroski_score: u8,
    pub quality_score: f64,
    pub momentum_score: f64,
    /// Rule #1 value signal: 100 = below MOS, 50 = below sticker, 25 = within 1.5× sticker, 0 = overvalued
    pub value_signal: f64,
    /// Weighted composite: piotroski 30% + quality 25% + value 25% + momentum 20%
    pub composite_score: f64,
    pub signal: String,
}

/// Ranked stock picks for a sector based on composite scoring.
#[derive(Debug, Serialize, ToSchema)]
pub struct SectorScreenerResponse {
    pub sector: String,
    pub stocks_analyzed: usize,
    /// Sorted by composite_score descending
    pub results: Vec<ScreenerEntry>,
}
