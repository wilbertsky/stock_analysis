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
