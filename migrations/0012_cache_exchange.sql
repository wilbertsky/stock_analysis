-- Add exchange dimension to both cache tables.
-- Existing rows (all US) get exchange = 'us' via the DEFAULT.
-- Primary key changes from (sector) to (sector, exchange) so US and LSE
-- results for the same sector coexist independently.

ALTER TABLE screener_cache
    ADD COLUMN exchange TEXT NOT NULL DEFAULT 'us';

ALTER TABLE screener_cache
    DROP CONSTRAINT screener_cache_pkey;

ALTER TABLE screener_cache
    ADD PRIMARY KEY (sector, exchange);

ALTER TABLE discovery_cache
    ADD COLUMN exchange TEXT NOT NULL DEFAULT 'us';

ALTER TABLE discovery_cache
    DROP CONSTRAINT discovery_cache_pkey;

ALTER TABLE discovery_cache
    ADD PRIMARY KEY (sector, exchange);
