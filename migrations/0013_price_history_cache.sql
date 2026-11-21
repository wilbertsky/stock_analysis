-- Persistent cache of ~10 years of daily closes per ticker (FMP EOD, newest-first).
-- Shared by /price-history and /forecast; survives restarts and redeploys.
-- Expiry is enforced in application code (see routes::stock::price_series).
CREATE TABLE IF NOT EXISTS price_history_cache (
    ticker    TEXT        PRIMARY KEY,
    data      JSONB       NOT NULL,
    cached_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
