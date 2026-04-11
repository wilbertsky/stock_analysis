CREATE TABLE realized_gains (
    id             UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    portfolio_id   UUID          NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    ticker         TEXT          NOT NULL,
    shares         NUMERIC(18,6) NOT NULL,
    cost_per_share NUMERIC(18,6) NOT NULL,
    sale_price     NUMERIC(18,6) NOT NULL,
    realized_gain  NUMERIC(18,8) NOT NULL,
    sold_at        TIMESTAMPTZ   NOT NULL DEFAULT now()
);

CREATE INDEX realized_gains_portfolio_idx ON realized_gains (portfolio_id, sold_at DESC);
