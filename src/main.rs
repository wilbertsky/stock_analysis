mod auth;
mod calculations;
mod crypto;
mod edgar;
mod error;
mod fmp;
mod models;
mod providers;
mod routes;
mod sectors;
mod sp500;
mod state;
mod yahoo;

#[cfg(test)]
mod route_tests;
#[cfg(test)]
mod db_tests;

use tower_http::cors::{Any, CorsLayer};
use utoipa::{openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme}, Modify, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

use models::*;
use state::AppState;

struct BearerAuth;
impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new().scheme(HttpAuthScheme::Bearer).bearer_format("JWT").build(),
                ),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Stock Analysis API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Quantitative stock analysis using publicly available financial data: \
            DCF intrinsic value, Graham Number, PEG ratio, Piotroski F-Score, quality scoring, \
            momentum, sector screener, user accounts, and portfolio tracking.\n\n\
            **Disclaimer:** All scores and outputs are provided for educational purposes only. \
            They do not constitute investment advice, a recommendation to buy or sell any security, \
            or a guarantee of future performance. Always conduct your own research and consult a \
            licensed financial advisor before making investment decisions."
    ),
    modifiers(&BearerAuth),
    // All endpoints require a valid JWT by default.
    // Public endpoints override this with security(()) in their path macro.
    security(
        ("bearer_auth" = [])
    ),
    paths(
        routes::health_check,
        routes::feedback::submit_feedback,
        routes::feedback::list_feedback,
        routes::feedback::mark_read,
        routes::stock::get_fundamentals,
        routes::stock::get_growth_rates,
        routes::stock::get_intrinsic_value,
        routes::stock::get_graham_number,
        routes::stock::get_peg,
        routes::stock::get_summary,
        routes::stock::get_piotroski,
        routes::stock::get_dividends,
        routes::stock::get_quality,
        routes::stock::get_momentum,
        routes::stock::search_stocks,
        routes::stock::get_company_profile,
        routes::stock::get_company_news,
        routes::screener::get_sector_top_picks,
        routes::discovery::get_discovery,
        routes::chat::post_chat,
        routes::admin::create_invite,
        routes::admin::list_invites,
        routes::auth::register,
        routes::auth::login,
        routes::auth::get_me,
        routes::auth::update_profile,
        routes::auth::upsert_fmp_key,
        routes::auth::change_password,
        routes::auth::forgot_password,
        routes::auth::reset_password,
        routes::portfolio::create_portfolio,
        routes::portfolio::list_portfolios,
        routes::portfolio::get_portfolio,
        routes::portfolio::delete_portfolio,
        routes::portfolio::add_holding,
        routes::portfolio::import_holdings,
        routes::portfolio::remove_holding,
        routes::portfolio::sell_holding,
        routes::portfolio::update_portfolio,
        routes::portfolio::get_public_portfolio,
        routes::portfolio::get_community_portfolios,
    ),
    components(schemas(
        HealthResponse,
        SubmitFeedbackRequest,
        FeedbackRow,
        FundamentalsYear,
        FundamentalsResponse,
        MetricCagr,
        GrowthRatesResponse,
        IntrinsicValueResponse,
        GrahamNumberResponse,
        PegRatioResponse,
        SummaryResponse,
        PiotroskiResponse,
        DividendMetricsResponse,
        QualityScoreResponse,
        MomentumResponse,
        ScreenerEntry,
        SectorScreenerResponse,
        DiscoveryEntry,
        DiscoveryResponse,
        StockSearchResult,
        StockSearchResponse,
        CompanyProfileResponse,
        CompanyNewsResponse,
        NewsItem,
        CreateInviteRequest,
        InviteCodeRow,
        RegisterRequest,
        LoginRequest,
        AuthResponse,
        MeResponse,
        UpdateProfileRequest,
        UpsertFmpKeyRequest,
        ChangePasswordRequest,
        ForgotPasswordRequest,
        ResetPasswordRequest,
        CreatePortfolioRequest,
        PortfolioRow,
        AddHoldingRequest,
        HoldingRow,
        HoldingPerformance,
        PortfolioPerformanceResponse,
        PublicPortfolioSummary,
        ImportRowResult,
        ImportHoldingsResponse,
        SellHoldingRequest,
        UpdatePortfolioRequest,
        RealizedGainRow,
        RealizedGainsSummary,
        error::ErrorBody,
    )),
    tags(
        (name = "admin", description = "Admin-only operations"),
        (name = "health", description = "Service health"),
        (name = "feedback", description = "User feedback"),
        (name = "stock", description = "Stock valuation and analysis"),
        (name = "screener", description = "Sector screener"),
        (name = "discovery", description = "Small/mid-cap near-miss value discovery screener"),
        (name = "chat", description = "AI chat proxy — forwards authenticated requests to the RAG service"),
        (name = "auth", description = "User accounts and authentication"),
        (name = "portfolio", description = "Portfolio management and performance tracking"),
    )
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axum_api=debug".into()),
        )
        .init();

    let api_key =
        std::env::var("FMP_API_KEY").expect("FMP_API_KEY must be set in environment or .env file");

    let state = AppState::new(api_key).await;

    // Run database migrations on startup.
    sqlx::migrate!()
        .run(&state.db)
        .await
        .expect("Database migration failed");

    // Background cache refresh — populates discovery and screener caches for all sectors
    // on a 12h cycle so requests are served instantly from cache rather than hammering FMP
    // on every click. Discovery uses 60s inter-sector delays (FMP rate-limit headroom);
    // screener uses EDGAR/Yahoo as primary so no delay is needed there.
    {
        let state = state.clone();
        tokio::spawn(async move {
            // Initial delay: let the boot-time Sp500::load() FMP calls settle before the
            // background task starts adding more.
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            loop {
                tracing::info!("Background cache refresh: starting discovery");
                let mut first = true;
                for &slug in sectors::ALL_SECTOR_SLUGS {
                    if !first {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                    first = false;
                    if let Some(fmp_sector) = sectors::slug_to_fmp_sector(slug) {
                        match routes::discovery::run_discovery(&state.fmp, slug, fmp_sector).await {
                            Ok(response) => {
                                state.discovery_cache.write().await.insert(
                                    slug.to_owned(),
                                    (std::time::Instant::now(), response),
                                );
                                tracing::info!(sector = slug, "Discovery cache refreshed");
                            }
                            Err(e) => tracing::warn!(sector = slug, "Discovery cache refresh failed: {e}"),
                        }
                    }
                }

                tracing::info!("Background cache refresh: starting screener");
                for &slug in sectors::ALL_SECTOR_SLUGS {
                    match routes::screener::run_screener(&state, slug).await {
                        Ok(response) => {
                            state.screener_cache.write().await.insert(
                                slug.to_owned(),
                                (std::time::Instant::now(), response),
                            );
                            tracing::info!(sector = slug, "Screener cache refreshed");
                        }
                        Err(e) => tracing::warn!(sector = slug, "Screener cache refresh failed: {e}"),
                    }
                }

                tracing::info!("Background cache refresh: complete, sleeping 12h");
                tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
            }
        });
    }

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::health_check))
        .routes(routes!(routes::stock::get_fundamentals))
        .routes(routes!(routes::stock::get_growth_rates))
        .routes(routes!(routes::stock::get_intrinsic_value))
        .routes(routes!(routes::stock::get_graham_number))
        .routes(routes!(routes::stock::get_peg))
        .routes(routes!(routes::stock::get_summary))
        .routes(routes!(routes::stock::get_piotroski))
        .routes(routes!(routes::stock::get_dividends))
        .routes(routes!(routes::stock::get_quality))
        .routes(routes!(routes::stock::get_momentum))
        .routes(routes!(routes::stock::search_stocks))
        .routes(routes!(routes::stock::get_company_profile))
        .routes(routes!(routes::stock::get_company_news))
        .routes(routes!(routes::screener::get_sector_top_picks))
        .routes(routes!(routes::discovery::get_discovery))
        .routes(routes!(routes::chat::post_chat))
        .routes(routes!(routes::admin::create_invite))
        .routes(routes!(routes::admin::list_invites))
        .routes(routes!(routes::auth::register))
        .routes(routes!(routes::auth::login))
        .routes(routes!(routes::auth::get_me))
        .routes(routes!(routes::auth::update_profile))
        .routes(routes!(routes::auth::upsert_fmp_key))
        .routes(routes!(routes::auth::change_password))
        .routes(routes!(routes::auth::forgot_password))
        .routes(routes!(routes::auth::reset_password))
        .routes(routes!(routes::portfolio::create_portfolio))
        .routes(routes!(routes::portfolio::list_portfolios))
        .routes(routes!(routes::portfolio::get_portfolio))
        .routes(routes!(routes::portfolio::delete_portfolio))
        .routes(routes!(routes::portfolio::add_holding))
        .routes(routes!(routes::portfolio::import_holdings))
        .routes(routes!(routes::portfolio::remove_holding))
        .routes(routes!(routes::portfolio::sell_holding))
        .routes(routes!(routes::portfolio::update_portfolio))
        .routes(routes!(routes::portfolio::get_public_portfolio))
        .routes(routes!(routes::portfolio::get_community_portfolios))
        .routes(routes!(routes::feedback::submit_feedback))
        .routes(routes!(routes::feedback::list_feedback))
        .routes(routes!(routes::feedback::mark_read))
        .with_state(state)
        .split_for_parts();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = router
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api))
        .layer(cors);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_owned());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    tracing::info!("Listening on http://0.0.0.0:{}", port);
    tracing::info!("Swagger UI: http://0.0.0.0:{}/swagger-ui", port);
    axum::serve(listener, app).await.unwrap();
}
