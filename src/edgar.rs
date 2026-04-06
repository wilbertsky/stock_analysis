//! SEC EDGAR client — fundamental financial data from XBRL filings.
//!
//! Uses the EDGAR XBRL company facts API which is free, public, and officially supported.
//! Covers all US public companies that file 10-K annual reports with the SEC.
//!
//! Data flow:
//!   1. Ticker → CIK resolved from the bulk SEC company_tickers.json file (loaded at startup).
//!   2. Company facts JSON fetched from data.sec.gov (one request per company, ~2-5s, ~5-20MB).
//!   3. Annual 10-K entries extracted from XBRL, parsed into typed `AnnualRow`s.
//!   4. Results cached in-memory for 30 minutes to avoid repeat fetches during screener calls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::error::AppError;
use crate::fmp::{BalanceSheet, CashFlowStatement, IncomeStatement, KeyMetrics, Ratio};

const FACTS_BASE: &str = "https://data.sec.gov/api/xbrl/companyfacts";
const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
// EDGAR API terms require a descriptive User-Agent with contact info.
const USER_AGENT: &str = "stock-terminal/1.0 (contact@example.com)";
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);

// ── Public types ──────────────────────────────────────────────────────────────

pub struct EdgarClient {
    client: Client,
    /// ticker.to_uppercase() → zero-padded 10-digit CIK ("0000320193")
    cik_map: HashMap<String, String>,
    /// ticker.to_uppercase() → title-cased company name ("Apple Inc")
    name_map: HashMap<String, String>,
    /// padded_cik → cached parsed facts
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// Tracks the last time an HTTP request was sent to EDGAR.
    /// Enforces a minimum 110 ms gap between requests (≤9 req/s, under EDGAR's 10/s limit).
    last_edgar_request: AsyncMutex<Instant>,
}

struct CacheEntry {
    facts: Arc<ParsedFacts>,
    expires_at: Instant,
}

/// All annual metrics extracted from a company's EDGAR XBRL company facts, newest-first.
pub struct ParsedFacts {
    pub rows: Vec<AnnualRow>,
}

/// One fiscal year of data aligned across all XBRL concepts.
#[derive(Default, Clone)]
pub struct AnnualRow {
    pub end_date: String,
    pub revenue: Option<f64>,
    pub gross_profit: Option<f64>,
    pub net_income: Option<f64>,
    pub eps_diluted: Option<f64>,
    pub shares_diluted: Option<f64>,
    pub operating_cash_flow: Option<f64>,
    /// Capital expenditures (positive; subtract from OCF for FCF)
    pub capex: Option<f64>,
    pub total_assets: Option<f64>,
    pub current_assets: Option<f64>,
    pub current_liabilities: Option<f64>,
    pub long_term_debt: Option<f64>,
    pub stockholders_equity: Option<f64>,
    pub dividend_per_share: Option<f64>,
    /// ProceedsFromIssuanceOfCommonStock — used by Piotroski F7 (dilution check)
    pub common_stock_issuance: Option<f64>,
}

// ── Client impl ───────────────────────────────────────────────────────────────

impl EdgarClient {
    /// Creates an empty client with no CIK map — all calls fall through to FMP.
    /// Used in integration tests that mock only FMP endpoints.
    pub fn new_empty(client: Client) -> Self {
        Self {
            client,
            cik_map: HashMap::new(),
            name_map: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
            last_edgar_request: AsyncMutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    /// Initializes the client and downloads the ticker→CIK mapping from EDGAR.
    /// Falls back to an empty map on failure — all subsequent calls fall through to FMP.
    pub async fn new(client: Client) -> Self {
        let (cik_map, name_map) = match load_cik_map(&client).await {
            Ok(maps) => {
                tracing::info!("EDGAR: loaded {} tickers into CIK map", maps.0.len());
                maps
            }
            Err(e) => {
                tracing::warn!(
                    "EDGAR: failed to load CIK map ({}). EDGAR source disabled, FMP fallback active.",
                    e
                );
                (HashMap::new(), HashMap::new())
            }
        };
        Self {
            client,
            cik_map,
            name_map,
            cache: Mutex::new(HashMap::new()),
            last_edgar_request: AsyncMutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    /// Returns true when this ticker has a known EDGAR CIK (US-listed company).
    pub fn has_cik(&self, ticker: &str) -> bool {
        self.cik_map.contains_key(&ticker.to_uppercase())
    }

    /// Searches the in-memory ticker/name map. No network call — instant.
    /// Priority: exact ticker match → ticker prefix → company name substring.
    pub fn search_local(&self, query: &str, limit: usize) -> Vec<crate::fmp::SearchResult> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return vec![];
        }

        let mut exact: Vec<crate::fmp::SearchResult> = Vec::new();
        let mut ticker_prefix: Vec<crate::fmp::SearchResult> = Vec::new();
        let mut name_match: Vec<crate::fmp::SearchResult> = Vec::new();

        for (ticker, name) in &self.name_map {
            let tl = ticker.to_ascii_lowercase();
            let nl = name.to_ascii_lowercase();
            let result = crate::fmp::SearchResult {
                symbol: ticker.clone(),
                name: name.clone(),
                stock_exchange: None,
                exchange_short_name: None,
            };
            if tl == q {
                exact.push(result);
            } else if tl.starts_with(&q) {
                ticker_prefix.push(result);
            } else if nl.contains(&q) {
                name_match.push(result);
            }
        }

        ticker_prefix.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        name_match.sort_by(|a, b| a.symbol.cmp(&b.symbol));

        exact
            .into_iter()
            .chain(ticker_prefix)
            .chain(name_match)
            .take(limit)
            .collect()
    }

    /// Fetches and parses EDGAR company facts, using the in-memory 30-minute cache.
    /// Multiple concurrent callers for the same ticker each get cache hits after the first.
    pub async fn fetch_parsed_facts(
        &self,
        ticker: &str,
    ) -> Result<Arc<ParsedFacts>, AppError> {
        let cik = self
            .cik_map
            .get(&ticker.to_uppercase())
            .cloned()
            .ok_or(AppError::NotFound)?;

        // Check cache — drop the lock before any await
        {
            let cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.get(&cik) {
                if entry.expires_at > Instant::now() {
                    tracing::debug!("EDGAR cache hit for CIK {}", cik);
                    return Ok(entry.facts.clone());
                }
            }
        }

        // Rate-limit: enforce ≥110 ms between outgoing EDGAR HTTP requests (≤9 req/s).
        // Holds the lock across the sleep so concurrent cache-miss calls queue up
        // rather than all firing at once.
        {
            let mut last = self.last_edgar_request.lock().await;
            let elapsed = last.elapsed();
            let min_gap = Duration::from_millis(110);
            if elapsed < min_gap {
                tokio::time::sleep(min_gap - elapsed).await;
            }
            *last = Instant::now();
        }

        tracing::debug!("EDGAR: fetching company facts for {} (CIK {})", ticker, cik);
        let url = format!("{}/CIK{}.json", FACTS_BASE, cik);
        let raw: Value = self
            .client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let facts = Arc::new(parse_company_facts(&raw));

        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(
                cik,
                CacheEntry {
                    facts: facts.clone(),
                    expires_at: Instant::now() + CACHE_TTL,
                },
            );
        }

        Ok(facts)
    }

    // ── FMP-compatible output methods ─────────────────────────────────────────

    pub async fn income_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<IncomeStatement>, AppError> {
        let facts = self.fetch_parsed_facts(ticker).await?;
        let rows: Vec<IncomeStatement> = facts
            .rows
            .iter()
            .take(limit as usize)
            .map(|r| IncomeStatement {
                date: r.end_date.clone(),
                revenue: r.revenue,
                gross_profit: r.gross_profit,
                net_income: r.net_income,
                eps: r.eps_diluted,
                weighted_average_shs_out: r.shares_diluted,
            })
            .collect();

        if rows.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(rows)
    }

    pub async fn balance_sheets(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<BalanceSheet>, AppError> {
        let facts = self.fetch_parsed_facts(ticker).await?;
        let rows: Vec<BalanceSheet> = facts
            .rows
            .iter()
            .take(limit as usize)
            .map(|r| BalanceSheet {
                date: r.end_date.clone(),
                total_assets: r.total_assets,
                total_current_assets: r.current_assets,
                total_current_liabilities: r.current_liabilities,
                long_term_debt: r.long_term_debt,
                total_equity: r.stockholders_equity,
                // Approximate total_debt as long-term debt (EDGAR doesn't separate short-term cleanly)
                total_debt: r.long_term_debt,
            })
            .collect();

        if rows.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(rows)
    }

    pub async fn cash_flow_statements(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<CashFlowStatement>, AppError> {
        let facts = self.fetch_parsed_facts(ticker).await?;
        let rows: Vec<CashFlowStatement> = facts
            .rows
            .iter()
            .take(limit as usize)
            .map(|r| {
                let free_cash_flow = match (r.operating_cash_flow, r.capex) {
                    (Some(ocf), Some(capex)) => Some(ocf - capex),
                    (Some(ocf), None) => Some(ocf),
                    _ => None,
                };
                CashFlowStatement {
                    date: r.end_date.clone(),
                    operating_cash_flow: r.operating_cash_flow,
                    free_cash_flow,
                    common_stock_issuance: r.common_stock_issuance,
                }
            })
            .collect();

        if rows.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(rows)
    }

    /// Returns per-share and valuation ratios.
    /// `current_price` is passed in from Yahoo for P/E and dividend yield on the most recent entry.
    pub async fn ratios(
        &self,
        ticker: &str,
        current_price: Option<f64>,
        limit: u32,
    ) -> Result<Vec<Ratio>, AppError> {
        let facts = self.fetch_parsed_facts(ticker).await?;
        let rows: Vec<Ratio> = facts
            .rows
            .iter()
            .enumerate()
            .take(limit as usize)
            .map(|(i, r)| {
                let shares = r.shares_diluted.filter(|&s| s > 0.0);

                let book_value_per_share = match (r.stockholders_equity, shares) {
                    (Some(eq), Some(sh)) => Some(eq / sh),
                    _ => None,
                };

                let fcf = match (r.operating_cash_flow, r.capex) {
                    (Some(ocf), Some(capex)) => Some(ocf - capex),
                    (Some(ocf), None) => Some(ocf),
                    _ => None,
                };
                let free_cash_flow_per_share = match (fcf, shares) {
                    (Some(f), Some(sh)) => Some(f / sh),
                    _ => None,
                };

                // P/E and yield only for the most recent entry, and only when price is available
                let (price_to_earnings_ratio, dividend_yield_percentage) = if i == 0 {
                    let pe = match (current_price, r.eps_diluted.filter(|&e| e > 0.0)) {
                        (Some(price), Some(eps)) => Some(price / eps),
                        _ => None,
                    };
                    let yld = match (r.dividend_per_share, current_price.filter(|&p| p > 0.0)) {
                        (Some(div), Some(price)) => Some(div / price * 100.0),
                        _ => None,
                    };
                    (pe, yld)
                } else {
                    (None, None)
                };

                let dividend_payout_ratio =
                    match (r.dividend_per_share, r.eps_diluted.filter(|&e| e > 0.0)) {
                        (Some(div), Some(eps)) => Some(div / eps),
                        _ => None,
                    };

                let debt_to_equity_ratio =
                    match (r.long_term_debt, r.stockholders_equity.filter(|&e| e > 0.0)) {
                        (Some(ltd), Some(eq)) => Some(ltd / eq),
                        _ => None,
                    };

                Ratio {
                    date: r.end_date.clone(),
                    book_value_per_share,
                    free_cash_flow_per_share,
                    price_to_earnings_ratio,
                    dividend_yield_percentage,
                    dividend_payout_ratio,
                    dividend_per_share: r.dividend_per_share,
                    debt_to_equity_ratio,
                }
            })
            .collect();

        if rows.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(rows)
    }

    pub async fn key_metrics(
        &self,
        ticker: &str,
        limit: u32,
    ) -> Result<Vec<KeyMetrics>, AppError> {
        let facts = self.fetch_parsed_facts(ticker).await?;
        let rows: Vec<KeyMetrics> = facts
            .rows
            .iter()
            .take(limit as usize)
            .map(|r| {
                let invested_capital = match (r.stockholders_equity, r.long_term_debt) {
                    (Some(eq), Some(ltd)) => Some(eq + ltd),
                    (Some(eq), None) => Some(eq),
                    _ => None,
                };
                let return_on_invested_capital =
                    match (r.net_income, invested_capital.filter(|&ic| ic > 0.0)) {
                        (Some(ni), Some(ic)) => Some(ni / ic),
                        _ => None,
                    };
                let return_on_equity =
                    match (r.net_income, r.stockholders_equity.filter(|&e| e > 0.0)) {
                        (Some(ni), Some(eq)) => Some(ni / eq),
                        _ => None,
                    };
                KeyMetrics {
                    date: r.end_date.clone(),
                    return_on_invested_capital,
                    return_on_equity,
                }
            })
            .collect();

        if rows.is_empty() {
            return Err(AppError::NotFound);
        }
        Ok(rows)
    }
}

// ── CIK map loading ───────────────────────────────────────────────────────────

fn to_title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    first.to_uppercase().collect::<String>()
                        + &chars.as_str().to_ascii_lowercase()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn load_cik_map(
    client: &Client,
) -> Result<(HashMap<String, String>, HashMap<String, String>), AppError> {
    #[derive(Deserialize)]
    struct Entry {
        cik_str: u64,
        ticker: String,
        title: String,
    }

    // The bulk file is a JSON object keyed by sequential index strings ("0", "1", ...)
    let raw: HashMap<String, Entry> = client
        .get(TICKERS_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut cik_map = HashMap::with_capacity(raw.len());
    let mut name_map = HashMap::with_capacity(raw.len());

    for e in raw.into_values() {
        let ticker = e.ticker.to_uppercase();
        let padded_cik = format!("{:010}", e.cik_str);
        name_map.insert(ticker.clone(), to_title_case(&e.title));
        cik_map.insert(ticker, padded_cik);
    }

    Ok((cik_map, name_map))
}

// ── XBRL parsing ─────────────────────────────────────────────────────────────

/// Parses a raw EDGAR company facts JSON value into aligned annual rows.
fn parse_company_facts(raw: &Value) -> ParsedFacts {
    let gaap = &raw["facts"]["us-gaap"];

    // Extract each metric from XBRL concepts, trying fallback concept names in order
    let revenue = extract_annual_usd(
        gaap,
        &[
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "Revenues",
            "SalesRevenueNet",
            "SalesRevenueGoodsNet",
            "RevenueFromContractWithCustomer",
        ],
    );
    let gross_profit = extract_annual_usd(gaap, &["GrossProfit"]);
    let net_income = extract_annual_usd(
        gaap,
        &["NetIncomeLoss", "NetIncomeLossAvailableToCommonStockholdersBasic"],
    );
    let eps_diluted = extract_annual_units(gaap, "USD/shares", &["EarningsPerShareDiluted"]);
    let shares_diluted = extract_annual_units(
        gaap,
        "shares",
        &[
            "WeightedAverageNumberOfDilutedSharesOutstanding",
            "WeightedAverageNumberOfSharesOutstandingBasic",
            "CommonStockSharesOutstanding",
        ],
    );
    let ocf = extract_annual_usd(gaap, &["NetCashProvidedByUsedInOperatingActivities"]);
    let capex = extract_annual_usd(
        gaap,
        &[
            "PaymentsToAcquirePropertyPlantAndEquipment",
            "PaymentsToAcquireProductiveAssets",
        ],
    );
    let total_assets = extract_annual_usd(gaap, &["Assets"]);
    let current_assets = extract_annual_usd(gaap, &["AssetsCurrent"]);
    let current_liabilities = extract_annual_usd(gaap, &["LiabilitiesCurrent"]);
    let long_term_debt = extract_annual_usd(
        gaap,
        &["LongTermDebt", "LongTermDebtNoncurrent", "LongTermNotesPayable"],
    );
    let stockholders_equity = extract_annual_usd(
        gaap,
        &[
            "StockholdersEquity",
            "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        ],
    );
    let dividend_per_share = extract_annual_units(
        gaap,
        "USD/shares",
        &[
            "CommonStockDividendsPerShareCashPaid",
            "CommonStockDividendsPerShareDeclared",
        ],
    );
    let common_stock_issuance =
        extract_annual_usd(gaap, &["ProceedsFromIssuanceOfCommonStock"]);

    // Extract anchor dates from the most-populated primary metric before consuming the vecs
    let anchor_dates: Vec<String> = {
        let anchor = [&revenue, &net_income, &total_assets]
            .iter()
            .max_by_key(|v| v.len())
            .copied()
            .unwrap_or(&revenue);

        if anchor.is_empty() {
            return ParsedFacts { rows: vec![] };
        }
        anchor.iter().map(|(d, _)| d.clone()).collect()
    };

    // Convert all metrics to end_date→val maps
    let to_map = |rows: Vec<(String, f64)>| -> HashMap<String, f64> { rows.into_iter().collect() };

    let rev_map = to_map(revenue);
    let gp_map = to_map(gross_profit);
    let ni_map = to_map(net_income);
    let eps_map = to_map(eps_diluted);
    let sh_map = to_map(shares_diluted);
    let ocf_map = to_map(ocf);
    let capex_map = to_map(capex);
    let assets_map = to_map(total_assets);
    let ca_map = to_map(current_assets);
    let cl_map = to_map(current_liabilities);
    let ltd_map = to_map(long_term_debt);
    let eq_map = to_map(stockholders_equity);
    let div_map = to_map(dividend_per_share);
    let iss_map = to_map(common_stock_issuance);

    let rows: Vec<AnnualRow> = anchor_dates
        .iter()
        .map(|date| AnnualRow {
            end_date: date.clone(),
            revenue: rev_map.get(date).copied(),
            gross_profit: gp_map.get(date).copied(),
            net_income: ni_map.get(date).copied(),
            eps_diluted: eps_map.get(date).copied(),
            shares_diluted: sh_map.get(date).copied(),
            operating_cash_flow: ocf_map.get(date).copied(),
            capex: capex_map.get(date).copied(),
            total_assets: assets_map.get(date).copied(),
            current_assets: ca_map.get(date).copied(),
            current_liabilities: cl_map.get(date).copied(),
            long_term_debt: ltd_map.get(date).copied(),
            stockholders_equity: eq_map.get(date).copied(),
            dividend_per_share: div_map.get(date).copied(),
            common_stock_issuance: iss_map.get(date).copied(),
        })
        .collect();

    ParsedFacts { rows }
}

/// Extracts annual USD entries for the first concept that has data.
fn extract_annual_usd(gaap: &Value, concepts: &[&str]) -> Vec<(String, f64)> {
    extract_annual_units(gaap, "USD", concepts)
}

/// Extracts annual entries for a given unit ("USD", "USD/shares", "shares") and concept list.
/// Tries each concept in order, returning the first with non-empty annual 10-K data.
fn extract_annual_units(gaap: &Value, unit: &str, concepts: &[&str]) -> Vec<(String, f64)> {
    for &concept in concepts {
        let path = &gaap[concept]["units"][unit];
        if let Some(entries) = path.as_array() {
            let result = collect_annual_entries(entries);
            if !result.is_empty() {
                return result;
            }
        }
    }
    vec![]
}

/// Filters XBRL entries to annual 10-K filings, deduplicates by end date
/// (latest filed wins), and returns `(end_date, val)` sorted newest-first.
fn collect_annual_entries(entries: &[Value]) -> Vec<(String, f64)> {
    // end_date → (filed_date, val): keep the most recently filed entry per fiscal year end
    let mut by_end: HashMap<String, (String, f64)> = HashMap::new();

    for entry in entries {
        let form = entry["form"].as_str().unwrap_or("");
        let fp = entry["fp"].as_str().unwrap_or("");

        // 10-K and 10-K/A are annual reports; fp=="FY" ensures the full-year figure
        if !matches!(form, "10-K" | "10-K/A") || fp != "FY" {
            continue;
        }

        let end = match entry["end"].as_str() {
            Some(e) => e.to_owned(),
            None => continue,
        };
        let filed = entry["filed"].as_str().unwrap_or("").to_owned();
        let val = match entry["val"].as_f64() {
            Some(v) => v,
            None => continue,
        };

        by_end
            .entry(end)
            .and_modify(|existing| {
                if filed > existing.0 {
                    *existing = (filed.clone(), val);
                }
            })
            .or_insert((filed, val));
    }

    let mut result: Vec<(String, f64)> = by_end
        .into_iter()
        .map(|(end, (_, val))| (end, val))
        .collect();
    result.sort_by(|a, b| b.0.cmp(&a.0)); // newest-first
    result
}
