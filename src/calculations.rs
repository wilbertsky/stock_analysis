use crate::{
    error::AppError,
    models::{
        FundamentalsYear, GrowthRatesResponse, MetricCagr, GrahamNumberResponse,
        PegRatioResponse, StickerPriceResponse,
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

/// Build growth rates for all Big Five metrics from a fundamentals series.
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

/// Rule #1 Phil Town sticker price.
/// growth_rate: annual EPS CAGR (decimal, e.g. 0.15 for 15%), capped at 25%.
/// discount_rate: minimum acceptable rate of return (typically 0.15).
pub fn rule1_sticker_price(
    ticker: &str,
    current_eps: f64,
    growth_rate: f64,
    discount_rate: f64,
) -> Result<StickerPriceResponse, AppError> {
    if current_eps <= 0.0 {
        return Err(AppError::Unprocessable(
            "Cannot compute sticker price: EPS is zero or negative".into(),
        ));
    }
    let growth_rate = growth_rate.min(0.25).max(0.0);
    let pe_ratio_used = growth_rate * 100.0 * 2.0; // Rule #1: P/E = 2 × growth%
    let future_eps = current_eps * (1.0 + growth_rate).powi(10);
    let future_price = future_eps * pe_ratio_used;
    let sticker_price = future_price / (1.0 + discount_rate).powi(10);

    Ok(StickerPriceResponse {
        ticker: ticker.to_owned(),
        growth_rate_used: growth_rate,
        pe_ratio_used,
        future_eps,
        future_price,
        estimated_current_sticker_price: sticker_price,
        margin_of_safety_price: sticker_price * 0.5,
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
