/// Integration tests that require a live PostgreSQL database.
///
/// These run against the DATABASE_URL from `.env`.
/// Each test creates isolated data using unique UUIDs and cleans up after itself.
/// Run with: `cargo test db_tests`
#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::{get, patch, post, delete},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;
    use uuid::Uuid;
    use wiremock::{MockServer, Mock, ResponseTemplate, matchers::{method, path}};

    use crate::{
        auth::{jwt::encode_token, password::hash_password},
        routes,
        state::AppState,
    };

    // ── Router ────────────────────────────────────────────────────────────────

    fn build_router(state: AppState) -> axum::Router {
        axum::Router::new()
            .route("/api/auth/password",         patch(routes::auth::change_password))
            .route("/api/auth/forgot-password",  post(routes::auth::forgot_password))
            .route("/api/auth/reset-password",   post(routes::auth::reset_password))
            .route("/api/portfolio",             post(routes::portfolio::create_portfolio))
            .route("/api/portfolio/{id}/holdings",
                post(routes::portfolio::add_holding))
            .route("/api/portfolio/{id}/holdings/{hid}/sell",
                post(routes::portfolio::sell_holding))
            .route("/api/portfolio/{id}/holdings/{hid}",
                delete(routes::portfolio::remove_holding))
            .route("/api/portfolio/{id}",        get(routes::portfolio::get_portfolio))
            .with_state(state)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn setup(fmp_url: &str) -> AppState {
        AppState::with_db_and_base_url("key".into(), fmp_url.to_owned()).await
    }

    fn token_for(user_id: Uuid) -> String {
        let secret = b"test_jwt_secret_key_32bytes_pad_";
        encode_token(user_id, "test@example.com", "subscriber", secret).unwrap()
    }

    async fn send(
        app: axum::Router,
        method_str: &str,
        uri: &str,
        body: Value,
        token: Option<Uuid>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method_str)
            .uri(uri)
            .header("Content-Type", "application/json");
        if let Some(uid) = token {
            builder = builder.header("Authorization", format!("Bearer {}", token_for(uid)));
        }
        let req = builder.body(Body::from(body.to_string())).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or_default();
        (status, json)
    }

    /// Insert a test user directly into the DB and return their UUID.
    async fn insert_user(state: &AppState, email: &str, plain_password: &str) -> Uuid {
        let hash = tokio::task::spawn_blocking({
            let p = plain_password.to_owned();
            move || hash_password(&p)
        })
        .await
        .unwrap()
        .unwrap();

        let row = sqlx::query_as::<_, (Uuid,)>(
            "INSERT INTO users (email, password_hash, role_id)
             VALUES ($1, $2, (SELECT id FROM roles WHERE name = 'subscriber'))
             RETURNING id",
        )
        .bind(email)
        .bind(hash)
        .fetch_one(&state.db)
        .await
        .expect("Failed to insert test user");
        row.0
    }

    /// Delete test user and all cascade-deleted data.
    async fn delete_user(state: &AppState, user_id: Uuid) {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&state.db)
            .await
            .ok();
    }

    // ── Change password ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn change_password_happy_path() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("chgpw-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "OldPassword1").await;
        let app = build_router(state.clone());

        let (status, _) = send(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({
                "current_password": "OldPassword1",
                "new_password": "NewPassword1"
            }),
            Some(user_id),
        ).await;

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn change_password_wrong_current_fails() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("chgpw-wrong-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "CorrectPassword1").await;
        let app = build_router(state.clone());

        let (status, _) = send(
            app, "PATCH", "/api/auth/password",
            serde_json::json!({
                "current_password": "WrongPassword1",
                "new_password": "AnotherPassword1"
            }),
            Some(user_id),
        ).await;

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Forgot / reset password ───────────────────────────────────────────────

    #[tokio::test]
    async fn forgot_password_unknown_email_returns_204() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let app = build_router(state);

        // Should always return 204 regardless — prevents enumeration
        let (status, _) = send(
            app, "POST", "/api/auth/forgot-password",
            serde_json::json!({ "email": "nobody-exists@test.invalid" }),
            None,
        ).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn forgot_password_known_email_stores_token() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("forgot-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "Password123").await;
        let app = build_router(state.clone());

        let (status, _) = send(
            app, "POST", "/api/auth/forgot-password",
            serde_json::json!({ "email": email }),
            None,
        ).await;

        // Verify a token was stored for this user
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM password_reset_tokens
             WHERE user_id = $1 AND used_at IS NULL AND expires_at > NOW()"
        )
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(count.0, 1, "Expected exactly one active reset token");
    }

    #[tokio::test]
    async fn reset_password_valid_token_updates_password() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("reset-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "OldPass123").await;

        // Manually insert a valid token
        let token = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO password_reset_tokens (token, user_id, expires_at)
             VALUES ($1, $2, NOW() + INTERVAL '1 hour')"
        )
        .bind(&token)
        .bind(user_id)
        .execute(&state.db)
        .await
        .unwrap();

        let app = build_router(state.clone());
        let (status, _) = send(
            app, "POST", "/api/auth/reset-password",
            serde_json::json!({ "token": token, "new_password": "NewPass456" }),
            None,
        ).await;

        // Verify token is now marked used
        let used: (Option<chrono::DateTime<chrono::Utc>>,) = sqlx::query_as(
            "SELECT used_at FROM password_reset_tokens WHERE token = $1"
        )
        .bind(&token)
        .fetch_one(&state.db)
        .await
        .unwrap();

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(used.0.is_some(), "Token should be marked as used");
    }

    #[tokio::test]
    async fn reset_password_expired_token_returns_400() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("reset-exp-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "OldPass123").await;

        let token = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO password_reset_tokens (token, user_id, expires_at)
             VALUES ($1, $2, NOW() - INTERVAL '1 hour')" // already expired
        )
        .bind(&token)
        .bind(user_id)
        .execute(&state.db)
        .await
        .unwrap();

        let app = build_router(state.clone());
        let (status, _) = send(
            app, "POST", "/api/auth/reset-password",
            serde_json::json!({ "token": token, "new_password": "NewPass456" }),
            None,
        ).await;

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reset_password_used_token_returns_400() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("reset-used-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "OldPass123").await;

        let token = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO password_reset_tokens (token, user_id, expires_at, used_at)
             VALUES ($1, $2, NOW() + INTERVAL '1 hour', NOW())" // already used
        )
        .bind(&token)
        .bind(user_id)
        .execute(&state.db)
        .await
        .unwrap();

        let app = build_router(state.clone());
        let (status, _) = send(
            app, "POST", "/api/auth/reset-password",
            serde_json::json!({ "token": token, "new_password": "NewPass456" }),
            None,
        ).await;

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ── Sell holding ──────────────────────────────────────────────────────────

    /// Helper: mount a FMP price mock returning a fixed price.
    async fn mount_price(server: &MockServer, price: f64) {
        Mock::given(method("GET"))
            .and(path("/historical-price-eod/light"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!(
                    r#"[{{"date":"2025-01-01","price":{price}}}]"#
                )),
            )
            .mount(server)
            .await;
    }

    /// Insert a portfolio + holding, returning (portfolio_id, holding_id).
    async fn insert_portfolio_with_holding(
        state: &AppState,
        user_id: Uuid,
        ticker: &str,
        cost: f64,
        shares: f64,
    ) -> (Uuid, Uuid) {
        let p_row = sqlx::query_as::<_, (Uuid,)>(
            "INSERT INTO portfolios (user_id, name, is_public)
             VALUES ($1, $2, false) RETURNING id"
        )
        .bind(user_id)
        .bind(format!("test-{}", Uuid::new_v4()))
        .fetch_one(&state.db)
        .await
        .unwrap();

        let h_row = sqlx::query_as::<_, (Uuid,)>(
            "INSERT INTO portfolio_holdings (portfolio_id, ticker, price_at_add, shares)
             VALUES ($1, $2, $3, $4) RETURNING id"
        )
        .bind(p_row.0)
        .bind(ticker)
        .bind(cost)
        .bind(shares)
        .fetch_one(&state.db)
        .await
        .unwrap();

        (p_row.0, h_row.0)
    }

    #[tokio::test]
    async fn sell_holding_full_removes_holding_and_records_gain() {
        let server = MockServer::start().await;
        mount_price(&server, 200.0).await;
        let state = setup(&server.uri()).await;
        let email = format!("sell-full-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "Password123").await;
        let (portfolio_id, holding_id) =
            insert_portfolio_with_holding(&state, user_id, "AAPL", 150.0, 10.0).await;

        let app = build_router(state.clone());
        let uri = format!("/api/portfolio/{portfolio_id}/holdings/{holding_id}/sell");
        let (status, body) = send(
            app, "POST", &uri,
            serde_json::json!({ "shares": 10.0, "price": 200.0 }),
            Some(user_id),
        ).await;

        // Holding should be gone
        let holding_exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM portfolio_holdings WHERE id = $1"
        )
        .bind(holding_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        // Realized gain should exist
        let gain: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM realized_gains WHERE portfolio_id = $1"
        )
        .bind(portfolio_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(holding_exists.0, 0, "Holding should be deleted after full sell");
        assert_eq!(gain.0, 1, "One realized gain row expected");
        // (200 - 150) * 10 = 500
        assert!((body["realized_gain"].as_f64().unwrap() - 500.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn sell_holding_partial_reduces_shares() {
        let server = MockServer::start().await;
        mount_price(&server, 200.0).await;
        let state = setup(&server.uri()).await;
        let email = format!("sell-partial-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "Password123").await;
        let (portfolio_id, holding_id) =
            insert_portfolio_with_holding(&state, user_id, "MSFT", 100.0, 10.0).await;

        let app = build_router(state.clone());
        let uri = format!("/api/portfolio/{portfolio_id}/holdings/{holding_id}/sell");
        let (status, _) = send(
            app, "POST", &uri,
            serde_json::json!({ "shares": 4.0, "price": 200.0 }),
            Some(user_id),
        ).await;

        // Holding should still exist with 6 shares
        let remaining: (Option<f64>,) = sqlx::query_as(
            "SELECT shares::FLOAT8 FROM portfolio_holdings WHERE id = $1"
        )
        .bind(holding_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::OK);
        assert!((remaining.0.unwrap() - 6.0).abs() < 1e-6, "Should have 6 shares remaining");
    }

    #[tokio::test]
    async fn sell_holding_too_many_shares_returns_400() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("sell-over-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "Password123").await;
        let (portfolio_id, holding_id) =
            insert_portfolio_with_holding(&state, user_id, "GOOG", 100.0, 5.0).await;

        let app = build_router(state.clone());
        let uri = format!("/api/portfolio/{portfolio_id}/holdings/{holding_id}/sell");
        let (status, body) = send(
            app, "POST", &uri,
            serde_json::json!({ "shares": 10.0 }), // more than the 5 owned
            Some(user_id),
        ).await;

        delete_user(&state, user_id).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("shares"));
    }

    #[tokio::test]
    async fn sell_holding_cannot_sell_other_users_holding() {
        let server = MockServer::start().await;
        mount_price(&server, 200.0).await;
        let state = setup(&server.uri()).await;

        let owner_email = format!("owner-{}@test.invalid", Uuid::new_v4());
        let attacker_email = format!("attacker-{}@test.invalid", Uuid::new_v4());
        let owner_id = insert_user(&state, &owner_email, "Password123").await;
        let attacker_id = insert_user(&state, &attacker_email, "Password123").await;

        let (portfolio_id, holding_id) =
            insert_portfolio_with_holding(&state, owner_id, "TSLA", 100.0, 5.0).await;

        let app = build_router(state.clone());
        let uri = format!("/api/portfolio/{portfolio_id}/holdings/{holding_id}/sell");
        let (status, _) = send(
            app, "POST", &uri,
            serde_json::json!({ "shares": 1.0 }),
            Some(attacker_id), // not the owner
        ).await;

        delete_user(&state, owner_id).await;
        delete_user(&state, attacker_id).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn portfolio_performance_includes_realized_gains() {
        let server = MockServer::start().await;
        let state = setup(&server.uri()).await;
        let email = format!("perf-{}@test.invalid", Uuid::new_v4());
        let user_id = insert_user(&state, &email, "Password123").await;
        // Create a portfolio with no open holdings so no price fetch is needed.
        let p_row: (Uuid,) = sqlx::query_as(
            "INSERT INTO portfolios (user_id, name, is_public) VALUES ($1, $2, false) RETURNING id"
        )
        .bind(user_id)
        .bind(format!("perf-test-{}", Uuid::new_v4()))
        .fetch_one(&state.db)
        .await
        .unwrap();
        let portfolio_id = p_row.0;

        // Insert a realized gain row directly
        sqlx::query(
            "INSERT INTO realized_gains
             (portfolio_id, ticker, shares, cost_per_share, sale_price, realized_gain)
             VALUES ($1, 'AMZN', 10, 100.0, 150.0, 500.0)"
        )
        .bind(portfolio_id)
        .execute(&state.db)
        .await
        .unwrap();

        let token = token_for(user_id);
        let req = Request::builder()
            .uri(format!("/api/portfolio/{portfolio_id}"))
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = build_router(state.clone()).oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();

        delete_user(&state, user_id).await;

        let realized = &body["realized"];
        assert!(realized["total_realized_gain"].as_f64().unwrap() > 0.0);
        assert_eq!(realized["rows"].as_array().unwrap().len(), 1);
        assert_eq!(realized["rows"][0]["ticker"], "AMZN");
    }
}
