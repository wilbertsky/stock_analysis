//! Providers — unified data source routing.
//!
//! EDGAR + Yahoo Finance is the default for all data.
//! FMP is only used when a user has stored their own personal API key —
//! the server-level FMP key is never used as a silent fallback.
//!
//! The public method signatures mirror FmpClient so routes need no changes.
//!
//! ## Field-level backfill
//!
//! Earlier versions of `ratios`/`income_statements`/etc. treated EDGAR as all-or-
//! nothing: if EDGAR returned *any* data for a ticker, that response was used as-is,
//! even when individual fields inside it were `null`. In practice EDGAR's XBRL
//! extraction is narrower than FMP's for some companies — e.g. `gross_profit` only
//! checks the single `GrossProfit` XBRL concept (many business models, like REITs or
//! services companies, never tag it at all), and `debt_to_equity_ratio` is computed
//! from `LongTermDebt` specifically rather than total debt. Verified live against five
//! small/mid-cap tickers: FMP had `debtToEquityRatio` and `grossProfit` for every one
//! that came back `null` from EDGAR.
//!
//! So each fundamentals method now checks whether the EDGAR response has any gaps in
//! the fields actually used elsewhere in this app, and — only when a gap exists and an
//! FMP client is available — fetches FMP's data once and fills just the missing fields
//! (matched by date), keeping EDGAR's values wherever EDGAR had them. When EDGAR's data
//! is already complete, no extra FMP call happens at all, preserving the EDGAR-primary
//! cost model described above.

use std::{collections::HashMap, sync::Arc};

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

/// Merges EDGAR rows with FMP rows matched by date, filling gaps via `merge`.
/// `merge` receives the owned EDGAR row plus the matching FMP row (if any) and decides
/// field-by-field which value to keep — EDGAR values should always take precedence
/// when present, this only fills genuine gaps.
fn backfill_from_fmp<T>(
    edgar_rows: Vec<T>,
    fmp_rows: &[T],
    date_of: impl Fn(&T) -> &str,
    merge: impl Fn(T, Option<&T>) -> T,
) -> Vec<T> {
    let fmp_by_date: HashMap<&str, &T> = fmp_rows.iter().map(|r| (date_of(r), r)).collect();
    edgar_rows
        .into_iter()
        .map(|e| {
            let matched = fmp_by_date.get(date_of(&e)).copied();
            merge(e, matched)
        })
        .collect()
}

fn income_statement_has_gap(r: &IncomeStatement) -> bool {
    r.revenue.is_none()
        || r.gross_profit.is_none()
        || r.net_income.is_none()
        || r.eps.is_none()
        || r.weighted_average_shs_out.is_none()
}

fn balance_sheet_has_gap(r: &BalanceSheet) -> bool {
    r.total_assets.is_none()
        || r.total_current_assets.is_none()
        || r.total_current_liabilities.is_none()
        || r.long_term_debt.is_none()
        || r.total_equity.is_none()
        || r.total_debt.is_none()
}

fn cash_flow_has_gap(r: &CashFlowStatement) -> bool {
    r.operating_cash_flow.is_none() || r.free_cash_flow.is_none()
}

fn ratio_has_gap(r: &Ratio) -> bool {
    r.book_value_per_share.is_none()
        || r.free_cash_flow_per_share.is_none()
        || r.dividend_yield_percentage.is_none()
        || r.dividend_payout_ratio.is_none()
        || r.debt_to_equity_ratio.is_none()
}

fn key_metrics_has_gap(r: &KeyMetrics) -> bool {
    r.return_on_invested_capital.is_none() || r.return_on_equity.is_none()
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

    // ── Fundamentals: EDGAR first, field-level FMP backfill if present ───────

    pub async fn income_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<IncomeStatement>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.income_statements(ticker, limit).await {
                return Ok(self.backfill_income_statements(ticker, data, limit).await);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.income_statements(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    async fn backfill_income_statements(
        &self,
        ticker: &str,
        edgar_data: Vec<IncomeStatement>,
        limit: u32,
    ) -> Vec<IncomeStatement> {
        let Some(fmp) = &self.fmp else { return edgar_data };
        if !edgar_data.iter().any(income_statement_has_gap) {
            return edgar_data;
        }
        let Ok(fmp_data) = fmp.income_statements(ticker, limit).await else { return edgar_data };
        backfill_from_fmp(edgar_data, &fmp_data, |r| r.date.as_str(), |e, f| IncomeStatement {
            date: e.date.clone(),
            revenue: e.revenue.or(f.and_then(|f| f.revenue)),
            gross_profit: e.gross_profit.or(f.and_then(|f| f.gross_profit)),
            net_income: e.net_income.or(f.and_then(|f| f.net_income)),
            eps: e.eps.or(f.and_then(|f| f.eps)),
            weighted_average_shs_out: e
                .weighted_average_shs_out
                .or(f.and_then(|f| f.weighted_average_shs_out)),
        })
    }

    pub async fn balance_sheets(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<BalanceSheet>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.balance_sheets(ticker, limit).await {
                return Ok(self.backfill_balance_sheets(ticker, data, limit).await);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.balance_sheets(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    async fn backfill_balance_sheets(
        &self,
        ticker: &str,
        edgar_data: Vec<BalanceSheet>,
        limit: u32,
    ) -> Vec<BalanceSheet> {
        let Some(fmp) = &self.fmp else { return edgar_data };
        if !edgar_data.iter().any(balance_sheet_has_gap) {
            return edgar_data;
        }
        let Ok(fmp_data) = fmp.balance_sheets(ticker, limit).await else { return edgar_data };
        backfill_from_fmp(edgar_data, &fmp_data, |r| r.date.as_str(), |e, f| BalanceSheet {
            date: e.date.clone(),
            total_assets: e.total_assets.or(f.and_then(|f| f.total_assets)),
            total_current_assets: e.total_current_assets.or(f.and_then(|f| f.total_current_assets)),
            total_current_liabilities: e
                .total_current_liabilities
                .or(f.and_then(|f| f.total_current_liabilities)),
            long_term_debt: e.long_term_debt.or(f.and_then(|f| f.long_term_debt)),
            total_equity: e.total_equity.or(f.and_then(|f| f.total_equity)),
            total_debt: e.total_debt.or(f.and_then(|f| f.total_debt)),
        })
    }

    pub async fn cash_flow_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<CashFlowStatement>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.cash_flow_statements(ticker, limit).await {
                return Ok(self.backfill_cash_flow_statements(ticker, data, limit).await);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.cash_flow_statements(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    async fn backfill_cash_flow_statements(
        &self,
        ticker: &str,
        edgar_data: Vec<CashFlowStatement>,
        limit: u32,
    ) -> Vec<CashFlowStatement> {
        let Some(fmp) = &self.fmp else { return edgar_data };
        if !edgar_data.iter().any(cash_flow_has_gap) {
            return edgar_data;
        }
        let Ok(fmp_data) = fmp.cash_flow_statements(ticker, limit).await else { return edgar_data };
        backfill_from_fmp(edgar_data, &fmp_data, |r| r.date.as_str(), |e, f| CashFlowStatement {
            date: e.date.clone(),
            operating_cash_flow: e.operating_cash_flow.or(f.and_then(|f| f.operating_cash_flow)),
            free_cash_flow: e.free_cash_flow.or(f.and_then(|f| f.free_cash_flow)),
            common_stock_issuance: e.common_stock_issuance.or(f.and_then(|f| f.common_stock_issuance)),
        })
    }

    /// Ratios include P/E for the most recent entry, computed from Yahoo current price ÷ EDGAR EPS.
    pub async fn ratios(&self, ticker: &str, limit: u32) -> Result<Vec<Ratio>, AppError> {
        if self.edgar.has_cik(ticker) {
            let current_price = self.yahoo.quote_price(ticker).await.ok();
            if let Ok(data) = self.edgar.ratios(ticker, current_price, limit).await {
                return Ok(self.backfill_ratios(ticker, data, limit).await);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.ratios(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    async fn backfill_ratios(&self, ticker: &str, edgar_data: Vec<Ratio>, limit: u32) -> Vec<Ratio> {
        let Some(fmp) = &self.fmp else { return edgar_data };
        if !edgar_data.iter().any(ratio_has_gap) {
            return edgar_data;
        }
        let Ok(fmp_data) = fmp.ratios(ticker, limit).await else { return edgar_data };
        backfill_from_fmp(edgar_data, &fmp_data, |r| r.date.as_str(), |e, f| Ratio {
            date: e.date.clone(),
            book_value_per_share: e.book_value_per_share.or(f.and_then(|f| f.book_value_per_share)),
            free_cash_flow_per_share: e
                .free_cash_flow_per_share
                .or(f.and_then(|f| f.free_cash_flow_per_share)),
            // EDGAR computes P/E itself from a live Yahoo price; never overwrite with
            // FMP's (different-instant) P/E, only fill if EDGAR had no price to compute it.
            price_to_earnings_ratio: e
                .price_to_earnings_ratio
                .or(f.and_then(|f| f.price_to_earnings_ratio)),
            dividend_yield_percentage: e
                .dividend_yield_percentage
                .or(f.and_then(|f| f.dividend_yield_percentage)),
            dividend_payout_ratio: e.dividend_payout_ratio.or(f.and_then(|f| f.dividend_payout_ratio)),
            dividend_per_share: e.dividend_per_share.or(f.and_then(|f| f.dividend_per_share)),
            debt_to_equity_ratio: e.debt_to_equity_ratio.or(f.and_then(|f| f.debt_to_equity_ratio)),
        })
    }

    pub async fn key_metrics(&self, ticker: &str, limit: u32) -> Result<Vec<KeyMetrics>, AppError> {
        if self.edgar.has_cik(ticker) {
            if let Ok(data) = self.edgar.key_metrics(ticker, limit).await {
                return Ok(self.backfill_key_metrics(ticker, data, limit).await);
            }
        }
        match &self.fmp {
            Some(fmp) => fmp.key_metrics(ticker, limit).await,
            None => Err(AppError::NotFound),
        }
    }

    async fn backfill_key_metrics(&self, ticker: &str, edgar_data: Vec<KeyMetrics>, limit: u32) -> Vec<KeyMetrics> {
        let Some(fmp) = &self.fmp else { return edgar_data };
        if !edgar_data.iter().any(key_metrics_has_gap) {
            return edgar_data;
        }
        let Ok(fmp_data) = fmp.key_metrics(ticker, limit).await else { return edgar_data };
        backfill_from_fmp(edgar_data, &fmp_data, |r| r.date.as_str(), |e, f| KeyMetrics {
            date: e.date.clone(),
            return_on_invested_capital: e
                .return_on_invested_capital
                .or(f.and_then(|f| f.return_on_invested_capital)),
            return_on_equity: e.return_on_equity.or(f.and_then(|f| f.return_on_equity)),
        })
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
        if let Some(fmp) = &self.fmp {
            if let Ok(p) = fmp.price_on_date(ticker, date).await { return Ok(p); }
        }
        self.yahoo.price_on_date(ticker, date).await
    }

    pub async fn quote_price(&self, ticker: &str) -> Result<f64, AppError> {
        if let Some(fmp) = &self.fmp {
            if let Ok(p) = fmp.quote_price(ticker).await { return Ok(p); }
        }
        self.yahoo.quote_price(ticker).await
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
