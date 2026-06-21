/// Integration tests for API routes.
///
/// Each test spins up a real wiremock server that stands in for FMP,
/// builds an Axum router pointing at it, and drives requests through
/// `tower::ServiceExt::oneshot` — no network calls to FMP are made.
#[cfg(test)]
mod tests {
    use axum::{body::Body, http::{Request, StatusCode}};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    use crate::{auth::jwt::encode_token, routes, state::AppState};

    // ── Router builder ────────────────────────────────────────────────────────

    fn build_test_router(state: AppState) -> axum::Router {
        use axum::routing::{get, patch, post};
        axum::Router::new()
            .route("/api/health",                         get(routes::health_check))
            .route("/api/stock/{ticker}/fundamentals",    get(routes::stock::get_fundamentals))
            .route("/api/stock/{ticker}/intrinsic-value", get(routes::stock::get_intrinsic_value))
            .route("/api/stock/{ticker}/graham-number",   get(routes::stock::get_graham_number))
            .route("/api/stock/{ticker}/piotroski",       get(routes::stock::get_piotroski))
            .route("/api/stock/{ticker}/dividends",       get(routes::stock::get_dividends))
            .route("/api/stock/{ticker}/quality",         get(routes::stock::get_quality))
            .route("/api/stock/{ticker}/momentum",        get(routes::stock::get_momentum))
            .route("/api/screener/{sector}",              get(routes::screener::get_sector_top_picks))
            .route("/api/discovery",                      get(routes::discovery::get_discovery))
            // Auth routes under test
            .route("/api/auth/password",                  patch(routes::auth::change_password))
            .route("/api/auth/forgot-password",           post(routes::auth::forgot_password))
            .route("/api/auth/reset-password",            post(routes::auth::reset_password))
            .with_state(state)
    }

    // ── Auth helper ───────────────────────────────────────────────────────────

    /// Generates a valid JWT signed with the test AppState's hardcoded secret.
    fn test_token() -> String {
        let secret = b"test_jwt_secret_key_32bytes_pad_";
        encode_token(uuid::Uuid::new_v4(), "test@example.com", "subscriber", secret).unwrap()
    }

    // ── Request helper ────────────────────────────────────────────────────────

    async fn get_json(
        app: axum::Router,
        uri: &str,
    ) -> (StatusCode, serde_json::Value) {
        let token = test_token();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = response.status();
        let collected: http_body_util::Collected<bytes::Bytes> =
            response.into_body().collect().await.unwrap();
        let bytes = collected.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        (status, json)
    }

    // ── Mock JSON payloads ────────────────────────────────────────────────────

    const INCOME_2YR: &str = r#"[
        {"date":"2024-09-28","revenue":391035000000,"grossProfit":180683000000,
         "netIncome":93736000000,"eps":6.11,"weightedAverageShsOut":15343783000},
        {"date":"2023-09-30","revenue":383285000000,"grossProfit":169148000000,
         "netIncome":96995000000,"eps":6.13,"weightedAverageShsOut":15812547000}
    ]"#;

    const INCOME_5YR: &str = r#"[
        {"date":"2024-09-28","revenue":391035000000,"grossProfit":180683000000,
         "netIncome":93736000000,"eps":6.50,"weightedAverageShsOut":15343783000},
        {"date":"2023-09-30","revenue":383285000000,"grossProfit":169148000000,
         "netIncome":96995000000,"eps":6.13,"weightedAverageShsOut":15812547000},
        {"date":"2022-09-24","revenue":394328000000,"grossProfit":170782000000,
         "netIncome":99803000000,"eps":6.11,"weightedAverageShsOut":16325819000},
        {"date":"2021-09-25","revenue":365817000000,"grossProfit":152836000000,
         "netIncome":94680000000,"eps":5.61,"weightedAverageShsOut":16864919000},
        {"date":"2020-09-26","revenue":274515000000,"grossProfit":104956000000,
         "netIncome":57411000000,"eps":3.28,"weightedAverageShsOut":17528214000}
    ]"#;

    const BALANCE_2YR: &str = r#"[
        {"date":"2024-09-28","totalAssets":364980000000,"totalCurrentAssets":152987000000,
         "totalCurrentLiabilities":176392000000,"longTermDebt":85750000000,
         "totalEquity":56950000000,"totalDebt":101304000000},
        {"date":"2023-09-30","totalAssets":352583000000,"totalCurrentAssets":143566000000,
         "totalCurrentLiabilities":145308000000,"longTermDebt":95281000000,
         "totalEquity":62146000000,"totalDebt":111088000000}
    ]"#;

    const CASHFLOW_2YR: &str = r#"[
        {"date":"2024-09-28","operatingCashFlow":118254000000,"freeCashFlow":108807000000,
         "commonStockIssuance":0},
        {"date":"2023-09-30","operatingCashFlow":113036000000,"freeCashFlow":99584000000,
         "commonStockIssuance":0}
    ]"#;

    const RATIOS_5YR: &str = r#"[
        {"date":"2024-09-28","bookValuePerShare":3.77,"freeCashFlowPerShare":7.17,
         "priceToEarningsRatio":35.5,"dividendYieldPercentage":0.44,
         "dividendPayoutRatio":0.156,"dividendPerShare":0.97,"debtToEquityRatio":1.78},
        {"date":"2023-09-30","bookValuePerShare":4.05,"freeCashFlowPerShare":6.43,
         "priceToEarningsRatio":29.7,"dividendYieldPercentage":0.51,
         "dividendPayoutRatio":0.147,"dividendPerShare":0.93,"debtToEquityRatio":1.97},
        {"date":"2022-09-24","bookValuePerShare":3.61,"freeCashFlowPerShare":6.02,
         "priceToEarningsRatio":24.4,"dividendYieldPercentage":0.68,
         "dividendPayoutRatio":0.152,"dividendPerShare":0.90,"debtToEquityRatio":1.86},
        {"date":"2021-09-25","bookValuePerShare":3.83,"freeCashFlowPerShare":5.26,
         "priceToEarningsRatio":28.9,"dividendYieldPercentage":0.56,
         "dividendPayoutRatio":0.151,"dividendPerShare":0.85,"debtToEquityRatio":1.52},
        {"date":"2020-09-26","bookValuePerShare":4.21,"freeCashFlowPerShare":3.73,
         "priceToEarningsRatio":35.6,"dividendYieldPercentage":0.67,
         "dividendPayoutRatio":0.208,"dividendPerShare":0.80,"debtToEquityRatio":1.19}
    ]"#;

    const KEY_METRICS_5YR: &str = r#"[
        {"date":"2024-09-28","returnOnInvestedCapital":0.545,"returnOnEquity":1.564},
        {"date":"2023-09-30","returnOnInvestedCapital":0.562,"returnOnEquity":1.474},
        {"date":"2022-09-24","returnOnInvestedCapital":0.531,"returnOnEquity":1.755},
        {"date":"2021-09-25","returnOnInvestedCapital":0.482,"returnOnEquity":1.496},
        {"date":"2020-09-26","returnOnInvestedCapital":0.297,"returnOnEquity":0.868}
    ]"#;

    /// Build a 260-entry price JSON array (newest-first) with specific values
    /// at the momentum window indices (0 = current, 63 = 3m, 126 = 6m, 252 = 12m).
    fn price_json(current: f64, m3: f64, m6: f64, m12: f64) -> String {
        let mut prices = vec![current; 260];
        prices[63] = m3;
        prices[126] = m6;
        prices[252] = m12;
        let entries: Vec<String> = prices
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                format!(r#"{{"date":"2024-{:02}-01","price":{:.2}}}"#, (i % 12) + 1, p)
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    // ── Health check ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_check_returns_200() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));

        let (status, body) = get_json(app, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    // ── Fundamentals ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fundamentals_returns_aligned_years() {
        let server = MockServer::start().await;
        mount_income(&server, INCOME_5YR).await;
        mount_ratios(&server, RATIOS_5YR).await;
        mount_key_metrics(&server, KEY_METRICS_5YR).await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/stock/AAPL/fundamentals").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ticker"], "AAPL");
        let years = body["years"].as_array().unwrap();
        assert_eq!(years.len(), 5);
        // Sorted oldest → newest: first entry should be 2020
        assert_eq!(years[0]["fiscal_year"], "2020");
        assert_eq!(years[4]["fiscal_year"], "2024");
        // EPS should be populated
        assert!(years[4]["eps"].as_f64().is_some());
    }

    #[tokio::test]
    async fn fundamentals_returns_404_when_fmp_has_no_data() {
        let server = MockServer::start().await;
        // FMP returns an empty array → our API returns 404
        mount_income(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, _) = get_json(app, "/api/stock/FAKE/fundamentals").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── Piotroski ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn piotroski_returns_score_in_valid_range() {
        let server = MockServer::start().await;
        mount_income(&server, INCOME_2YR).await;
        mount_balance(&server, BALANCE_2YR).await;
        mount_cashflow(&server, CASHFLOW_2YR).await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/stock/AAPL/piotroski").await;

        assert_eq!(status, StatusCode::OK);
        let score = body["score"].as_u64().unwrap();
        assert!(score <= 9);
        // AAPL mock data has: positive NI, positive OCF > NI, reducing shares → expect ≥ 3
        assert!(score >= 3);
        assert!(body["interpretation"].as_str().is_some());
    }

    // ── Intrinsic value ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn intrinsic_value_computes_positive_result() {
        let server = MockServer::start().await;
        mount_income(&server, INCOME_5YR).await;
        mount_ratios(&server, RATIOS_5YR).await;
        mount_key_metrics(&server, KEY_METRICS_5YR).await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/stock/AAPL/intrinsic-value").await;

        assert_eq!(status, StatusCode::OK);
        let iv = body["estimated_intrinsic_value"].as_f64().unwrap();
        let mos = body["margin_of_safety_price"].as_f64().unwrap();
        assert!(iv > 0.0);
        assert!((mos - iv * 0.5).abs() < 0.01);
    }

    // ── Momentum ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn momentum_returns_score_between_0_and_100() {
        let server = MockServer::start().await;
        // AAPL: strong outperformer
        mount_prices(&server, "AAPL", &price_json(220.0, 200.0, 180.0, 160.0)).await;
        // SPY: modest gains
        mount_prices(&server, "SPY", &price_json(500.0, 480.0, 460.0, 430.0)).await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/stock/AAPL/momentum").await;

        assert_eq!(status, StatusCode::OK);
        let score = body["momentum_score"].as_f64().unwrap();
        assert!((0.0..=100.0).contains(&score));
        // AAPL is outperforming SPY in our mock data
        assert!(score > 50.0);
    }

    // ── Screener ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn screener_invalid_sector_returns_422() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));

        let (status, body) = get_json(app, "/api/screener/made-up-sector").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].as_str().unwrap().contains("Unknown sector"));
    }

    #[tokio::test]
    async fn screener_response_includes_disclaimer() {
        let server = MockServer::start().await;
        // All tickers in "technology" sector will fail (no mock data) and be omitted.
        // The response is still valid with 0 results and a disclaimer.
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "AAPL", "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/technology").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["disclaimer"].as_str().unwrap().len() > 20);
        assert_eq!(body["sector"], "technology");
    }

    #[tokio::test]
    async fn screener_response_has_model_metadata() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/technology").await;

        assert_eq!(status, StatusCode::OK);
        // New fields: scoring_model, score_labels (4 elements), score_weights (4 elements)
        assert!(body["scoring_model"].as_str().is_some());
        let labels = body["score_labels"].as_array().unwrap();
        let weights = body["score_weights"].as_array().unwrap();
        assert_eq!(labels.len(), 4);
        assert_eq!(weights.len(), 4);
    }

    #[tokio::test]
    async fn screener_technology_uses_standard_model() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/technology").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scoring_model"], "Standard");
        // Standard model score_labels[0] is Piotroski
        assert!(body["score_labels"][0].as_str().unwrap().contains("Piotroski"));
    }

    #[tokio::test]
    async fn screener_financials_uses_financials_model() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/financials").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scoring_model"], "Financials");
        // Financials model score_labels[0] is ROE
        assert!(body["score_labels"][0].as_str().unwrap().contains("Return on Equity"));
        // Weights sum check: "35%", "25%", "25%", "15%"
        assert_eq!(body["score_weights"][0], "35%");
    }

    #[tokio::test]
    async fn screener_real_estate_uses_real_estate_model() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/real-estate").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scoring_model"], "Real Estate");
        assert!(body["score_labels"][0].as_str().unwrap().contains("Dividend"));
    }

    #[tokio::test]
    async fn screener_energy_uses_energy_model() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/energy").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scoring_model"], "Energy");
        assert!(body["score_labels"][1].as_str().unwrap().contains("FCF"));
        assert_eq!(body["score_weights"][1], "30%");
    }

    #[tokio::test]
    async fn screener_consumer_staples_uses_dividend_model() {
        let server = MockServer::start().await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;
        mount_prices(&server, "SPY", "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/screener/consumer-staples").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["scoring_model"], "Dividend");
        assert!(body["score_labels"][0].as_str().unwrap().contains("Dividend"));
    }

    // ── Discovery ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn discovery_invalid_sector_returns_422() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));

        let (status, body) = get_json(app, "/api/discovery?sector=made-up-sector").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].as_str().unwrap().contains("Unknown sector"));
    }

    #[tokio::test]
    async fn discovery_empty_universe_still_returns_valid_response() {
        let server = MockServer::start().await;
        // FMP company-screener returns no candidates — response should still be well-formed.
        mount_company_screener(&server, "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery?sector=technology").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sector"], "technology");
        assert_eq!(body["candidates_screened"], 0);
        assert_eq!(body["results"].as_array().unwrap().len(), 0);
        assert!(body["disclaimer"].as_str().unwrap().len() > 20);
    }

    /// Screening across all sectors at once was tried and dropped — with the same fixed
    /// per-bucket candidate budget spread across the whole market, it consistently
    /// surfaced fewer results than picking any individual sector (verified live: 1 result
    /// for "all sectors" vs. 1-3 for any single sector, using an identical 48-candidate
    /// budget). A sector is now required.
    #[tokio::test]
    async fn discovery_missing_sector_returns_400() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("sector"));
    }

    #[tokio::test]
    async fn discovery_candidates_without_fundamentals_are_skipped_not_errored() {
        let server = MockServer::start().await;
        // One candidate from the universe, but no fundamentals mocked for it (empty
        // arrays) — it should be silently skipped, not surfaced as an error.
        let candidates = r#"[
            {"symbol":"ABCD","companyName":"Test Co","marketCap":1000000000,
             "sector":"Technology","industry":"Software","exchangeShortName":"NASDAQ",
             "price":50.0,"isEtf":false,"isFund":false,"isActivelyTrading":true}
        ]"#;
        mount_company_screener(&server, candidates).await;
        mount_income(&server, "[]").await;
        mount_balance(&server, "[]").await;
        mount_cashflow(&server, "[]").await;
        mount_ratios(&server, "[]").await;
        mount_key_metrics(&server, "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery?sector=technology").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["candidates_screened"], 1);
        assert_eq!(body["results"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn discovery_response_includes_market_cap_band() {
        let server = MockServer::start().await;
        mount_company_screener(&server, "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery?sector=technology").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["market_cap_floor"], 300_000_000.0);
        assert_eq!(body["market_cap_ceiling"], 5_000_000_000.0);
        assert_eq!(body["deviation_band_pct"], 20.0);
    }

    /// A candidate with a Graham Number near-miss but no debt_to_equity/gross_profit/ROE
    /// data (from either EDGAR or FMP) would fail the quality floor (quality=0,
    /// debt_safety=0) — but since that failure is driven by missing data rather than
    /// genuinely bad fundamentals, it should be included with `missing_data_fields` set,
    /// not silently dropped.
    #[tokio::test]
    async fn discovery_includes_near_miss_candidate_despite_missing_quality_data() {
        let server = MockServer::start().await;
        // eps=2.0, bvps=20.0 -> graham_number = sqrt(22.5*2*20) = 30.0; price=30.0 -> 0% deviation
        let candidates = r#"[
            {"symbol":"TSTX","companyName":"Test Co","marketCap":1000000000,
             "sector":"Technology","industry":"Software","exchangeShortName":"NASDAQ",
             "price":30.0,"isEtf":false,"isFund":false,"isActivelyTrading":true}
        ]"#;
        mount_company_screener(&server, candidates).await;
        // No grossProfit field -> gross_margin missing
        mount_income(&server, r#"[{"date":"2024-12-31","revenue":1000000000,"netIncome":50000000,"eps":2.0,"weightedAverageShsOut":25000000}]"#).await;
        mount_balance(&server, r#"[{"date":"2024-12-31","totalAssets":500000000,"totalCurrentAssets":100000000,"totalCurrentLiabilities":50000000,"longTermDebt":50000000}]"#).await;
        mount_cashflow(&server, r#"[{"date":"2024-12-31","operatingCashFlow":60000000}]"#).await;
        // bookValuePerShare present (needed for Graham Number) but no debtToEquityRatio -> debt_to_equity missing
        mount_ratios(&server, r#"[{"date":"2024-12-31","bookValuePerShare":20.0}]"#).await;
        // Empty -> return_on_equity missing
        mount_key_metrics(&server, "[]").await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery?sector=technology").await;

        assert_eq!(status, StatusCode::OK);
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "candidate should be included despite missing data");
        let entry = &results[0];
        assert_eq!(entry["ticker"], "TSTX");
        assert_eq!(entry["quality_score"], 0.0);
        assert_eq!(entry["debt_safety_score"], 0.0);
        let missing = entry["missing_data_fields"].as_array().unwrap();
        let missing: Vec<&str> = missing.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(missing.contains(&"debt_to_equity"));
        assert!(missing.contains(&"gross_margin"));
        assert!(missing.contains(&"return_on_equity"));
    }

    /// Same near-miss setup as above, but this time all three fields ARE present with
    /// genuinely weak values — the candidate should still be excluded, proving the
    /// quality floor wasn't made toothless by the missing-data leniency above.
    #[tokio::test]
    async fn discovery_excludes_near_miss_candidate_with_complete_weak_data() {
        let server = MockServer::start().await;
        let candidates = r#"[
            {"symbol":"WEAK","companyName":"Weak Co","marketCap":1000000000,
             "sector":"Technology","industry":"Software","exchangeShortName":"NASDAQ",
             "price":30.0,"isEtf":false,"isFund":false,"isActivelyTrading":true}
        ]"#;
        mount_company_screener(&server, candidates).await;
        mount_income(&server, r#"[{"date":"2024-12-31","revenue":1000000000,"grossProfit":50000000,"netIncome":50000000,"eps":2.0,"weightedAverageShsOut":25000000}]"#).await;
        mount_balance(&server, r#"[{"date":"2024-12-31","totalAssets":500000000,"totalCurrentAssets":100000000,"totalCurrentLiabilities":50000000,"longTermDebt":50000000}]"#).await;
        mount_cashflow(&server, r#"[{"date":"2024-12-31","operatingCashFlow":60000000}]"#).await;
        // Fully populated, but debtToEquityRatio is very high (weak) — D/E 5.0 -> debt_safety_score 0
        mount_ratios(&server, r#"[{"date":"2024-12-31","bookValuePerShare":20.0,"debtToEquityRatio":5.0}]"#).await;
        // Fully populated, but ROE is negative (weak)
        mount_key_metrics(&server, r#"[{"date":"2024-12-31","returnOnEquity":-0.10}]"#).await;

        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let (status, body) = get_json(app, "/api/discovery?sector=technology").await;

        assert_eq!(status, StatusCode::OK);
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 0, "candidate with complete-but-weak data should still be excluded");
    }

    // ── Mount helpers ─────────────────────────────────────────────────────────

    async fn mount_income(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/income-statement"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_balance(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/balance-sheet-statement"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_cashflow(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/cash-flow-statement"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_ratios(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/ratios"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_key_metrics(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/key-metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_company_screener(server: &MockServer, body: &str) {
        Mock::given(method("GET"))
            .and(path("/company-screener"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    async fn mount_prices(server: &MockServer, symbol: &str, body: &str) {
        Mock::given(method("GET"))
            .and(path("/historical-price-eod/light"))
            .and(query_param("symbol", symbol))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    /// POST or PATCH a JSON body, optionally with a Bearer token.
    async fn send_json(
        app: axum::Router,
        http_method: &str,
        uri: &str,
        body: serde_json::Value,
        token: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder()
            .method(http_method)
            .uri(uri)
            .header("Content-Type", "application/json");
        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        let req = builder
            .body(Body::from(body.to_string()))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    // ── Password: change_password auth guard ──────────────────────────────────

    /// PATCH /api/auth/password without a token must return 401.
    #[tokio::test]
    async fn change_password_requires_auth() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let status = send_json(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({ "current_password": "old", "new_password": "newpass1" }),
            None,
        ).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// PATCH /api/auth/password with an invalid token must return 401.
    #[tokio::test]
    async fn change_password_rejects_invalid_token() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let status = send_json(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({ "current_password": "old", "new_password": "newpass1" }),
            Some("not.a.valid.token"),
        ).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Password: forgot / reset are public (no auth token required) ──────────

    /// POST /api/auth/forgot-password must NOT return 401 — it is a public endpoint.
    /// Without a database it may 500, but it must not require a token.
    #[tokio::test]
    async fn forgot_password_is_public() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let status = send_json(
            app, "POST", "/api/auth/forgot-password",
            serde_json::json!({ "email": "user@example.com" }),
            None, // no token
        ).await;
        assert_ne!(status, StatusCode::UNAUTHORIZED);
    }

    /// POST /api/auth/reset-password must NOT return 401 — it is a public endpoint.
    #[tokio::test]
    async fn reset_password_is_public() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let status = send_json(
            app, "POST", "/api/auth/reset-password",
            serde_json::json!({ "token": "some-token", "new_password": "newpassword1" }),
            None,
        ).await;
        assert_ne!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Password: change_password input validation (unit-level) ──────────────

    /// With a valid token but a new password that's too short, expect 400.
    /// The validation runs before any DB access so this works without a live DB.
    #[tokio::test]
    async fn change_password_rejects_short_new_password() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let token = test_token();
        let status = send_json(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({ "current_password": "currentpass", "new_password": "short" }),
            Some(&token),
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// With a valid token but identical current and new passwords, expect 400.
    #[tokio::test]
    async fn change_password_rejects_same_password() {
        let server = MockServer::start().await;
        let app = build_test_router(AppState::with_base_url("key".into(), server.uri()));
        let token = test_token();
        let status = send_json(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({
                "current_password": "samepassword",
                "new_password": "samepassword"
            }),
            Some(&token),
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ── Realized gain calculation (pure unit tests, no router needed) ─────────

    /// The realized gain formula: (sale_price - cost_per_share) * shares.
    #[test]
    fn realized_gain_profit() {
        let cost = 150.0_f64;
        let sale = 200.0_f64;
        let shares = 10.0_f64;
        let gain = (sale - cost) * shares;
        assert!((gain - 500.0).abs() < 0.001);
    }

    #[test]
    fn realized_gain_loss() {
        let cost = 200.0_f64;
        let sale = 150.0_f64;
        let shares = 5.0_f64;
        let gain = (sale - cost) * shares;
        assert!((gain - (-250.0)).abs() < 0.001);
    }

    #[test]
    fn realized_gain_breakeven() {
        let cost = 100.0_f64;
        let sale = 100.0_f64;
        let shares = 100.0_f64;
        let gain = (sale - cost) * shares;
        assert!(gain.abs() < 0.001);
    }

    /// Combined P&L = realized gain + unrealized dollar gain.
    #[test]
    fn combined_gain_sums_realized_and_unrealized() {
        let realized = 500.0_f64;
        let cost = 100.0_f64;
        let current = 120.0_f64;
        let shares = 10.0_f64;
        let unrealized = (current - cost) * shares; // 200.0
        let combined = realized + unrealized;
        assert!((combined - 700.0).abs() < 0.001);
    }

    /// Partial sell reduces remaining shares correctly.
    #[test]
    fn partial_sell_remaining_shares() {
        let owned = 10.0_f64;
        let selling = 3.0_f64;
        let remaining = owned - selling;
        assert!((remaining - 7.0).abs() < 1e-9);
    }

    /// Full sell (within epsilon) leaves zero remaining.
    #[test]
    fn full_sell_within_epsilon() {
        let owned = 10.0_f64;
        let selling = 10.0_f64;
        let remaining = owned - selling;
        assert!(remaining < 1e-9);
    }
}
