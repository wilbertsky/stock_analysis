use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// One year of core fundamental metrics.
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

/// Up to 10 years of core fundamental metrics for a ticker.
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

/// Core fundamental growth rates.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrowthRatesResponse {
    pub ticker: String,
    pub revenue: MetricCagr,
    pub eps: MetricCagr,
    pub book_value_per_share: MetricCagr,
    pub free_cash_flow_per_share: MetricCagr,
    pub roic: MetricCagr,
}

/// Simplified DCF intrinsic value estimate.
#[derive(Debug, Serialize, ToSchema)]
pub struct IntrinsicValueResponse {
    pub ticker: String,
    /// Annual EPS growth rate used (decimal)
    pub growth_rate_used: f64,
    /// Growth-adjusted P/E = 2 × growth_rate_used × 100
    pub pe_ratio_used: f64,
    /// Estimated EPS 10 years from now
    pub future_eps: f64,
    /// Estimated stock price 10 years from now
    pub future_price: f64,
    /// Estimated intrinsic value today: future_price discounted at 15% required return for 10 years
    pub estimated_intrinsic_value: f64,
    /// Margin of safety price: 50% of estimated intrinsic value (Benjamin Graham)
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
    /// Most recent closing price (USD), taken from the first entry in the price history.
    pub current_price: Option<f64>,
    pub fundamentals: FundamentalsResponse,
    pub growth_rates: GrowthRatesResponse,
    /// None when EPS growth is zero or negative (model projects $0 future price — not meaningful).
    pub intrinsic_value: Option<IntrinsicValueResponse>,
    /// None when EPS or book value per share is zero or negative.
    pub graham_number: Option<GrahamNumberResponse>,
    /// None when EPS growth is zero or negative (PEG is undefined for non-growth companies).
    pub peg: Option<PegRatioResponse>,
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
/// All four scores are normalized 0–100. Their meaning depends on the sector
/// scoring model (see `score_labels` and `score_weights` on the response).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScreenerEntry {
    pub ticker: String,
    /// Score A (0–100) — primary signal; see `score_labels[0]`
    pub score_a: f64,
    /// Score B (0–100) — see `score_labels[1]`
    pub score_b: f64,
    /// Score C (0–100) — see `score_labels[2]`
    pub score_c: f64,
    /// Score D (0–100) — see `score_labels[3]`
    pub score_d: f64,
    /// Weighted composite of all four scores (0–100)
    pub composite_score: f64,
    /// Relative score tier based on composite_score. For educational use only — not a
    /// recommendation to buy or sell.
    pub score_tier: String,
}

/// Ranked sector screener results based on composite scoring.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SectorScreenerResponse {
    pub sector: String,
    /// Name of the scoring model applied (e.g. "Standard", "Financials", "Real Estate")
    pub scoring_model: String,
    /// Human-readable labels for score_a, score_b, score_c, score_d respectively
    pub score_labels: Vec<String>,
    /// Percentage weight strings for each score slot (e.g. ["30%", "25%", "25%", "20%"])
    pub score_weights: Vec<String>,
    pub stocks_analyzed: usize,
    /// Sorted by composite_score descending
    pub results: Vec<ScreenerEntry>,
    /// Educational disclaimer — scores are not investment advice
    pub disclaimer: String,
}

// ── Discovery Screener ──────────────────────────────────────────────────────────

/// A small/mid-cap candidate that cleared the discovery screener's quality floor and
/// falls within the near-miss band around its DCF intrinsic value.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryEntry {
    pub ticker: String,
    pub company_name: String,
    pub sector: Option<String>,
    pub market_cap: f64,
    pub current_price: f64,
    /// Graham Number: sqrt(22.5 × EPS × BVPS) (see `/stock/{ticker}/graham-number`).
    /// Used instead of the DCF intrinsic value because the DCF formula's 10-year
    /// growth-rate compounding proved far too unstable across a broad, noisy
    /// small/mid-cap universe — Graham Number's EPS×BVPS calculation has no
    /// compounding and is much more bounded.
    pub graham_number: f64,
    /// (current_price − graham_number) ÷ graham_number × 100.
    /// Negative = trading below Graham Number; positive = trading above.
    pub deviation_from_graham_number_pct: f64,
    /// Composite quality score (0–100) — see `/stock/{ticker}/quality`.
    pub quality_score: f64,
    /// Debt safety score (0–100), based on debt-to-equity ratio.
    pub debt_safety_score: f64,
    /// Piotroski F-Score (0–9) — see `/stock/{ticker}/piotroski`.
    pub piotroski_score: u8,
    /// Underlying fields that were unavailable from both EDGAR and FMP (e.g.
    /// "debt_to_equity", "gross_margin", "return_on_equity") for this candidate, after
    /// best-effort backfill between the two data sources. Empty means all data was
    /// available. When non-empty, this candidate may have failed the quality floor (or
    /// scored lower than warranted) due to missing data rather than genuinely weak
    /// fundamentals — it's still included rather than silently dropped, since the
    /// absence of data isn't evidence of a problem.
    pub missing_data_fields: Vec<String>,
}

/// Small/mid-cap "near miss" discovery results: candidates close to their DCF intrinsic
/// value with quality fundamentals above a floor (not a ceiling) — names that a
/// large-cap-only screener would never see, since they're excluded from its universe
/// before any scoring happens.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryResponse {
    /// Sector slug this response was screened against.
    pub sector: String,
    /// Market cap floor used for the candidate universe (USD).
    pub market_cap_floor: f64,
    /// Market cap ceiling used for the candidate universe (USD).
    pub market_cap_ceiling: f64,
    /// Deviation band applied around intrinsic value (percentage, both directions).
    pub deviation_band_pct: f64,
    /// Number of candidates fetched from the universe and evaluated.
    pub candidates_screened: usize,
    /// Sorted by |deviation_from_intrinsic_value_pct| ascending — closest to fair value first.
    pub results: Vec<DiscoveryEntry>,
    /// Educational disclaimer — not investment advice.
    pub disclaimer: String,
}

// ── Search ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct StockSearchResult {
    pub symbol: String,
    pub name: String,
    pub exchange: Option<String>,
    pub exchange_short: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StockSearchResponse {
    pub results: Vec<StockSearchResult>,
}

// ── Auth ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    /// Invite code assigned to this email address by an admin.
    pub invite_code: String,
    /// Optional display name shown in community portfolios instead of your email prefix.
    pub display_name: Option<String>,
}

// ── Admin / invite codes ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateInviteRequest {
    /// The email address this code is reserved for.
    pub email: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InviteCodeRow {
    pub code: String,
    pub email: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub used_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    pub user_id: Uuid,
    pub email: String,
    /// "admin" or "subscriber"
    pub role: String,
    pub has_fmp_key: bool,
    /// Optional display name. When set, shown instead of email prefix in community views.
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProfileRequest {
    /// Set to a non-empty string to use as your display name, or null to clear it.
    #[serde(default)]
    pub display_name: Option<String>,
}

// ── Password management ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

// ── Community / public portfolios ────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicPortfolioSummary {
    pub id: Uuid,
    pub name: String,
    /// Email prefix (before @) of the portfolio owner.
    pub owner: String,
    pub holdings: Vec<HoldingPerformance>,
    /// Weighted average unrealised return across open holdings (%).
    /// None only when the portfolio has no open holdings.
    pub total_return_pct: Option<f64>,
    /// Total unrealized dollar gain across open holdings.
    pub total_unrealized_gain: f64,
    /// Closed positions and cumulative realized P&L.
    pub realized: RealizedGainsSummary,
    /// Combined ranking score: realized_gain + unrealized_dollar_gain.
    pub combined_gain: f64,
}

// ── Feedback ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SubmitFeedbackRequest {
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackRow {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    /// Email of the submitter, null if the user account was deleted.
    pub user_email: Option<String>,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub is_read: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertFmpKeyRequest {
    pub fmp_key: String,
}

// ── Portfolio import ──────────────────────────────────────────────────────────

/// Per-row outcome from a CSV holdings import.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportRowResult {
    /// 1-indexed row number (including the header row, so data starts at 2).
    pub row: usize,
    pub ticker: String,
    pub ok: bool,
    /// The price stored as the cost basis (lookup or explicit override).
    pub price_used: Option<f64>,
    /// Human-readable failure reason when `ok` is false.
    pub error: Option<String>,
}

/// Summary result returned after a bulk CSV import.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportHoldingsResponse {
    pub imported: usize,
    pub failed: usize,
    /// One entry per CSV data row, sorted by row number.
    pub rows: Vec<ImportRowResult>,
}

// ── Portfolio ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePortfolioRequest {
    pub name: String,
    pub is_public: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdatePortfolioRequest {
    pub is_public: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioRow {
    pub id: Uuid,
    pub name: String,
    pub is_public: bool,
    /// Opaque token used to construct the public share URL.
    pub share_token: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AddHoldingRequest {
    pub ticker: String,
    /// Optional number of shares. Required for dollar-weighted performance.
    pub shares: Option<f64>,
    /// Optional purchase date (YYYY-MM-DD). When provided, the closing price on that
    /// date is looked up from Yahoo Finance history and used as the cost basis.
    /// Defaults to today's price when omitted.
    pub date: Option<chrono::NaiveDate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HoldingRow {
    pub id: Uuid,
    pub ticker: String,
    /// Price (USD) when the holding was added.
    pub price_at_add: f64,
    pub shares: Option<f64>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HoldingPerformance {
    #[serde(flatten)]
    pub holding: HoldingRow,
    /// Current price fetched live from FMP.
    pub current_price: f64,
    /// (current_price / price_at_add - 1) × 100, in percent.
    pub return_pct: f64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SellHoldingRequest {
    /// Number of shares to sell. Must be ≤ the holding's current share count.
    pub shares: f64,
    /// Sale price per share (USD). When omitted the current market price is fetched.
    pub price: Option<f64>,
    /// Sale date (YYYY-MM-DD). When omitted today's date/price is used.
    pub date: Option<chrono::NaiveDate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RealizedGainRow {
    pub id: Uuid,
    pub ticker: String,
    /// Shares sold in this transaction.
    pub shares: f64,
    /// Average cost basis per share at time of sale.
    pub cost_per_share: f64,
    /// Sale price per share.
    pub sale_price: f64,
    /// (sale_price − cost_per_share) × shares
    pub realized_gain: f64,
    pub sold_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RealizedGainsSummary {
    /// All individual closed positions, newest first.
    pub rows: Vec<RealizedGainRow>,
    /// Sum of all realized_gain values (positive = profit, negative = loss).
    pub total_realized_gain: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioPerformanceResponse {
    pub portfolio: PortfolioRow,
    pub holdings: Vec<HoldingPerformance>,
    /// Average return % across holdings (share-weighted where shares are provided,
    /// simple average otherwise). Null if no holdings.
    pub total_return_pct: Option<f64>,
    /// Closed positions and cumulative realized P&L for this portfolio.
    pub realized: RealizedGainsSummary,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CompanyProfileResponse {
    pub ticker: String,
    pub description: String,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub website: Option<String>,
    pub employees: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NewsItem {
    pub title: String,
    pub url: String,
    /// ISO 8601 publish date.
    pub published: Option<String>,
    pub source: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CompanyNewsResponse {
    pub ticker: String,
    pub items: Vec<NewsItem>,
}
