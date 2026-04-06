CREATE TABLE IF NOT EXISTS company_profiles (
    ticker          TEXT PRIMARY KEY,
    description     TEXT,
    sector          TEXT,
    industry        TEXT,
    website         TEXT,
    employees       BIGINT,
    fetched_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
