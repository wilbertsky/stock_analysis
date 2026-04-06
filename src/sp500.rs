//! Large-cap constituent lists — loaded at startup from public datasets.
//!
//! Combines S&P 500 and Nasdaq 100 membership so the screener covers large-cap
//! stocks from both indices. Falls back to `sectors.rs` lists if all fetches fail.

use std::collections::HashMap;

use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

const SP500_URL: &str =
    "https://raw.githubusercontent.com/datasets/s-and-p-500-companies/main/data/constituents.json";

// Wikipedia's Nasdaq 100 list as maintained by the datasets project.
const NDX_URL: &str =
    "https://raw.githubusercontent.com/datasets/nasdaq-100/main/data/constituents.json";

const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/120.0.0.0 Safari/537.36";

// ── S&P 500 deserialization ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct Sp500Constituent {
    #[serde(rename = "Symbol")]
    symbol: String,
    #[serde(rename = "GICS Sector")]
    sector: String,
}

/// Maps a GICS sector name to our URL slug.
fn gics_to_slug(gics: &str) -> Option<&'static str> {
    match gics {
        "Information Technology" => Some("technology"),
        "Health Care" => Some("healthcare"),
        "Financials" => Some("financials"),
        "Energy" => Some("energy"),
        "Consumer Staples" => Some("consumer-staples"),
        "Consumer Discretionary" => Some("consumer-discretionary"),
        "Industrials" => Some("industrials"),
        "Materials" => Some("materials"),
        "Real Estate" => Some("real-estate"),
        "Communication Services" => Some("communication"),
        "Utilities" => Some("utilities"),
        _ => None,
    }
}

// ── Nasdaq 100 deserialization ────────────────────────────────────────────────

#[derive(Deserialize)]
struct NdxConstituent {
    #[serde(rename = "Ticker")]
    ticker: String,
    #[serde(rename = "Sector")]
    sector: String,
}

/// Maps Nasdaq 100 dataset sector names to our slugs.
/// The Nasdaq 100 dataset uses GICS names matching the S&P 500 dataset.
fn ndx_sector_to_slug(sector: &str) -> Option<&'static str> {
    // Same GICS names as S&P 500 — reuse the same mapping.
    gics_to_slug(sector)
}

// ── Combined constituent store ────────────────────────────────────────────────

/// Large-cap constituent list indexed by sector slug, merged from S&P 500 + Nasdaq 100.
pub struct Sp500 {
    /// sector slug → deduplicated list of ticker symbols
    by_sector: HashMap<String, Vec<String>>,
}

impl Sp500 {
    /// Creates an empty store (triggers fallback to `sectors.rs` in screener).
    pub fn empty() -> Self {
        Self { by_sector: HashMap::new() }
    }

    /// Load S&P 500 and Nasdaq 100 constituents concurrently.
    /// Returns empty (triggering fallback) only if both fetches fail.
    pub async fn load(client: &Client) -> Self {
        let (sp500_result, ndx_result) =
            tokio::join!(fetch_sp500(client), fetch_ndx(client));

        let mut by_sector: HashMap<String, Vec<String>> = HashMap::new();

        match sp500_result {
            Ok(map) => {
                let count: usize = map.values().map(|v| v.len()).sum();
                info!(count, "Loaded S&P 500 constituents");
                merge_into(&mut by_sector, map);
            }
            Err(e) => warn!("Failed to load S&P 500 constituents: {e}"),
        }

        match ndx_result {
            Ok(map) => {
                let count: usize = map.values().map(|v| v.len()).sum();
                info!(count, "Loaded Nasdaq 100 constituents");
                merge_into(&mut by_sector, map);
            }
            Err(e) => warn!("Failed to load Nasdaq 100 constituents: {e}"),
        }

        if by_sector.is_empty() {
            warn!("All constituent fetches failed — screener will use fallback lists");
        } else {
            let total: usize = by_sector.values().map(|v| v.len()).sum();
            info!(total, "Total unique tickers across all sectors after merge");
        }

        Self { by_sector }
    }

    /// Returns all tickers in the given sector slug, or `None` if unknown / not loaded.
    pub fn tickers_for_sector(&self, sector: &str) -> Option<&[String]> {
        self.by_sector.get(sector).map(|v| v.as_slice())
    }

    /// `true` if constituent data was loaded successfully.
    pub fn is_loaded(&self) -> bool {
        !self.by_sector.is_empty()
    }
}

// ── Fetch helpers ─────────────────────────────────────────────────────────────

async fn fetch_sp500(
    client: &Client,
) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
    let constituents: Vec<Sp500Constituent> = client
        .get(SP500_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut by_sector: HashMap<String, Vec<String>> = HashMap::new();
    for c in constituents {
        if let Some(slug) = gics_to_slug(&c.sector) {
            by_sector.entry(slug.to_owned()).or_default().push(c.symbol);
        }
    }
    Ok(by_sector)
}

async fn fetch_ndx(
    client: &Client,
) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
    let constituents: Vec<NdxConstituent> = client
        .get(NDX_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut by_sector: HashMap<String, Vec<String>> = HashMap::new();
    for c in constituents {
        if let Some(slug) = ndx_sector_to_slug(&c.sector) {
            by_sector.entry(slug.to_owned()).or_default().push(c.ticker);
        }
    }
    Ok(by_sector)
}

/// Merges `src` into `dst`, skipping any ticker already present in the target sector list.
fn merge_into(dst: &mut HashMap<String, Vec<String>>, src: HashMap<String, Vec<String>>) {
    for (sector, tickers) in src {
        let entry = dst.entry(sector).or_default();
        for ticker in tickers {
            if !entry.iter().any(|t| t == &ticker) {
                entry.push(ticker);
            }
        }
    }
}
