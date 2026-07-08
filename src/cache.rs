//! DB-backed persistence for discovery and screener caches.
//!
//! The in-memory caches in AppState are cleared on every process restart.
//! These helpers read the last-known results from Postgres on startup and
//! write back after every refresh, so redeployments don't force FMP re-queries.
//!
//! Persistence failures are logged and silently ignored — a failed write just
//! means the next restart falls back to cold compute, not that the request fails.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use sqlx::PgPool;
use sqlx::Row as _;
use tokio::sync::RwLock;

use crate::models::{DiscoveryResponse, SectorScreenerResponse};

/// Load all rows from both cache tables into the in-memory maps.
/// Called once at startup (after migrations) so requests are served immediately.
/// The loaded rows are stamped with Instant::now() — they look "fresh" to the
/// TTL check in each handler, and the 12h background task will refresh them anyway.
pub async fn load_from_db(
    db: &PgPool,
    discovery_cache: &Arc<RwLock<HashMap<String, (Instant, DiscoveryResponse)>>>,
    screener_cache: &Arc<RwLock<HashMap<String, (Instant, SectorScreenerResponse)>>>,
) {
    match sqlx::query("SELECT sector, data FROM discovery_cache")
        .fetch_all(db)
        .await
    {
        Ok(rows) => {
            let mut cache = discovery_cache.write().await;
            let mut loaded = 0usize;
            for row in rows {
                let sector: String = match row.try_get("sector") {
                    Ok(s) => s,
                    Err(e) => { tracing::warn!("discovery_cache: bad sector column: {e}"); continue; }
                };
                let data: serde_json::Value = match row.try_get("data") {
                    Ok(d) => d,
                    Err(e) => { tracing::warn!("discovery_cache: bad data column for {sector}: {e}"); continue; }
                };
                match serde_json::from_value::<DiscoveryResponse>(data) {
                    Ok(response) => { cache.insert(sector, (Instant::now(), response)); loaded += 1; }
                    Err(e) => tracing::warn!("discovery_cache: deserialize failed for {sector}: {e}"),
                }
            }
            tracing::info!("Loaded {loaded} discovery cache entries from DB");
        }
        Err(e) => tracing::warn!("Failed to load discovery cache from DB: {e}"),
    }

    match sqlx::query("SELECT sector, data FROM screener_cache")
        .fetch_all(db)
        .await
    {
        Ok(rows) => {
            let mut cache = screener_cache.write().await;
            let mut loaded = 0usize;
            for row in rows {
                let sector: String = match row.try_get("sector") {
                    Ok(s) => s,
                    Err(e) => { tracing::warn!("screener_cache: bad sector column: {e}"); continue; }
                };
                let data: serde_json::Value = match row.try_get("data") {
                    Ok(d) => d,
                    Err(e) => { tracing::warn!("screener_cache: bad data column for {sector}: {e}"); continue; }
                };
                match serde_json::from_value::<SectorScreenerResponse>(data) {
                    Ok(response) => { cache.insert(sector, (Instant::now(), response)); loaded += 1; }
                    Err(e) => tracing::warn!("screener_cache: deserialize failed for {sector}: {e}"),
                }
            }
            tracing::info!("Loaded {loaded} screener cache entries from DB");
        }
        Err(e) => tracing::warn!("Failed to load screener cache from DB: {e}"),
    }
}

/// Upsert one screener result into the DB. Non-fatal on failure.
pub async fn persist_screener(db: &PgPool, sector: &str, response: &SectorScreenerResponse) {
    let data = match serde_json::to_value(response) {
        Ok(v) => v,
        Err(e) => { tracing::warn!("screener_cache: serialize failed for {sector}: {e}"); return; }
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO screener_cache (sector, data, cached_at)
         VALUES ($1, $2, NOW())
         ON CONFLICT (sector) DO UPDATE SET data = EXCLUDED.data, cached_at = NOW()",
    )
    .bind(sector)
    .bind(data)
    .execute(db)
    .await
    {
        tracing::warn!("screener_cache: DB write failed for {sector}: {e}");
    }
}

/// Upsert one discovery result into the DB. Non-fatal on failure.
pub async fn persist_discovery(db: &PgPool, sector: &str, response: &DiscoveryResponse) {
    let data = match serde_json::to_value(response) {
        Ok(v) => v,
        Err(e) => { tracing::warn!("discovery_cache: serialize failed for {sector}: {e}"); return; }
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO discovery_cache (sector, data, cached_at)
         VALUES ($1, $2, NOW())
         ON CONFLICT (sector) DO UPDATE SET data = EXCLUDED.data, cached_at = NOW()",
    )
    .bind(sector)
    .bind(data)
    .execute(db)
    .await
    {
        tracing::warn!("discovery_cache: DB write failed for {sector}: {e}");
    }
}
