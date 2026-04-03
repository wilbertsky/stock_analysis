CREATE TABLE portfolio_holdings (
    id             UUID           PRIMARY KEY DEFAULT gen_random_uuid(),
    portfolio_id   UUID           NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    ticker         TEXT           NOT NULL,
    -- Price (USD) at the moment the holding was added, fetched live from FMP
    price_at_add   NUMERIC(18,6)  NOT NULL,
    -- Optional: number of shares. NULL means "tracking only" (no dollar value)
    shares         NUMERIC(18,6),
    added_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);

CREATE INDEX idx_holdings_portfolio_id ON portfolio_holdings(portfolio_id);
