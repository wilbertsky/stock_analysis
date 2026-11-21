//! Price-only forecast: volatility-scaled lognormal range cone + walk-forward
//! calibration backtest. Deliberately separate from fundamentals (calculations.rs).
//!
//! This forecasts a *range*, not a direction: drift is set so the expected price
//! equals today's price, so the median sits slightly below spot (the usual
//! lognormal skew). Volatility is a RiskMetrics EWMA (lambda 0.94), which is
//! GARCH(1,1) with fixed parameters.
// ponytail: closed-form lognormal quantiles instead of Monte Carlo paths — identical
// distribution, deterministic, no RNG. Switch to bootstrapped paths if fat tails matter.

use serde::Serialize;
use utoipa::ToSchema;

const LAMBDA: f64 = 0.94;
const TRADING_DAYS: f64 = 252.0;
/// Minimum history before the backtest starts forecasting (about 1 year).
const WARMUP: usize = 252;
/// Spacing between backtest origins, in trading days.
const BACKTEST_STRIDE: usize = 5;
pub const MAX_HORIZON: usize = 252;

#[derive(Debug, Serialize, ToSchema)]
pub struct ForecastBand {
    /// Trading days ahead (1-based).
    pub day: usize,
    pub lo95: f64,
    pub lo68: f64,
    pub median: f64,
    pub hi68: f64,
    pub hi95: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Backtest {
    pub horizon_days: usize,
    /// Number of (overlapping) forecast windows evaluated.
    pub samples: usize,
    /// Share of outcomes inside the 68% band. A calibrated model scores ≈ 0.68.
    pub coverage68: f64,
    /// Share of outcomes inside the 95% band. A calibrated model scores ≈ 0.95.
    pub coverage95: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ForecastResponse {
    pub ticker: String,
    pub last_price: f64,
    pub last_date: String,
    pub horizon_days: usize,
    /// EWMA daily volatility of log returns.
    pub daily_vol: f64,
    /// daily_vol × √252.
    pub annual_vol: f64,
    pub bands: Vec<ForecastBand>,
    /// None when there is too little history to backtest.
    pub backtest: Option<Backtest>,
}

fn log_returns(closes: &[f64]) -> Vec<f64> {
    closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect()
}

/// EWMA daily volatility from log returns (oldest → newest).
fn ewma_vol(returns: &[f64]) -> f64 {
    let mut var = returns.iter().take(20).map(|r| r * r).sum::<f64>() / returns.len().min(20).max(1) as f64;
    for r in returns {
        var = LAMBDA * var + (1.0 - LAMBDA) * r * r;
    }
    var.sqrt()
}

/// (lo95, lo68, median, hi68, hi95) after `days` trading days.
fn band(price: f64, daily_vol: f64, days: usize) -> (f64, f64, f64, f64, f64) {
    let s = daily_vol * (days as f64).sqrt();
    // E[P_t] = P_0  =>  log drift = -s²/2
    let m = -0.5 * s * s;
    let q = |z: f64| price * (m + z * s).exp();
    (q(-1.96), q(-1.0), q(0.0), q(1.0), q(1.96))
}

/// Fraction of walk-forward forecasts whose realised price landed inside each band.
fn backtest(closes: &[f64], horizon: usize) -> Option<Backtest> {
    if closes.len() < WARMUP + horizon + 1 {
        return None;
    }
    let (mut n, mut in68, mut in95) = (0usize, 0usize, 0usize);
    let mut t = WARMUP;
    while t + horizon < closes.len() {
        // Only data up to index t is used for the forecast — no look-ahead.
        let vol = ewma_vol(&log_returns(&closes[..=t]));
        let (lo95, lo68, _, hi68, hi95) = band(closes[t], vol, horizon);
        let actual = closes[t + horizon];
        n += 1;
        in68 += (actual >= lo68 && actual <= hi68) as usize;
        in95 += (actual >= lo95 && actual <= hi95) as usize;
        t += BACKTEST_STRIDE;
    }
    Some(Backtest {
        horizon_days: horizon,
        samples: n,
        coverage68: in68 as f64 / n as f64,
        coverage95: in95 as f64 / n as f64,
    })
}

/// `closes`/`dates` are chronological (oldest → newest). Returns None if < 30 closes.
pub fn forecast(ticker: &str, dates: &[String], closes: &[f64], horizon: usize) -> Option<ForecastResponse> {
    if closes.len() < 30 || closes.len() != dates.len() {
        return None;
    }
    let horizon = horizon.clamp(1, MAX_HORIZON);
    let last = *closes.last()?;
    let vol = ewma_vol(&log_returns(closes));
    let bands = (1..=horizon)
        .map(|day| {
            let (lo95, lo68, median, hi68, hi95) = band(last, vol, day);
            ForecastBand { day, lo95, lo68, median, hi68, hi95 }
        })
        .collect();
    Some(ForecastResponse {
        ticker: ticker.to_string(),
        last_price: last,
        last_date: dates.last()?.clone(),
        horizon_days: horizon,
        daily_vol: vol,
        annual_vol: vol * TRADING_DAYS.sqrt(),
        bands,
        backtest: backtest(closes, horizon),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random walk with ~1% daily vol (LCG + Box-Muller, no deps).
    fn walk(n: usize) -> Vec<f64> {
        let mut seed = 42u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0)
        };
        let mut p = 100.0;
        (0..n)
            .map(|_| {
                let z = (-2.0 * next().ln()).sqrt() * (2.0 * std::f64::consts::PI * next()).cos();
                p *= (0.01 * z).exp();
                p
            })
            .collect()
    }

    #[test]
    fn bands_are_ordered_and_widen() {
        let c = walk(400);
        let d: Vec<String> = (0..400).map(|i| i.to_string()).collect();
        let f = forecast("X", &d, &c, 63).unwrap();
        for b in &f.bands {
            assert!(b.lo95 < b.lo68 && b.lo68 < b.median && b.median < b.hi68 && b.hi68 < b.hi95);
        }
        assert!(f.bands[62].hi95 - f.bands[62].lo95 > f.bands[0].hi95 - f.bands[0].lo95);
    }

    #[test]
    fn backtest_is_roughly_calibrated_on_gbm() {
        let c = walk(2000);
        let bt = backtest(&c, 21).unwrap();
        assert!((bt.coverage68 - 0.68).abs() < 0.10, "68% coverage {}", bt.coverage68);
        assert!((bt.coverage95 - 0.95).abs() < 0.06, "95% coverage {}", bt.coverage95);
    }
}
