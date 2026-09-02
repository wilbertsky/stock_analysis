ALTER TABLE portfolios ADD COLUMN is_ai_generated BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE ai_portfolio_selections (
    id                  UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    portfolio_id        UUID          NOT NULL REFERENCES portfolios(id) ON DELETE CASCADE,
    ticker              TEXT          NOT NULL,
    sector              TEXT          NOT NULL,
    composite_score     NUMERIC(6,2)  NOT NULL,
    score_a             NUMERIC(6,2),
    score_b             NUMERIC(6,2),
    score_c             NUMERIC(6,2),
    score_d             NUMERIC(6,2),
    news_sentiment      TEXT          CHECK (news_sentiment IN ('positive', 'neutral', 'negative')),
    selection_rationale TEXT          NOT NULL,
    cycle               TEXT          NOT NULL,
    selected_at         TIMESTAMPTZ   NOT NULL DEFAULT now()
);

CREATE INDEX ai_selections_portfolio_cycle_idx ON ai_portfolio_selections(portfolio_id, cycle, selected_at DESC);
CREATE INDEX ai_selections_ticker_idx ON ai_portfolio_selections(ticker);
