CREATE TABLE portfolios (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT        NOT NULL,
    is_public    BOOLEAN     NOT NULL DEFAULT false,
    share_token  UUID        NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_portfolios_user_id    ON portfolios(user_id);
CREATE INDEX idx_portfolios_share_token ON portfolios(share_token);
