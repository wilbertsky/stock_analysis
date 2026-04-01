mod calculations;
mod error;
mod fmp;
mod models;
mod routes;
mod sectors;
mod state;

#[cfg(test)]
mod route_tests;

use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

use models::*;
use state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Stock Analysis API",
        version = "0.1.0",
        description = "Quantitative stock analysis using publicly available financial data: \
            DCF intrinsic value, Graham Number, PEG ratio, Piotroski F-Score, quality scoring, \
            momentum, and a sector screener — all powered by FMP.\n\n\
            **Disclaimer:** All scores and outputs are provided for educational purposes only. \
            They do not constitute investment advice, a recommendation to buy or sell any security, \
            or a guarantee of future performance. Always conduct your own research and consult a \
            licensed financial advisor before making investment decisions."
    ),
    paths(
        routes::health_check,
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
        routes::screener::get_sector_top_picks,
    ),
    components(schemas(
        HealthResponse,
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
        error::ErrorBody,
    )),
    tags(
        (name = "health", description = "Service health"),
        (name = "stock", description = "Stock valuation endpoints"),
        (name = "screener", description = "Sector screener"),
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

    let state = AppState::new(api_key);

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
        .routes(routes!(routes::screener::get_sector_top_picks))
        .with_state(state)
        .split_for_parts();

    let app = router.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();
    tracing::info!("Listening on http://localhost:8080");
    tracing::info!("Swagger UI: http://localhost:8080/swagger-ui");
    axum::serve(listener, app).await.unwrap();
}
