use reqwest::Client;
use serde::Deserialize;
use crate::error::AppError;

const DEFAULT_BASE_URL: &str = "https://financialmodelingprep.com/stable";

pub struct FmpClient {
    client: Client,
    api_key: String,
    base_url: String,
}

impl FmpClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL.to_owned())
    }

    /// Alternate constructor — primarily for tests that point at a mock HTTP server.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("Failed to build HTTP client"),
            api_key,
            base_url,
        }
    }

    pub async fn income_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<IncomeStatement>, AppError> {
        self.fetch_list(&format!("{}/income-statement", self.base_url), ticker, limit).await
    }

    pub async fn balance_sheets(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<BalanceSheet>, AppError> {
        self.fetch_list(&format!("{}/balance-sheet-statement", self.base_url), ticker, limit).await
    }

    pub async fn cash_flow_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<CashFlowStatement>, AppError> {
        self.fetch_list(&format!("{}/cash-flow-statement", self.base_url), ticker, limit).await
    }

    /// Returns empty vec instead of NotFound — supplementary data may be absent on some plans.
    pub async fn ratios(&self, ticker: &str, limit: u32) -> Result<Vec<Ratio>, AppError> {
        self.fetch_list_or_empty(&format!("{}/ratios", self.base_url), ticker, limit).await
    }

    /// Returns empty vec instead of NotFound — ROIC/ROE may be absent on some plans.
    pub async fn key_metrics(&self, ticker: &str, limit: u32) -> Result<Vec<KeyMetrics>, AppError> {
        self.fetch_list_or_empty(&format!("{}/key-metrics", self.base_url), ticker, limit).await
    }

    /// Fetches daily closing prices, newest-first. limit=260 ≈ 1 trading year.
    pub async fn historical_prices(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<HistoricalPrice>, AppError> {
        self.fetch_list(&format!("{}/historical-price-eod/light", self.base_url), ticker, limit).await
    }

    async fn fetch_list<T>(&self, url: &str, ticker: &str, limit: u32) -> Result<Vec<T>, AppError>
    where
        T: serde::de::DeserializeOwned,
    {
        let list: Vec<T> = self
            .client
            .get(url)
            .query(&[
                ("symbol", ticker),
                ("period", "annual"),
                ("limit", &limit.to_string()),
                ("apikey", &self.api_key),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if list.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(list)
    }

    async fn fetch_list_or_empty<T>(&self, url: &str, ticker: &str, limit: u32) -> Result<Vec<T>, AppError>
    where
        T: serde::de::DeserializeOwned,
    {
        match self.fetch_list(url, ticker, limit).await {
            Err(AppError::NotFound) => Ok(vec![]),
            result => result,
        }
    }
}

// ── FMP deserialization types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeStatement {
    pub date: String,
    #[serde(default)] pub revenue: Option<f64>,
    #[serde(default)] pub gross_profit: Option<f64>,
    #[serde(default)] pub net_income: Option<f64>,
    #[serde(default)] pub eps: Option<f64>,
    #[serde(default)] pub weighted_average_shs_out: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSheet {
    pub date: String,
    #[serde(default)] pub total_assets: Option<f64>,
    #[serde(default)] pub total_current_assets: Option<f64>,
    #[serde(default)] pub total_current_liabilities: Option<f64>,
    #[serde(default)] pub long_term_debt: Option<f64>,
    #[serde(default)] pub total_equity: Option<f64>,
    #[serde(default)] pub total_debt: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CashFlowStatement {
    pub date: String,
    #[serde(default)] pub operating_cash_flow: Option<f64>,
    #[serde(default)] pub free_cash_flow: Option<f64>,
    #[serde(default)] pub common_stock_issuance: Option<f64>,
}

/// From /stable/ratios — per-share values, P/E, and dividend metrics.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ratio {
    pub date: String,
    #[serde(default)] pub book_value_per_share: Option<f64>,
    #[serde(default)] pub free_cash_flow_per_share: Option<f64>,
    #[serde(default)] pub price_to_earnings_ratio: Option<f64>,
    #[serde(default)] pub dividend_yield_percentage: Option<f64>,
    #[serde(default)] pub dividend_payout_ratio: Option<f64>,
    #[serde(default)] pub dividend_per_share: Option<f64>,
    #[serde(default)] pub debt_to_equity_ratio: Option<f64>,
}

/// From /stable/key-metrics — ROIC and ROE.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyMetrics {
    pub date: String,
    #[serde(default)] pub return_on_invested_capital: Option<f64>,
    #[serde(default)] pub return_on_equity: Option<f64>,
}

/// From /stable/historical-price-eod/light — daily closing prices, newest-first.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalPrice {
    pub date: String,
    #[serde(default)] pub price: Option<f64>,
}
