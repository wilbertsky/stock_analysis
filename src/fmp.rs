use reqwest::Client;
use serde::Deserialize;
use crate::error::AppError;

const BASE_URL: &str = "https://financialmodelingprep.com/stable";

pub struct FmpClient {
    client: Client,
    api_key: String,
}

impl FmpClient {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("Failed to build HTTP client"),
            api_key,
        }
    }

    pub async fn income_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<IncomeStatement>, AppError> {
        self.fetch_list(&format!("{BASE_URL}/income-statement"), ticker, limit).await
    }

    /// Returns empty vec instead of NotFound — BVPS/FCF/PE may not be available on all plans.
    pub async fn ratios(&self, ticker: &str, limit: u32) -> Result<Vec<Ratio>, AppError> {
        self.fetch_list_or_empty(&format!("{BASE_URL}/ratios"), ticker, limit).await
    }

    /// Returns empty vec instead of NotFound — ROIC may not be available on all plans.
    pub async fn key_metrics(&self, ticker: &str, limit: u32) -> Result<Vec<KeyMetrics>, AppError> {
        self.fetch_list_or_empty(&format!("{BASE_URL}/key-metrics"), ticker, limit).await
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

    /// Same as fetch_list but returns Ok(vec![]) on empty instead of NotFound.
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

// ── FMP deserialization types (internal) ────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomeStatement {
    pub date: String,
    #[serde(default)]
    pub revenue: Option<f64>,
    #[serde(default)]
    pub eps: Option<f64>,
}

/// From /stable/ratios — has per-share values and P/E.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ratio {
    pub date: String,
    #[serde(default)]
    pub book_value_per_share: Option<f64>,
    #[serde(default)]
    pub free_cash_flow_per_share: Option<f64>,
    #[serde(default)]
    pub price_to_earnings_ratio: Option<f64>,
}

/// From /stable/key-metrics — has ROIC.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyMetrics {
    pub date: String,
    #[serde(default)]
    pub return_on_invested_capital: Option<f64>,
}
