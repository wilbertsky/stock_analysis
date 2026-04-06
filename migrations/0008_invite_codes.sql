CREATE TABLE IF NOT EXISTS invite_codes (
    code        TEXT PRIMARY KEY,
    email       TEXT NOT NULL,
    created_by  UUID NOT NULL REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    used_at     TIMESTAMPTZ,
    used_by     UUID REFERENCES users(id)
);
