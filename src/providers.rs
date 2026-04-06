//! Providers — unified data source routing.
//!
//! EDGAR + Yahoo Finance is the default for all data.
//! FMP is only used when a user has stored their own personal API key —
//! the server-level FMP key is never used as a silent fallback.
//!
//! The public method signatures mirror FmpClient so routes need no changes.

use std::sync::Arc;

use crate::{
    edgar::EdgarClient,
    error::AppError,
    fmp::{BalanceSheet, CashFlowStatement, FmpClient, HistoricalPrice, IncomeStatement, KeyMetrics, Ratio, SearchResult},
    yahoo::{RssNewsItem, YahooClient},
};

#[derive(Clone)]
pub struct Providers {
    edgar: Arc<EdgarClient>,
    yahoo: Arc<YahooClient>,
    /// Only `Some` when the current user has stored their own FMP API key.
    /// The server-level FMP key is intentionally NOT set here.
    fmp: Option<Arc<FmpClient>>,
}

impl Providers {
    /// Creates a Providers with EDGAR + Yahoo only. No FMP fallback.
    pub fn new(edgar: Arc<EdgarClient>, yahoo: Arc<YahooClient>) -> Self {
        Self { edgar, yahoo, fmp: None }
    }

    /// Returns a Providers that adds a user-supplied FMP client as fallback.
    /// EDGAR and Yahoo clients are shared from the original instance.
    pub fn with_fmp(&self, fmp: Arc<FmpClient>) -> Self {
        Self {
            edgar: self.edgar.clone(),
            yahoo: self.yahoo.clone(),
            fmp: Some(fmp),
        }
    }

    // ── Fundamentals: EDGAR first, user FMP fallback if present ──────────────

    pub async fn income_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<IncomeStatement>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.income_statements(ticker, limit).await {
                return Ok(data);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.income_statements(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    pub async fn balance_sheets(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<BalanceSheet>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.balance_sheets(ticker, limit).await {
                return Ok(data);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.balance_sheets(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    pub async fn cash_flow_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<CashFlowStatement>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.cash_flow_statements(ticker, limit).await {
                return Ok(data);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.cash_flow_statements(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    /// Ratios include P/E for the most recent entry, computed from Yahoo current price ÷ EDGAR EPS.
    pub async fn ratios(&self, ticker: &str, limit: u32) -> Result<Vec<Ratio>, AppError> {
        if self.edgar.has_cik(ticker) {
            let current_price = self.yahoo.quote_price(ticker).await.ok();
            if let Ok(data) = self.edgar.ratios(ticker, current_price, limit).await {
                return Ok(data);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.ratios(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    pub async fn key_metrics(&self, ticker: &str, limit: u32) -> Result<Vec<KeyMetrics>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.key_metrics(ticker, limit).await {
                return Ok(data);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.key_metrics(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    // ── Prices: Yahoo first, user FMP fallback if present ────────────────────

    pub async fn historical_prices(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<HistoricalPrice>, AppError> {
        match self.yahoo.historical_prices(ticker, limit).await {
            Ok(data) => Ok(data),
            Err(e) => match &self.fmp {
                Some(fmp) => fmp.historical_prices(ticker, limit).await,
                None => Err(e),
            },
        }
    }

    /// Returns the closing price on the given date (or nearest prior trading day).
    pub async fn price_on_date(
        &self,
        ticker: &str,
        date: chrono::NaiveDate,
    ) -> Result<f64, AppError> {
        // Yahoo has decades of daily history; no FMP fallback needed.
        self.yahoo.price_on_date(ticker, date).await
    }

    pub async fn quote_price(&self, ticker: &str) -> Result<f64, AppError> {
        match self.yahoo.quote_price(ticker).await {
            Ok(p) => Ok(p),
            Err(e) => match &self.fmp {
                Some(fmp) => fmp.quote_price(ticker).await,
                None => Err(e),
            },
        }
    }

    /// Returns market caps (USD) for a batch of tickers, best-effort.
    pub async fn batch_market_caps(
        &self,
        tickers: &[String],
    ) -> std::collections::HashMap<String, u64> {
        self.yahoo.batch_market_caps(tickers).await
    }

    // ── News: Yahoo RSS ───────────────────────────────────────────────────────

    pub async fn company_news(&self, ticker: &str, limit: usize) -> Result<Vec<RssNewsItem>, AppError> {
        self.yahoo.news(ticker, limit).await
    }

    // ── Search: EDGAR local first, user FMP fallback if needed ──────────────

    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>, AppError> {
        let results = self.edgar.search_local(query, limit as usize);
        if !results.is_empty() {
            return Ok(results);
        }
        // EDGAR map not loaded or no match — fall back to user FMP if available
        match &self.fmp {
            Some(fmp) => fmp.search(query, limit).await,
            None => Ok(vec![]),
        }
    }
}
