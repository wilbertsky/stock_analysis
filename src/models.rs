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
}

/// Health check response.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}
