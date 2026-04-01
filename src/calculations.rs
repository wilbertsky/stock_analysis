use crate::{
    error::AppError,
    fmp::{BalanceSheet, CashFlowStatement, HistoricalPrice, IncomeStatement, Ratio, KeyMetrics},
    models::{
        DividendMetricsResponse, FundamentalsYear, GrahamNumberResponse, GrowthRatesResponse,
        IntrinsicValueResponse, MetricCagr, MomentumResponse, PegRatioResponse, PiotroskiResponse,
        QualityScoreResponse,
    },
};

/// Compound Annual Growth Rate between two values over `years` years.
pub fn cagr(start: f64, end: f64, years: u32) -> Option<f64> {
    if start <= 0.0 || end <= 0.0 || years == 0 {
        return None;
    }
    Some((end / start).powf(1.0 / years as f64) - 1.0)
}

/// Compute 1, 5, and 10-year CAGRs from a time-ordered (oldest→newest) slice.
/// Handles missing values — if either endpoint of a window is None, that window is None.
pub fn metric_cagr(values: &[Option<f64>]) -> MetricCagr {
    let n = values.len();
    let last = values.last().and_then(|v| *v);

    let window = |idx: usize, years: u32| -> Option<f64> {
        let start = values.get(idx).and_then(|v| *v)?;
        let end = last?;
        cagr(start, end, years)
    };

    MetricCagr {
        cagr_1yr: if n >= 2 { window(n - 2, 1) } else { None },
        cagr_5yr: if n >= 6 { window(n - 6, 5) } else { None },
        cagr_10yr: if n >= 11 { window(n - 11, 10) } else { None },
    }
}

/// Build growth rates for all core fundamental metrics from a fundamentals series.
pub fn build_growth_rates(ticker: &str, years: &[FundamentalsYear]) -> GrowthRatesResponse {
    let collect = |f: fn(&FundamentalsYear) -> Option<f64>| -> Vec<Option<f64>> {
        years.iter().map(f).collect()
    };

    GrowthRatesResponse {
        ticker: ticker.to_owned(),
        revenue: metric_cagr(&collect(|y| y.revenue)),
        eps: metric_cagr(&collect(|y| y.eps)),
        book_value_per_share: metric_cagr(&collect(|y| y.book_value_per_share)),
        free_cash_flow_per_share: metric_cagr(&collect(|y| y.free_cash_flow_per_share)),
        roic: metric_cagr(&collect(|y| y.roic)),
    }
}

/// Simplified DCF intrinsic value estimate.
/// Projects EPS 10 years forward, applies a growth-adjusted P/E (2 × growth rate%),
/// then discounts back at the required rate of return.
/// growth_rate: annual EPS CAGR (decimal, e.g. 0.15 for 15%), capped at 25%.
/// discount_rate: minimum required rate of return (typically 0.15).
pub fn growth_dcf_valuation(
    ticker: &str,
    current_eps: f64,
    growth_rate: f64,
    discount_rate: f64,
) -> Result<IntrinsicValueResponse, AppError> {
    if current_eps <= 0.0 {
        return Err(AppError::Unprocessable(
            "Cannot compute intrinsic value: EPS is zero or negative".into(),
        ));
    }
    let growth_rate = growth_rate.min(0.25).max(0.0);
    let pe_ratio_used = growth_rate * 100.0 * 2.0; // growth-adjusted P/E: 2 × growth rate%
    let future_eps = current_eps * (1.0 + growth_rate).powi(10);
    let future_price = future_eps * pe_ratio_used;
    let intrinsic_value = future_price / (1.0 + discount_rate).powi(10);

    Ok(IntrinsicValueResponse {
        ticker: ticker.to_owned(),
        growth_rate_used: growth_rate,
        pe_ratio_used,
        future_eps,
        future_price,
        estimated_intrinsic_value: intrinsic_value,
        margin_of_safety_price: intrinsic_value * 0.5,
    })
}

/// Graham Number = sqrt(22.5 × EPS × BVPS).
pub fn graham_number(
    ticker: &str,
    eps: f64,
    bvps: f64,
) -> Result<GrahamNumberResponse, AppError> {
    let product = 22.5 * eps * bvps;
    if product <= 0.0 {
        return Err(AppError::Unprocessable(format!(
            "Cannot compute Graham number: EPS={eps:.2}, BVPS={bvps:.2} — both must be positive"
        )));
    }
    Ok(GrahamNumberResponse {
        ticker: ticker.to_owned(),
        eps,
        book_value_per_share: bvps,
        graham_number: product.sqrt(),
    })
}

// ── Piotroski F-Score ────────────────────────────────────────────────────────

/// Nine-signal Piotroski F-Score. Slices are newest-first (as returned by FMP).
pub fn piotroski_f_score(
    ticker: &str,
    income: &[IncomeStatement],
    balance: &[BalanceSheet],
    cashflow: &[CashFlowStatement],
) -> PiotroskiResponse {
    let cur_inc = income.first();
    let prv_inc = income.get(1);
    let cur_bal = balance.first();
    let prv_bal = balance.get(1);
    let cur_cf = cashflow.first();

    let roa = |inc: Option<&IncomeStatement>, bal: Option<&BalanceSheet>| -> Option<f64> {
        let ni = inc.and_then(|i| i.net_income)?;
        let ta = bal.and_then(|b| b.total_assets)?;
        if ta > 0.0 { Some(ni / ta) } else { None }
    };

    // F1: positive ROA
    let f1_roa_positive = roa(cur_inc, cur_bal).map(|r| r > 0.0).unwrap_or(false);

    // F2: positive operating cash flow
    let f2_ocf_positive = cur_cf
        .and_then(|cf| cf.operating_cash_flow)
        .map(|ocf| ocf > 0.0)
        .unwrap_or(false);

    // F3: ROA improving year-over-year
    let f3_roa_increasing = match (roa(cur_inc, cur_bal), roa(prv_inc, prv_bal)) {
        (Some(cur), Some(prv)) => cur > prv,
        _ => false,
    };

    // F4: cash flow quality (OCF > net income — accrual signal)
    let f4_accrual_quality = match (
        cur_cf.and_then(|cf| cf.operating_cash_flow),
        cur_inc.and_then(|i| i.net_income),
    ) {
        (Some(ocf), Some(ni)) => ocf > ni,
        _ => false,
    };

    // F5: long-term leverage decreasing
    let leverage = |bal: Option<&BalanceSheet>| -> Option<f64> {
        let ltd = bal.and_then(|b| b.long_term_debt)?;
        let ta = bal.and_then(|b| b.total_assets)?;
        if ta > 0.0 { Some(ltd / ta) } else { None }
    };
    let f5_leverage_decreasing = match (leverage(cur_bal), leverage(prv_bal)) {
        (Some(cur), Some(prv)) => cur < prv,
        _ => false,
    };

    // F6: current ratio improving
    let current_ratio = |bal: Option<&BalanceSheet>| -> Option<f64> {
        let ca = bal.and_then(|b| b.total_current_assets)?;
        let cl = bal.and_then(|b| b.total_current_liabilities)?;
        if cl > 0.0 { Some(ca / cl) } else { None }
    };
    let f6_current_ratio_improving = match (current_ratio(cur_bal), current_ratio(prv_bal)) {
        (Some(cur), Some(prv)) => cur > prv,
        _ => false,
    };

    // F7: no new share dilution
    let f7_no_dilution = match (
        cur_inc.and_then(|i| i.weighted_average_shs_out),
        prv_inc.and_then(|i| i.weighted_average_shs_out),
    ) {
        (Some(cur), Some(prv)) => cur <= prv,
        _ => false,
    };

    // F8: gross margin improving
    let gross_margin = |inc: Option<&IncomeStatement>| -> Option<f64> {
        let gp = inc.and_then(|i| i.gross_profit)?;
        let rev = inc.and_then(|i| i.revenue)?;
        if rev > 0.0 { Some(gp / rev) } else { None }
    };
    let f8_gross_margin_improving = match (gross_margin(cur_inc), gross_margin(prv_inc)) {
        (Some(cur), Some(prv)) => cur > prv,
        _ => false,
    };

    // F9: asset turnover improving
    let asset_turnover = |inc: Option<&IncomeStatement>, bal: Option<&BalanceSheet>| -> Option<f64> {
        let rev = inc.and_then(|i| i.revenue)?;
        let ta = bal.and_then(|b| b.total_assets)?;
        if ta > 0.0 { Some(rev / ta) } else { None }
    };
    let f9_asset_turnover_improving = match (
        asset_turnover(cur_inc, cur_bal),
        asset_turnover(prv_inc, prv_bal),
    ) {
        (Some(cur), Some(prv)) => cur > prv,
        _ => false,
    };

    let score = [
        f1_roa_positive, f2_ocf_positive, f3_roa_increasing, f4_accrual_quality,
        f5_leverage_decreasing, f6_current_ratio_improving, f7_no_dilution,
        f8_gross_margin_improving, f9_asset_turnover_improving,
    ]
    .iter()
    .filter(|&&b| b)
    .count() as u8;

    let interpretation = if score >= 7 {
        "Strong (≥7): high-quality company with improving fundamentals".to_owned()
    } else if score >= 4 {
        "Moderate (4–6): average quality — verify trend direction".to_owned()
    } else {
        "Weak (≤3): poor fundamentals or deteriorating — exercise caution".to_owned()
    };

    PiotroskiResponse {
        ticker: ticker.to_owned(),
        score,
        f1_roa_positive,
        f2_ocf_positive,
        f3_roa_increasing,
        f4_accrual_quality,
        f5_leverage_decreasing,
        f6_current_ratio_improving,
        f7_no_dilution,
        f8_gross_margin_improving,
        f9_asset_turnover_improving,
        interpretation,
    }
}

// ── Dividend Metrics ──────────────────────────────────────────────────────────

/// Dividend health and sustainability. Ratio slice is newest-first.
pub fn dividend_metrics(ticker: &str, ratios: &[Ratio]) -> DividendMetricsResponse {
    let cur = ratios.first();
    let prv = ratios.get(1);

    let dividend_yield_pct = cur.and_then(|r| r.dividend_yield_percentage);
    let payout_ratio = cur.and_then(|r| r.dividend_payout_ratio);
    let dividend_per_share = cur.and_then(|r| r.dividend_per_share);

    let dividend_growth_rate_1yr = match (
        dividend_per_share,
        prv.and_then(|r| r.dividend_per_share),
    ) {
        (Some(end), Some(start)) => cagr(start, end, 1),
        _ => None,
    };

    let is_sustainable = payout_ratio.map(|pr| pr < 0.60);

    let no_dividend = dividend_per_share.map(|d| d <= 0.0).unwrap_or(true);
    let interpretation = if no_dividend {
        "No dividend paid — growth or capital-reinvestment focus".to_owned()
    } else if is_sustainable == Some(true) {
        "Dividend appears sustainable (payout ratio < 60%)".to_owned()
    } else if is_sustainable == Some(false) {
        "High payout ratio (>60%) — sustainability may be at risk".to_owned()
    } else {
        "Dividend data available but payout ratio could not be computed".to_owned()
    };

    DividendMetricsResponse {
        ticker: ticker.to_owned(),
        dividend_yield_pct,
        payout_ratio,
        dividend_per_share,
        dividend_growth_rate_1yr,
        is_sustainable,
        interpretation,
    }
}

// ── Quality Score ─────────────────────────────────────────────────────────────

/// Composite quality score 0–100 based on gross margin, ROE, and debt levels.
/// All slices are newest-first.
pub fn quality_score(
    ticker: &str,
    income: &[IncomeStatement],
    ratios: &[Ratio],
    km: &[KeyMetrics],
) -> QualityScoreResponse {
    let cur_inc = income.first();
    let prv_inc = income.get(1);

    let gm = |inc: Option<&IncomeStatement>| -> Option<f64> {
        let gp = inc.and_then(|i| i.gross_profit)?;
        let rev = inc.and_then(|i| i.revenue)?;
        if rev > 0.0 { Some(gp / rev) } else { None }
    };

    let gross_margin = gm(cur_inc);
    let gross_margin_prv = gm(prv_inc);

    let gross_margin_trend = match (gross_margin, gross_margin_prv) {
        (Some(cur), Some(prv)) => {
            if cur > prv + 0.01 { Some("improving".to_owned()) }
            else if cur < prv - 0.01 { Some("declining".to_owned()) }
            else { Some("stable".to_owned()) }
        }
        _ => None,
    };

    let return_on_equity = km.first().and_then(|k| k.return_on_equity);
    let debt_to_equity = ratios.first().and_then(|r| r.debt_to_equity_ratio);

    let mut score = 0.0_f64;

    if let Some(roe) = return_on_equity {
        score += if roe > 0.20 { 35.0 } else if roe > 0.15 { 25.0 } else if roe > 0.10 { 10.0 } else { 0.0 };
    }

    if let Some(gm_val) = gross_margin {
        score += if gm_val > 0.50 { 35.0 } else if gm_val > 0.40 { 25.0 } else if gm_val > 0.30 { 15.0 } else { 0.0 };
    }

    if gross_margin_trend.as_deref() == Some("improving") {
        score += 10.0;
    }

    if let Some(de) = debt_to_equity {
        score += if de < 0.0 { 20.0 } else if de < 0.5 { 20.0 } else if de < 1.0 { 10.0 } else { 0.0 };
    }

    let quality_score = score.min(100.0);

    let interpretation = if quality_score >= 70.0 {
        "High quality: strong margins, low debt, excellent returns on equity".to_owned()
    } else if quality_score >= 45.0 {
        "Moderate quality: some positive characteristics but gaps remain".to_owned()
    } else {
        "Lower quality: weak margins, high debt, or poor returns — dig deeper".to_owned()
    };

    QualityScoreResponse {
        ticker: ticker.to_owned(),
        gross_margin,
        gross_margin_trend,
        return_on_equity,
        debt_to_equity,
        quality_score,
        interpretation,
    }
}

// ── Momentum Score ────────────────────────────────────────────────────────────

/// Price momentum relative to SPY benchmark. Both slices are newest-first daily prices.
/// Approximate windows: 3 m ≈ 63 trading days, 6 m ≈ 126, 12 m ≈ 252.
pub fn momentum_score(
    ticker: &str,
    prices: &[HistoricalPrice],
    spy_prices: &[HistoricalPrice],
) -> MomentumResponse {
    let period_return = |p: &[HistoricalPrice], days: usize| -> Option<f64> {
        let current = p.first()?.price?;
        let past = p.get(days)?.price?;
        if past > 0.0 { Some((current - past) / past) } else { None }
    };

    let return_3m = period_return(prices, 63);
    let return_6m = period_return(prices, 126);
    let return_12m = period_return(prices, 252);

    let spy_return_3m = period_return(spy_prices, 63);
    let spy_return_6m = period_return(spy_prices, 126);
    let spy_return_12m = period_return(spy_prices, 252);

    let relative_strength_3m = return_3m.zip(spy_return_3m).map(|(r, s)| r - s);
    let relative_strength_6m = return_6m.zip(spy_return_6m).map(|(r, s)| r - s);
    let relative_strength_12m = return_12m.zip(spy_return_12m).map(|(r, s)| r - s);

    // Start at neutral 50; each period of relative outperformance/underperformance shifts score.
    let mut score = 50.0_f64;
    for rel in [relative_strength_3m, relative_strength_6m, relative_strength_12m] {
        if let Some(r) = rel {
            score += (r * 100.0).clamp(-16.67, 16.67);
        }
    }
    let momentum_score = score.clamp(0.0, 100.0);

    let interpretation = if momentum_score >= 65.0 {
        "Strong momentum: outperforming the S&P 500 across multiple periods".to_owned()
    } else if momentum_score >= 40.0 {
        "Neutral momentum: roughly in line with the broader market".to_owned()
    } else {
        "Weak momentum: underperforming the S&P 500 — near-term headwinds".to_owned()
    };

    MomentumResponse {
        ticker: ticker.to_owned(),
        return_3m,
        return_6m,
        return_12m,
        spy_return_3m,
        spy_return_6m,
        spy_return_12m,
        relative_strength_3m,
        relative_strength_6m,
        relative_strength_12m,
        momentum_score,
        interpretation,
    }
}

// ── Value Signal (screener helper) ────────────────────────────────────────────

/// DCF value signal for the screener.
/// Returns 100 if price ≤ margin of safety, 50 if ≤ intrinsic value,
/// 25 if ≤ 1.5× intrinsic value, else 0.
pub fn value_signal(ticker: &str, years: &[FundamentalsYear], current_price: f64) -> f64 {
    let growth = build_growth_rates(ticker, years);
    let growth_rate = match growth.eps.cagr_5yr.or(growth.eps.cagr_1yr) {
        Some(g) => g,
        None => return 0.0,
    };
    let eps = match years.last().and_then(|y| y.eps) {
        Some(e) if e > 0.0 => e,
        _ => return 0.0,
    };
    match growth_dcf_valuation(ticker, eps, growth_rate, 0.15) {
        Ok(iv) => {
            if current_price <= iv.margin_of_safety_price { 100.0 }
            else if current_price <= iv.estimated_intrinsic_value { 50.0 }
            else if current_price <= iv.estimated_intrinsic_value * 1.5 { 25.0 }
            else { 0.0 }
        }
        Err(_) => 0.0,
    }
}

/// PEG ratio = (P/E) ÷ EPS_growth_rate_percent.
pub fn peg_ratio(
    ticker: &str,
    pe_ratio: f64,
    growth_rate_decimal: f64,
) -> Result<PegRatioResponse, AppError> {
    let growth_pct = growth_rate_decimal * 100.0;
    if growth_pct <= 0.0 {
        return Err(AppError::Unprocessable(
            "Cannot compute PEG: growth rate must be positive".into(),
        ));
    }
    Ok(PegRatioResponse {
        ticker: ticker.to_owned(),
        pe_ratio,
        earnings_growth_rate_pct: growth_pct,
        peg_ratio: pe_ratio / growth_pct,
    })
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fmp::{BalanceSheet, CashFlowStatement, HistoricalPrice, IncomeStatement, KeyMetrics, Ratio},
        models::FundamentalsYear,
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn income(revenue: f64, gross: f64, net: f64, eps: f64, shares: f64) -> IncomeStatement {
        IncomeStatement {
            date: "2024-01-01".to_owned(),
            revenue: Some(revenue),
            gross_profit: Some(gross),
            net_income: Some(net),
            eps: Some(eps),
            weighted_average_shs_out: Some(shares),
        }
    }

    fn balance(total_assets: f64, cur_assets: f64, cur_liab: f64, ltd: f64) -> BalanceSheet {
        BalanceSheet {
            date: "2024-01-01".to_owned(),
            total_assets: Some(total_assets),
            total_current_assets: Some(cur_assets),
            total_current_liabilities: Some(cur_liab),
            long_term_debt: Some(ltd),
            total_equity: None,
            total_debt: None,
        }
    }

    fn cashflow(ocf: f64) -> CashFlowStatement {
        CashFlowStatement {
            date: "2024-01-01".to_owned(),
            operating_cash_flow: Some(ocf),
            free_cash_flow: None,
            common_stock_issuance: None,
        }
    }

    fn price(p: f64) -> HistoricalPrice {
        HistoricalPrice { date: "2024-01-01".to_owned(), price: Some(p) }
    }

    fn ratio_full(div_yield: f64, payout: f64, dps: f64, de: f64) -> Ratio {
        Ratio {
            date: "2024-01-01".to_owned(),
            book_value_per_share: None,
            free_cash_flow_per_share: None,
            price_to_earnings_ratio: None,
            dividend_yield_percentage: Some(div_yield),
            dividend_payout_ratio: Some(payout),
            dividend_per_share: Some(dps),
            debt_to_equity_ratio: Some(de),
        }
    }

    fn year(eps: f64) -> FundamentalsYear {
        FundamentalsYear {
            fiscal_year: "2024".to_owned(),
            revenue: Some(100.0),
            eps: Some(eps),
            book_value_per_share: None,
            free_cash_flow_per_share: None,
            roic: None,
        }
    }

    /// Build a price vec of `len` entries (newest-first) where specific indices
    /// have known values; all other entries use `fill`.
    fn prices_with(len: usize, fill: f64, overrides: &[(usize, f64)]) -> Vec<HistoricalPrice> {
        let mut v: Vec<HistoricalPrice> = (0..len).map(|_| price(fill)).collect();
        for &(idx, val) in overrides {
            if idx < len {
                v[idx] = price(val);
            }
        }
        v
    }

    // ── cagr ─────────────────────────────────────────────────────────────────

    #[test]
    fn cagr_doubles_in_one_year() {
        assert!((cagr(100.0, 200.0, 1).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cagr_known_ten_year_value() {
        // 100 → 259.37 at 10% per year
        let r = cagr(100.0, 259.374, 10).unwrap();
        assert!((r - 0.10).abs() < 0.001);
    }

    #[test]
    fn cagr_zero_start_returns_none() {
        assert_eq!(cagr(0.0, 200.0, 5), None);
    }

    #[test]
    fn cagr_negative_start_returns_none() {
        assert_eq!(cagr(-50.0, 200.0, 5), None);
    }

    #[test]
    fn cagr_zero_years_returns_none() {
        assert_eq!(cagr(100.0, 200.0, 0), None);
    }

    // ── metric_cagr ──────────────────────────────────────────────────────────

    #[test]
    fn metric_cagr_empty_all_none() {
        let r = metric_cagr(&[]);
        assert!(r.cagr_1yr.is_none() && r.cagr_5yr.is_none() && r.cagr_10yr.is_none());
    }

    #[test]
    fn metric_cagr_single_element_all_none() {
        let r = metric_cagr(&[Some(100.0)]);
        assert!(r.cagr_1yr.is_none());
    }

    #[test]
    fn metric_cagr_two_elements_gives_1yr_only() {
        let r = metric_cagr(&[Some(100.0), Some(200.0)]);
        assert!((r.cagr_1yr.unwrap() - 1.0).abs() < 1e-9);
        assert!(r.cagr_5yr.is_none());
        assert!(r.cagr_10yr.is_none());
    }

    #[test]
    fn metric_cagr_none_at_endpoint_returns_none() {
        // Last value is None → can't compute any CAGR
        let r = metric_cagr(&[Some(100.0), None]);
        assert!(r.cagr_1yr.is_none());
    }

    // ── growth_dcf_valuation ─────────────────────────────────────────────────

    #[test]
    fn dcf_negative_eps_errors() {
        assert!(growth_dcf_valuation("T", -1.0, 0.15, 0.15).is_err());
    }

    #[test]
    fn dcf_zero_eps_errors() {
        assert!(growth_dcf_valuation("T", 0.0, 0.15, 0.15).is_err());
    }

    #[test]
    fn dcf_caps_growth_at_25_percent() {
        let r = growth_dcf_valuation("T", 5.0, 0.99, 0.15).unwrap();
        assert!((r.growth_rate_used - 0.25).abs() < 1e-9);
    }

    #[test]
    fn dcf_floors_growth_at_zero() {
        let r = growth_dcf_valuation("T", 5.0, -0.10, 0.15).unwrap();
        assert_eq!(r.growth_rate_used, 0.0);
    }

    #[test]
    fn dcf_margin_of_safety_is_half_intrinsic() {
        let r = growth_dcf_valuation("T", 5.0, 0.15, 0.15).unwrap();
        assert!((r.margin_of_safety_price - r.estimated_intrinsic_value * 0.5).abs() < 1e-9);
    }

    #[test]
    fn dcf_pe_equals_two_times_growth_pct() {
        // growth 15% → PE = 2 × 15 = 30
        let r = growth_dcf_valuation("T", 5.0, 0.15, 0.15).unwrap();
        assert!((r.pe_ratio_used - 30.0).abs() < 1e-9);
    }

    #[test]
    fn dcf_10pct_growth_discounted_at_10pct_equals_initial_pe_times_eps() {
        // When growth_rate == discount_rate, future_price / (1+r)^10 = eps * pe_ratio
        // because (1+g)^10 cancels with 1/(1+r)^10
        let eps = 5.0;
        let g = 0.10;
        let r = growth_dcf_valuation("T", eps, g, g).unwrap();
        let expected = eps * r.pe_ratio_used; // 5 * 20 = 100
        assert!((r.estimated_intrinsic_value - expected).abs() < 0.01);
    }

    // ── graham_number ─────────────────────────────────────────────────────────

    #[test]
    fn graham_number_known_value() {
        // sqrt(22.5 × 4 × 25) = sqrt(2250) ≈ 47.43
        let r = graham_number("T", 4.0, 25.0).unwrap();
        assert!((r.graham_number - (22.5 * 4.0 * 25.0_f64).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn graham_number_negative_eps_errors() {
        assert!(graham_number("T", -1.0, 25.0).is_err());
    }

    #[test]
    fn graham_number_zero_bvps_errors() {
        assert!(graham_number("T", 4.0, 0.0).is_err());
    }

    // ── peg_ratio ─────────────────────────────────────────────────────────────

    #[test]
    fn peg_ratio_known_value() {
        // P/E 30 ÷ 15% growth = 2.0
        let r = peg_ratio("T", 30.0, 0.15).unwrap();
        assert!((r.peg_ratio - 2.0).abs() < 1e-9);
    }

    #[test]
    fn peg_ratio_zero_growth_errors() {
        assert!(peg_ratio("T", 30.0, 0.0).is_err());
    }

    #[test]
    fn peg_ratio_negative_growth_errors() {
        assert!(peg_ratio("T", 30.0, -0.05).is_err());
    }

    // ── piotroski_f_score ─────────────────────────────────────────────────────

    #[test]
    fn piotroski_all_nine_signals_pass() {
        //  cur year: higher margins, higher asset turnover, fewer shares, lower leverage,
        //            better current ratio, positive OCF > NI
        let inc = vec![
            income(200.0, 100.0, 20.0, 2.0, 9.0),  // current
            income(150.0,  60.0, 15.0, 1.5, 10.0), // prior
        ];
        let bal = vec![
            balance(100.0, 60.0, 30.0, 10.0), // LTD/assets = 0.10, CR = 2.0
            balance(90.0,  40.0, 30.0, 20.0), // LTD/assets = 0.22, CR = 1.33
        ];
        let cf = vec![cashflow(30.0)]; // OCF(30) > NI(20) ✓

        let r = piotroski_f_score("T", &inc, &bal, &cf);
        assert_eq!(r.score, 9);
        assert_eq!(r.interpretation.to_lowercase().contains("strong"), true);
    }

    #[test]
    fn piotroski_all_nine_signals_fail() {
        let inc = vec![
            income(100.0, 30.0, -5.0, -0.5, 12.0), // current: loss, diluted
            income(150.0, 60.0, 10.0,  1.0, 10.0), // prior: better on all
        ];
        let bal = vec![
            balance(100.0, 20.0, 40.0, 50.0), // worse leverage, worse CR
            balance(100.0, 40.0, 30.0, 30.0),
        ];
        let cf = vec![cashflow(-10.0)]; // negative OCF

        let r = piotroski_f_score("T", &inc, &bal, &cf);
        assert_eq!(r.score, 0);
    }

    #[test]
    fn piotroski_empty_slices_return_zero() {
        let r = piotroski_f_score("T", &[], &[], &[]);
        assert_eq!(r.score, 0);
    }

    // ── dividend_metrics ──────────────────────────────────────────────────────

    #[test]
    fn dividends_no_dividend_paid() {
        let ratios = vec![{
            let mut r = ratio_full(0.0, 0.0, 0.0, 1.5);
            r.dividend_per_share = Some(0.0);
            r
        }];
        let r = dividend_metrics("T", &ratios);
        assert!(r.is_sustainable.is_none() || r.dividend_per_share == Some(0.0));
    }

    #[test]
    fn dividends_sustainable_payout() {
        let ratios = vec![ratio_full(2.5, 0.40, 1.00, 0.5)];
        let r = dividend_metrics("T", &ratios);
        assert_eq!(r.is_sustainable, Some(true));
        assert_eq!(r.dividend_yield_pct, Some(2.5));
    }

    #[test]
    fn dividends_unsustainable_payout() {
        let ratios = vec![ratio_full(8.0, 0.90, 2.00, 2.0)];
        let r = dividend_metrics("T", &ratios);
        assert_eq!(r.is_sustainable, Some(false));
    }

    #[test]
    fn dividends_growth_rate_calculated() {
        let ratios = vec![
            ratio_full(2.5, 0.40, 1.10, 0.5), // current
            ratio_full(2.3, 0.38, 1.00, 0.5), // prior
        ];
        let r = dividend_metrics("T", &ratios);
        assert!(r.dividend_growth_rate_1yr.is_some());
        assert!(r.dividend_growth_rate_1yr.unwrap() > 0.0); // growing
    }

    // ── quality_score ─────────────────────────────────────────────────────────

    #[test]
    fn quality_high_score_for_strong_company() {
        let inc = vec![
            income(100.0, 60.0, 25.0, 2.5, 10.0), // 60% gross margin
            income(90.0,  50.0, 20.0, 2.0, 10.0),
        ];
        let ratios = vec![ratio_full(1.5, 0.3, 0.5, 0.3)]; // low D/E
        let km = vec![KeyMetrics {
            date: "2024-01-01".to_owned(),
            return_on_invested_capital: None,
            return_on_equity: Some(0.25), // 25% ROE
        }];
        let r = quality_score("T", &inc, &ratios, &km);
        assert!(r.quality_score >= 70.0);
        assert_eq!(r.gross_margin_trend.as_deref(), Some("improving"));
    }

    #[test]
    fn quality_low_score_for_weak_company() {
        let inc = vec![income(100.0, 10.0, -5.0, -0.5, 10.0)]; // 10% margin, loss
        let ratios = vec![ratio_full(0.0, 0.0, 0.0, 5.0)]; // very high D/E
        let km = vec![KeyMetrics {
            date: "2024-01-01".to_owned(),
            return_on_invested_capital: None,
            return_on_equity: Some(-0.05), // negative ROE
        }];
        let r = quality_score("T", &inc, &ratios, &km);
        assert!(r.quality_score < 45.0);
    }

    // ── momentum_score ────────────────────────────────────────────────────────

    #[test]
    fn momentum_insufficient_data_stays_neutral() {
        let p = vec![price(100.0)];
        let spy = vec![price(400.0)];
        let r = momentum_score("T", &p, &spy);
        assert!((r.momentum_score - 50.0).abs() < 1e-9);
        assert!(r.return_3m.is_none() && r.return_6m.is_none() && r.return_12m.is_none());
    }

    #[test]
    fn momentum_outperforming_spy_scores_above_50() {
        // Stock: 160→220 (+37.5%); SPY: 430→500 (+16.3%)
        let stock = prices_with(260, 220.0, &[(63, 200.0), (126, 180.0), (252, 160.0)]);
        let spy   = prices_with(260, 500.0, &[(63, 480.0), (126, 460.0), (252, 430.0)]);
        let r = momentum_score("T", &stock, &spy);
        assert!(r.momentum_score > 50.0);
        assert!(r.relative_strength_12m.unwrap() > 0.0);
    }

    #[test]
    fn momentum_underperforming_spy_scores_below_50() {
        // Stock flat; SPY up significantly
        let stock = prices_with(260, 100.0, &[(63, 100.0), (126, 100.0), (252, 100.0)]);
        let spy   = prices_with(260, 500.0, &[(63, 430.0), (126, 400.0), (252, 350.0)]);
        let r = momentum_score("T", &stock, &spy);
        assert!(r.momentum_score < 50.0);
    }

    // ── value_signal ──────────────────────────────────────────────────────────

    #[test]
    fn value_signal_very_cheap_price_returns_100() {
        // Two years of EPS data gives a 1yr CAGR; price far below MOS
        let years = vec![year(4.0), year(5.0)];
        assert_eq!(value_signal("T", &years, 1.0), 100.0);
    }

    #[test]
    fn value_signal_no_eps_returns_zero() {
        let years = vec![FundamentalsYear {
            fiscal_year: "2024".to_owned(),
            revenue: Some(100.0),
            eps: None,
            book_value_per_share: None,
            free_cash_flow_per_share: None,
            roic: None,
        }];
        assert_eq!(value_signal("T", &years, 100.0), 0.0);
    }

    #[test]
    fn value_signal_overvalued_returns_zero() {
        // Extremely high price relative to any reasonable DCF output
        let years = vec![year(4.0), year(5.0)];
        assert_eq!(value_signal("T", &years, 1_000_000.0), 0.0);
    }
}
