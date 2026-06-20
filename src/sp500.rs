//! Large-cap constituent lists — sourced from FMP's `/stable/company-screener`.
//!
//! Previously this pulled S&P 500 + Nasdaq 100 membership from two free GitHub-hosted
//! JSON datasets. Both are now dead: the S&P 500 repo dropped its `.json` export in
//! favor of `.csv` (constituents.json now 404s), and the `datasets/nasdaq-100` repo no
//! longer exists at all. FMP's own index-constituent endpoints (`/sp500-constituent`,
//! `/nasdaq-constituent`) returned HTTP 402 (not included on the Starter plan) when
//! checked directly, so this loads a market-cap-band approximation of "large cap" via
//! `/stable/company-screener` instead, which **is** available on Starter.
//!
//! Note this is an approximation, not literal index membership: company-screener
//! returns everything above the market-cap floor in a sector, not the specific set of
//! names an index committee selected. For this app's purpose — sourcing a reasonable
//! large-cap candidate pool for the sector screener — that's an acceptable trade, and
//! more transparent than depending on a third party's free dataset staying online.

use std::collections::HashMap;

use tracing::{info, warn};

use crate::{fmp::FmpClient, sectors};

/// Large-cap floor (USD). Standard convention for "large cap" is ≥$10B.
const LARGE_CAP_FLOOR: u64 = 10_000_000_000;

/// Candidates fetched per sector. `screener.rs::top_by_market_cap` further trims this
/// down to `SCORE_TOP_N` (20), so this just needs to be comfortably larger than that.
const FETCH_LIMIT_PER_SECTOR: u32 = 50;

/// Large-cap constituent list indexed by sector slug, sourced from FMP company-screener.
pub struct Sp500 {
    /// sector slug → tickers sorted by market cap descending
    by_sector: HashMap<String, Vec<String>>,
}

impl Sp500 {
    /// Creates an empty store (triggers fallback to `sectors.rs` in screener).
    pub fn empty() -> Self {
        Self { by_sector: HashMap::new() }
    }

    /// Loads a large-cap candidate list per sector from FMP's company-screener.
    /// Sectors that fail individually are skipped (logged); the store is empty only if
    /// every sector fails, which triggers the `sectors.rs` hardcoded fallback.
    pub async fn load(fmp: &FmpClient) -> Self {
        let mut by_sector: HashMap<String, Vec<String>> = HashMap::new();

        for &slug in sectors::ALL_SECTOR_SLUGS {
            let Some(fmp_sector) = sectors::slug_to_fmp_sector(slug) else {
                continue; // unreachable given ALL_SECTOR_SLUGS is kept in sync, but stay safe
            };

            match fmp
                .company_screener(LARGE_CAP_FLOOR, None, Some(fmp_sector), FETCH_LIMIT_PER_SECTOR)
                .await
            {
                Ok(mut candidates) => {
                    candidates.sort_by(|a, b| {
                        b.market_cap.partial_cmp(&a.market_cap).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let tickers: Vec<String> = candidates.into_iter().map(|c| c.symbol).collect();
                    info!(sector = slug, count = tickers.len(), "Loaded large-cap candidates from FMP");
                    by_sector.insert(slug.to_owned(), tickers);
                }
                Err(e) => warn!(sector = slug, "Failed to load large-cap candidates from FMP: {e}"),
            }
        }

        if by_sector.is_empty() {
            warn!("All FMP company-screener sector fetches failed — screener will use sectors.rs fallback");
        } else {
            let total: usize = by_sector.values().map(|v| v.len()).sum();
            info!(total, sectors = by_sector.len(), "Total large-cap tickers loaded across all sectors");
        }

        Self { by_sector }
    }

    /// Returns all tickers in the given sector slug, or `None` if unknown / not loaded.
    pub fn tickers_for_sector(&self, sector: &str) -> Option<&[String]> {
        self.by_sector.get(sector).map(|v| v.as_slice())
    }

    /// `true` if at least one sector's constituent data was loaded successfully.
    pub fn is_loaded(&self) -> bool {
        !self.by_sector.is_empty()
    }
}
