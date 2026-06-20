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

    /// Construct from a shared reqwest Client (avoids spinning up a new connection pool
    /// for each per-user client instance). Used for user-keyed authenticated requests.
    pub fn with_shared_client(client: Client, api_key: String, base_url: String) -> Self {
        Self { client, api_key, base_url }
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

    /// Search for tickers and company names.
    /// Calls both search-symbol and search-name in parallel, merges and deduplicates by symbol.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>, AppError> {
        let symbol_url = format!("{}/search-symbol", self.base_url);
        let name_url = format!("{}/search-name", self.base_url);

        let (symbol_res, name_res) = tokio::join!(
            self.client
                .get(&symbol_url)
                .query(&[("query", query), ("limit", &limit.to_string()), ("apikey", &self.api_key)])
                .send(),
            self.client
                .get(&name_url)
                .query(&[("query", query), ("limit", &limit.to_string()), ("apikey", &self.api_key)])
                .send(),
        );

        let mut seen = std::collections::HashSet::new();
        let mut results: Vec<SearchResult> = Vec::new();

        // Symbol matches first (more relevant for ticker searches)
        if let Ok(resp) = symbol_res {
            if let Ok(list) = resp.json::<Vec<SearchResult>>().await {
                for r in list {
                    if seen.insert(r.symbol.clone()) {
                        results.push(r);
                    }
                }
            }
        }

        // Then name matches (fills in company-name searches)
        if let Ok(resp) = name_res {
            if let Ok(list) = resp.json::<Vec<SearchResult>>().await {
                for r in list {
                    if seen.insert(r.symbol.clone()) {
                        results.push(r);
                    }
                }
            }
        }

        results.truncate(limit as usize);
        Ok(results)
    }

    /// Fetches daily closing prices, newest-first. limit=260 ≈ 1 trading year.
    pub async fn historical_prices(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<HistoricalPrice>, AppError> {
        self.fetch_list(&format!("{}/historical-price-eod/light", self.base_url), ticker, limit).await
    }

    /// Screens the full market by market-cap band and (optionally) sector via
    /// /stable/company-screener. Used to source both the large-cap screener universe
    /// (sp500.rs) and the small/mid-cap discovery universe (routes/discovery.rs) —
    /// FMP's own screener replaces the dead GitHub-hosted S&P 500/Nasdaq 100 constituent
    /// feeds and reaches well beyond index membership.
    ///
    /// Always scoped to `country=US` and `isActivelyTrading=true`; ETFs and funds are
    /// filtered out client-side since filtering them server-side via query params is
    /// unverified against the current plan.
    pub async fn company_screener(
        &self,
        market_cap_more_than: u64,
        market_cap_lower_than: Option<u64>,
        sector: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ScreenerCandidate>, AppError> {
        let mut query: Vec<(&str, String)> = vec![
            ("marketCapMoreThan", market_cap_more_than.to_string()),
            ("country", "US".to_owned()),
            ("isActivelyTrading", "true".to_owned()),
            ("limit", limit.to_string()),
            ("apikey", self.api_key.clone()),
        ];
        if let Some(cap) = market_cap_lower_than {
            query.push(("marketCapLowerThan", cap.to_string()));
        }
        if let Some(s) = sector {
            query.push(("sector", s.to_owned()));
        }

        let url = format!("{}/company-screener", self.base_url);
        let list: Vec<ScreenerCandidate> = self
            .client
            .get(&url)
            .query(&query)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(list
            .into_iter()
            .filter(|c| !c.is_etf && !c.is_fund && c.is_actively_trading)
            .collect())
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

    /// Fetches company profile (description, sector, industry, website, employees) from FMP.
    pub async fn company_profile(&self, ticker: &str) -> Result<FmpProfile, AppError> {
        let url = format!("{}/profile", self.base_url);
        let profiles: Vec<FmpProfile> = self
            .client
            .get(&url)
            .query(&[("symbol", ticker), ("apikey", &self.api_key)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        profiles.into_iter().next().ok_or(AppError::NotFound)
    }

    /// Returns the current bid/ask mid-price for a single ticker.
    pub async fn quote_price(&self, ticker: &str) -> Result<f64, AppError> {
        #[derive(Deserialize)]
        struct Quote {
            price: Option<f64>,
        }
        let url = format!("{}/quote-short", self.base_url);
        let quotes: Vec<Quote> = self
            .client
            .get(&url)
            .query(&[("symbol", ticker), ("apikey", &self.api_key)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        quotes
            .into_iter()
            .next()
            .and_then(|q| q.price)
            .ok_or(AppError::NotFound)
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

/// From /stable/search-symbol and /stable/search-name — ticker/company search result.
/// FMP response fields: symbol, name, currency, exchangeFullName, exchange
#[derive(Debug, Deserialize, Clone)]
pub struct SearchResult {
    pub symbol: String,
    pub name: String,
    #[serde(default, rename = "exchangeFullName")] pub stock_exchange: Option<String>,
    #[serde(default, rename = "exchange")] pub exchange_short_name: Option<String>,
}

/// From /stable/profile — company description, sector, industry, website, employee count.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FmpProfile {
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub sector: Option<String>,
    #[serde(default)] pub industry: Option<String>,
    #[serde(default)] pub website: Option<String>,
    /// FMP returns this as a string (e.g. "150000")
    #[serde(default, rename = "fullTimeEmployees", deserialize_with = "de_employees")]
    pub full_time_employees: Option<i64>,
}

/// From /stable/company-screener — market-cap/sector-filtered company list.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScreenerCandidate {
    pub symbol: String,
    pub company_name: String,
    pub market_cap: f64,
    #[serde(default)] pub sector: Option<String>,
    #[serde(default)] pub industry: Option<String>,
    #[serde(default)] pub exchange_short_name: Option<String>,
    #[serde(default)] pub price: Option<f64>,
    #[serde(default)] pub is_etf: bool,
    #[serde(default)] pub is_fund: bool,
    #[serde(default)] pub is_actively_trading: bool,
}

fn de_employees<'de, D>(de: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = Option::<serde_json::Value>::deserialize(de)?;
    Ok(match v {
        None => None,
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.replace(',', "").parse().ok(),
        _ => None,
    })
}
