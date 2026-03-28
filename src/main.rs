mod calculations;
mod error;
mod fmp;
mod models;
mod routes;
mod state;

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
        description = "Rule #1, Graham Number, PEG ratio, and Big Five fundamentals via FMP."
    ),
    paths(
        routes::health_check,
        routes::stock::get_fundamentals,
        routes::stock::get_growth_rates,
        routes::stock::get_rule_number_one,
        routes::stock::get_graham_number,
        routes::stock::get_peg,
        routes::stock::get_summary,
    ),
    components(schemas(
        HealthResponse,
        FundamentalsYear,
        FundamentalsResponse,
        MetricCagr,
        GrowthRatesResponse,
        StickerPriceResponse,
        GrahamNumberResponse,
        PegRatioResponse,
        SummaryResponse,
        error::ErrorBody,
    )),
    tags(
        (name = "health", description = "Service health"),
        (name = "stock", description = "Stock valuation endpoints"),
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
        .routes(routes!(routes::stock::get_rule_number_one))
        .routes(routes!(routes::stock::get_graham_number))
        .routes(routes!(routes::stock::get_peg))
        .routes(routes!(routes::stock::get_summary))
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
