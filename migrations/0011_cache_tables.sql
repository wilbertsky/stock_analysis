-- Persistent cache for discovery and screener results.
-- Survives service restarts and Railway redeployments.
-- Each sector has one row; the background 12h refresh does an UPSERT,
-- so stale sectors are updated without clearing unrelated ones.

CREATE TABLE IF NOT EXISTS screener_cache (
    sector    TEXT        PRIMARY KEY,
    data      JSONB       NOT NULL,
    cached_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS discovery_cache (
    sector    TEXT        PRIMARY KEY,
    data      JSONB       NOT NULL,
    cached_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
