use axum::{extract::State, Json};
use futures::future::join_all;
use std::time::Instant;

use crate::{error::AppError, models::{MarketQuote, MarketSnapshotResponse}, state::AppState};

const TTL_SECS: u64 = 600; // 10-minute cache

/// Ordered list of tickers and their display labels for the dashboard snapshot strip.
const SNAPSHOT_TICKERS: &[(&str, &str)] = &[
    ("SPY",  "S&P 500"),
    ("QQQ",  "Nasdaq 100"),
    ("DIA",  "Dow Jones"),
    ("XLK",  "Technology"),
    ("XLF",  "Financials"),
    ("XLE",  "Energy"),
    ("XLV",  "Healthcare"),
    ("XLI",  "Industrials"),
    ("XLY",  "Cons. Discretionary"),
    ("XLP",  "Cons. Staples"),
    ("XLRE", "Real Estate"),
    ("XLU",  "Utilities"),
    ("XLB",  "Materials"),
    ("XLC",  "Communication"),
];

#[utoipa::path(
    get,
    path = "/api/market-snapshot",
    tag = "market",
    security(()),
    description = "Current price and day-change for SPY, QQQ, DIA, and the 11 SPDR sector ETFs. \
        Cached for 10 minutes — suitable for a dashboard overview strip, not real-time quoting.",
    responses(
        (status = 200, description = "Market snapshot quotes", body = MarketSnapshotResponse),
        (status = 502, description = "Data provider error", body = crate::error::ErrorBody),
    )
)]
pub async fn get_market_snapshot(
    State(state): State<AppState>,
) -> Result<Json<MarketSnapshotResponse>, AppError> {
    // Return cached data if still fresh.
    {
        let guard = state.market_snapshot_cache.read().await;
        if let Some((cached_at, quotes)) = guard.as_ref() {
            if cached_at.elapsed().as_secs() < TTL_SECS {
                return Ok(Json(MarketSnapshotResponse { quotes: quotes.clone() }));
            }
        }
    }

    // Fire all 14 single-symbol quote requests in parallel. Individual calls are more
    // reliable than batch because FMP's batch endpoint URL-encodes commas inconsistently.
    let fmp = &state.fmp;
    let futures = SNAPSHOT_TICKERS.iter().map(|(sym, name)| async move {
        match fmp.quote_full(sym).await {
            Ok(q) => q.price.map(|price| MarketQuote {
                symbol: sym.to_string(),
                name: name.to_string(),
                price,
                change_pct: q.changes_percentage.unwrap_or(0.0),
                change: q.change.unwrap_or(0.0),
            }),
            Err(e) => {
                tracing::warn!(symbol = sym, "market snapshot quote failed: {e}");
                None
            }
        }
    });

    // SNAPSHOT_TICKERS order is preserved because join_all maintains input order.
    let quotes: Vec<MarketQuote> = join_all(futures).await
        .into_iter()
        .flatten()
        .collect();

    if quotes.is_empty() {
        return Err(AppError::Internal("All FMP quote requests failed".into()));
    }

    let mut guard = state.market_snapshot_cache.write().await;
    *guard = Some((Instant::now(), quotes.clone()));

    Ok(Json(MarketSnapshotResponse { quotes }))
}
