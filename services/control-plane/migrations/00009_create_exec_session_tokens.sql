-- Migration: 00009_create_exec_session_tokens
-- Description: Store exec session connection tokens (hashed) for single-use validation

CREATE TABLE IF NOT EXISTS exec_session_tokens (
    exec_session_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_exec_session_tokens_expires_at
    ON exec_session_tokens (expires_at);

COMMENT ON TABLE exec_session_tokens IS 'Single-use exec session tokens (hashed, short-lived)';
