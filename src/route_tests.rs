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

    use crate::{routes, state::AppState};

    // ── Router builder ────────────────────────────────────────────────────────

    fn build_test_router(state: AppState) -> axum::Router {
        use axum::routing::get;
        axum::Router::new()
            .route("/api/health", get(routes::health_check))
            .route("/api/stock/{ticker}/fundamentals",   get(routes::stock::get_fundamentals))
            .route("/api/stock/{ticker}/intrinsic-value",get(routes::stock::get_intrinsic_value))
            .route("/api/stock/{ticker}/graham-number",  get(routes::stock::get_graham_number))
            .route("/api/stock/{ticker}/piotroski",      get(routes::stock::get_piotroski))
            .route("/api/stock/{ticker}/dividends",      get(routes::stock::get_dividends))
            .route("/api/stock/{ticker}/quality",        get(routes::stock::get_quality))
            .route("/api/stock/{ticker}/momentum",       get(routes::stock::get_momentum))
            .route("/api/screener/{sector}",             get(routes::screener::get_sector_top_picks))
            .with_state(state)
    }

    // ── Request helper ────────────────────────────────────────────────────────

    async fn get_json(
        app: axum::Router,
        uri: &str,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .uri(uri)
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

    async fn mount_prices(server: &MockServer, symbol: &str, body: &str) {
        Mock::given(method("GET"))
            .and(path("/historical-price-eod/light"))
            .and(query_param("symbol", symbol))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
}
