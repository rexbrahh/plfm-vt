-- Migration: 00006_create_auth_tables
-- Description: Create authentication tables for tokens, device codes, and service principal credentials
-- See: docs/specs/api/auth.md

-- This migration adds the infrastructure for real authentication:
--   - access_tokens: Short-lived bearer tokens (15 min default)
--   - refresh_tokens: Longer-lived tokens for obtaining new access tokens (30 day default)
--   - device_codes: Device authorization flow codes
--   - Updates service_principals_view with client credentials

--------------------------------------------------------------------------------
-- 1) Add client credentials to service_principals_view
--------------------------------------------------------------------------------
-- These columns enable service principals to authenticate via client_id/client_secret
ALTER TABLE service_principals_view
    ADD COLUMN IF NOT EXISTS client_id TEXT,
    ADD COLUMN IF NOT EXISTS client_secret_hash TEXT;

-- client_id must be unique for lookups
CREATE UNIQUE INDEX IF NOT EXISTS idx_service_principals_client_id
    ON service_principals_view (client_id) WHERE client_id IS NOT NULL AND NOT is_deleted;

COMMENT ON COLUMN service_principals_view.client_id IS 'Unique client identifier for OAuth-style authentication';
COMMENT ON COLUMN service_principals_view.client_secret_hash IS 'Argon2id hash of client secret - secret shown only at creation';

--------------------------------------------------------------------------------
-- 2) access_tokens
--------------------------------------------------------------------------------
-- Short-lived bearer tokens for API authentication.
-- Tokens are stored hashed; the raw token is shown only at creation time.
CREATE TABLE IF NOT EXISTS access_tokens (
    token_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL,
    
    -- Subject identity
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'service_principal')),
    subject_id TEXT NOT NULL,
    
    -- For users, we store email for org membership lookups
    subject_email TEXT,
    
    -- Scopes granted to this token (subset of subject's allowed scopes)
    scopes JSONB NOT NULL DEFAULT '[]',
    
    -- Lifecycle
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    
    -- Audit
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    -- Optional: link to refresh token that created this access token
    refresh_token_id TEXT,
    
    -- Optional: device code that created this token (for device flow)
    device_code_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_access_tokens_hash
    ON access_tokens (token_hash);

CREATE INDEX IF NOT EXISTS idx_access_tokens_subject
    ON access_tokens (subject_type, subject_id);

CREATE INDEX IF NOT EXISTS idx_access_tokens_expires_at
    ON access_tokens (expires_at);

-- Partial index for non-revoked tokens (expiry checked at query time)
CREATE INDEX IF NOT EXISTS idx_access_tokens_active
    ON access_tokens (token_hash) 
    WHERE revoked_at IS NULL;

COMMENT ON TABLE access_tokens IS 'Short-lived bearer tokens for API authentication (15 min default)';
COMMENT ON COLUMN access_tokens.token_hash IS 'SHA-256 hash of the raw token for lookup';

--------------------------------------------------------------------------------
-- 3) refresh_tokens
--------------------------------------------------------------------------------
-- Longer-lived tokens used to obtain new access tokens.
-- Only used at /token/refresh and /token/revoke endpoints.
CREATE TABLE IF NOT EXISTS refresh_tokens (
    token_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL,
    
    -- Subject identity (same as access token it can mint)
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'service_principal')),
    subject_id TEXT NOT NULL,
    subject_email TEXT,
    
    -- Scopes (inherited by access tokens minted from this refresh token)
    scopes JSONB NOT NULL DEFAULT '[]',
    
    -- Lifecycle
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    
    -- Audit
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    -- Device code that created this token (for device flow)
    device_code_id TEXT,
    
    -- For refresh token rotation: previous token in chain
    previous_token_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_hash
    ON refresh_tokens (token_hash);

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_subject
    ON refresh_tokens (subject_type, subject_id);

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires_at
    ON refresh_tokens (expires_at);

-- Partial index for non-revoked tokens (expiry checked at query time)
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_active
    ON refresh_tokens (token_hash) 
    WHERE revoked_at IS NULL;

COMMENT ON TABLE refresh_tokens IS 'Longer-lived tokens for obtaining new access tokens (30 day default)';

--------------------------------------------------------------------------------
-- 4) device_codes
--------------------------------------------------------------------------------
-- Device authorization flow codes (RFC 8628).
-- Used for CLI login: CLI shows user_code, user approves in browser, CLI polls for token.
CREATE TABLE IF NOT EXISTS device_codes (
    device_code_id TEXT PRIMARY KEY,
    
    -- device_code is the secret the CLI uses to poll for tokens
    device_code_hash TEXT NOT NULL,
    
    -- user_code is the short code displayed to user (e.g., "ABCD-1234")
    user_code TEXT NOT NULL,
    
    -- Status of the authorization request
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'expired', 'consumed')),
    
    -- Once approved, these fields are populated
    approved_subject_type TEXT CHECK (approved_subject_type IN ('user', 'service_principal')),
    approved_subject_id TEXT,
    approved_subject_email TEXT,
    approved_scopes JSONB,
    approved_at TIMESTAMPTZ,
    
    -- Device metadata (for display in approval UI)
    device_name TEXT,
    
    -- Lifecycle
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    -- Rate limiting: track last poll time
    last_polled_at TIMESTAMPTZ,
    poll_count INT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_device_codes_hash
    ON device_codes (device_code_hash);

CREATE UNIQUE INDEX IF NOT EXISTS idx_device_codes_user_code
    ON device_codes (user_code) WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_device_codes_status
    ON device_codes (status);

CREATE INDEX IF NOT EXISTS idx_device_codes_expires_at
    ON device_codes (expires_at);

COMMENT ON TABLE device_codes IS 'Device authorization flow codes for CLI login (10 min default)';
COMMENT ON COLUMN device_codes.user_code IS 'Short code displayed to user (e.g., ABCD-1234)';
COMMENT ON COLUMN device_codes.device_code_hash IS 'SHA-256 hash of the device code secret';

--------------------------------------------------------------------------------
-- 5) Token cleanup function (optional, for scheduled maintenance)
--------------------------------------------------------------------------------
-- This function can be called periodically to clean up expired tokens.
-- Not called automatically - run via pg_cron or external scheduler.
CREATE OR REPLACE FUNCTION cleanup_expired_tokens()
RETURNS TABLE(
    access_tokens_deleted BIGINT,
    refresh_tokens_deleted BIGINT,
    device_codes_deleted BIGINT
) AS $$
DECLARE
    at_count BIGINT;
    rt_count BIGINT;
    dc_count BIGINT;
BEGIN
    -- Delete expired access tokens (keep 7 days after expiry for audit)
    DELETE FROM access_tokens 
    WHERE expires_at < now() - INTERVAL '7 days';
    GET DIAGNOSTICS at_count = ROW_COUNT;
    
    -- Delete expired refresh tokens (keep 7 days after expiry for audit)
    DELETE FROM refresh_tokens 
    WHERE expires_at < now() - INTERVAL '7 days';
    GET DIAGNOSTICS rt_count = ROW_COUNT;
    
    -- Delete expired device codes (keep 1 day after expiry)
    DELETE FROM device_codes 
    WHERE expires_at < now() - INTERVAL '1 day';
    GET DIAGNOSTICS dc_count = ROW_COUNT;
    
    RETURN QUERY SELECT at_count, rt_count, dc_count;
END;
$$ LANGUAGE plpgsql;

COMMENT ON FUNCTION cleanup_expired_tokens IS 'Cleanup expired auth tokens - run periodically via scheduler';
