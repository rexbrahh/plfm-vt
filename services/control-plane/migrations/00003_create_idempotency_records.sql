-- Migration: 00003_create_idempotency_records
-- Description: Create idempotency records table for command deduplication
-- See: docs/specs/state/event-log.md (lines 129-155)

-- Idempotency records store the response for previously executed commands.
-- If a command is retried with the same idempotency key, we return the stored response.
-- If the key is reused with a different request, we return 409 Conflict.

CREATE TABLE IF NOT EXISTS idempotency_records (
    -- Composite key: (org_id, actor_id, endpoint_name, idempotency_key)
    org_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    endpoint_name TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    
    -- Request fingerprint for conflict detection
    request_hash TEXT NOT NULL,
    
    -- Stored response for replay
    response_status_code INT NOT NULL,
    response_body JSONB,
    
    -- Tracking
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    PRIMARY KEY (org_id, actor_id, endpoint_name, idempotency_key)
);

-- Index for cleanup of old records (retention: minimum 24 hours per spec)
CREATE INDEX IF NOT EXISTS idx_idempotency_records_created_at
    ON idempotency_records (created_at);

COMMENT ON TABLE idempotency_records IS 'Stores command responses for idempotency - minimum 24h retention';
COMMENT ON COLUMN idempotency_records.request_hash IS 'Hash of normalized request body for conflict detection';
COMMENT ON COLUMN idempotency_records.response_body IS 'Cached response to return on retry with same key';
