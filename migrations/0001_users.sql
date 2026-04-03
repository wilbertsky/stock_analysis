CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        NOT NULL UNIQUE,
    password_hash TEXT        NOT NULL,
    -- AES-256-GCM encrypted blob of the user's FMP API key.
    -- Format: base64( nonce[12 bytes] || ciphertext ).
    -- NULL means the user has not provided an FMP key yet.
    fmp_key_enc   TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_users_email ON users(email);
