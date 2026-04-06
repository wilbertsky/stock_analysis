//! Yahoo Finance client — price history and news.
//!
//! Uses unofficial Yahoo Finance endpoints that require no API key.
//! These APIs have been stable for years and are widely used
//! in open-source financial tooling.

use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::error::AppError;
use crate::fmp::HistoricalPrice;

const CHART_BASE: &str = "https://query1.finance.yahoo.com/v8/finance/chart";
const QUOTE_URL: &str = "https://query2.finance.yahoo.com/v7/finance/quote";
const NEWS_RSS_BASE: &str = "https://feeds.finance.yahoo.com/rss/2.0/headline";
// User-Agent spoofing is required — Yahoo's CDN blocks default reqwest UA with 429.
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/120.0.0.0 Safari/537.36";

pub struct YahooClient {
    client: Client,
    /// When true, all methods immediately return NotFound (used in tests that mock FMP only).
    disabled: bool,
}

impl YahooClient {
    pub fn new(client: Client) -> Self {
        Self { client, disabled: false }
    }

    /// Creates a no-op client that always returns NotFound.
    /// Used in integration tests that mock only FMP endpoints — prices fall back to FMP mock.
    pub fn new_disabled() -> Self {
        Self { client: Client::new(), disabled: true }
    }

    /// Returns daily adjusted closing prices sorted newest-first, up to `limit` entries.
    pub async fn historical_prices(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<HistoricalPrice>, AppError> {
        if self.disabled { return Err(AppError::NotFound); }
        self.historical_prices_impl(ticker, limit).await
    }

    async fn historical_prices_impl(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<HistoricalPrice>, AppError> {
        let range = match limit {
            0..=65 => "3mo",
            66..=130 => "6mo",
            131..=260 => "1y",
            261..=520 => "2y",
            _ => "5y",
        };

        let resp = self
            .client
            .get(format!("{}/{}", CHART_BASE, ticker))
            .query(&[
                ("interval", "1d"),
                ("range", range),
                ("events", "history"),
                ("includePrePost", "false"),
            ])
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .json::<ChartResponse>()
            .await?;

        let result = resp
            .chart
            .result
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .ok_or(AppError::NotFound)?;

        let timestamps = result.timestamp.unwrap_or_default();
        let closes = result
            .indicators
            .adjclose
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .map(|ac| ac.adjclose)
            .unwrap_or_default();

        let mut prices: Vec<HistoricalPrice> = timestamps
            .into_iter()
            .zip(closes)
            .filter_map(|(ts, price)| {
                let date = Utc
                    .timestamp_opt(ts, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%d").to_string())?;
                Some(HistoricalPrice { date, price: Some(price) })
            })
            .collect();

        // Yahoo returns oldest-first; reverse to match FMP convention (newest-first)
        prices.reverse();
        prices.truncate(limit as usize);

        if prices.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(prices)
    }

    /// Returns the closing price on the given date, or the nearest prior trading day.
    /// Uses a targeted 7-day window so the request is small.
    pub async fn price_on_date(
        &self,
        ticker: &str,
        date: chrono::NaiveDate,
    ) -> Result<f64, AppError> {
        if self.disabled { return Err(AppError::NotFound); }

        use chrono::Duration;
        // Fetch from (date − 7d) through (date + 2d) to cover weekends and holidays.
        let period1 = (date - Duration::days(7))
            .and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        let period2 = (date + Duration::days(2))
            .and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();

        let resp = self
            .client
            .get(format!("{}/{}", CHART_BASE, ticker))
            .query(&[
                ("interval", "1d"),
                ("period1", period1.to_string().as_str()),
                ("period2", period2.to_string().as_str()),
                ("includePrePost", "false"),
            ])
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .json::<ChartResponse>()
            .await?;

        let result = resp
            .chart
            .result
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .ok_or(AppError::NotFound)?;

        let timestamps = result.timestamp.unwrap_or_default();
        let closes = result
            .indicators
            .adjclose
            .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .map(|ac| ac.adjclose)
            .unwrap_or_default();

        // Yahoo returns oldest-first. Take the last entry whose date ≤ target date.
        timestamps
            .into_iter()
            .zip(closes)
            .filter_map(|(ts, price)| {
                let ts_date = Utc.timestamp_opt(ts, 0).single()?.date_naive();
                if ts_date <= date { Some(price) } else { None }
            })
            .last()
            .ok_or(AppError::NotFound)
    }

    /// Returns the latest available closing price.
    pub async fn quote_price(&self, ticker: &str) -> Result<f64, AppError> {
        self.historical_prices(ticker, 1)
            .await?
            .into_iter()
            .next()
            .and_then(|p| p.price)
            .ok_or(AppError::NotFound)
    }

    /// Returns market caps (in USD) for a batch of tickers.
    /// Tickers with no market cap data are omitted from the result.
    pub async fn batch_market_caps(
        &self,
        tickers: &[String],
    ) -> std::collections::HashMap<String, u64> {
        if self.disabled || tickers.is_empty() {
            return std::collections::HashMap::new();
        }
        // Yahoo accepts up to ~1500 symbols in one request; we chunk at 200 to be safe.
        let mut result = std::collections::HashMap::new();
        for chunk in tickers.chunks(200) {
            let symbols = chunk.join(",");
            if let Ok(map) = self.batch_market_caps_chunk(&symbols).await {
                result.extend(map);
            }
        }
        result
    }

    async fn batch_market_caps_chunk(
        &self,
        symbols: &str,
    ) -> Result<std::collections::HashMap<String, u64>, crate::error::AppError> {
        let resp = self
            .client
            .get(QUOTE_URL)
            .query(&[("symbols", symbols), ("fields", "marketCap")])
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .json::<QuoteResponse>()
            .await?;

        let map = resp
            .quote_response
            .result
            .into_iter()
            .filter_map(|q| q.market_cap.map(|mc| (q.symbol, mc as u64)))
            .collect();

        Ok(map)
    }


    /// Returns recent news headlines for a ticker from the Yahoo Finance RSS feed.
    pub async fn news(&self, ticker: &str, limit: usize) -> Result<Vec<RssNewsItem>, AppError> {
        if self.disabled { return Err(AppError::NotFound); }
        let bytes = self
            .client
            .get(NEWS_RSS_BASE)
            .query(&[("s", ticker), ("region", "US"), ("lang", "en-US")])
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let channel = rss::Channel::read_from(std::io::BufReader::new(&bytes[..]))
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let items = channel
            .into_items()
            .into_iter()
            .take(limit)
            .map(|item| RssNewsItem {
                title: item.title().unwrap_or("").to_owned(),
                url: item.link().unwrap_or("").to_owned(),
                published: item
                    .pub_date()
                    .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
                    .map(|dt| dt.to_rfc3339()),
                source: item.source().and_then(|s| s.title()).map(|t| t.to_owned()),
                summary: item.description().map(|s| strip_html(s)),
            })
            .collect();

        Ok(items)
    }
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteResponse {
    quote_response: QuoteResult,
}

#[derive(Deserialize)]
struct QuoteResult {
    result: Vec<QuoteEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteEntry {
    symbol: String,
    market_cap: Option<f64>,
}

#[derive(Deserialize)]
struct ChartResponse {
    chart: Chart,
}

#[derive(Deserialize)]
struct Chart {
    result: Option<Vec<ChartResult>>,
}

#[derive(Deserialize)]
struct ChartResult {
    timestamp: Option<Vec<i64>>,
    indicators: Indicators,
}

#[derive(Deserialize)]
struct Indicators {
    adjclose: Option<Vec<AdjClose>>,
}

#[derive(Deserialize)]
struct AdjClose {
    adjclose: Vec<f64>,
}

// ── News RSS types ────────────────────────────────────────────────────────────

pub struct RssNewsItem {
    pub title: String,
    pub url: String,
    /// ISO 8601 string converted from RSS RFC 2822 pub date.
    pub published: Option<String>,
    pub source: Option<String>,
    pub summary: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Strips HTML tags and decodes common entities from an RSS description.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}
