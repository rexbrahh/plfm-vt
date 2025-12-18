-- Migration: 00001_create_events_table
-- Description: Create the append-only event log table
-- See: docs/specs/state/event-log.md

-- The events table is the source of truth for all state changes.
-- It is append-only: no UPDATE or DELETE operations are permitted.

CREATE TABLE IF NOT EXISTS events (
    -- Global ordering
    event_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Aggregate identification and ordering
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    aggregate_seq INT NOT NULL,

    -- Event type and version
    event_type TEXT NOT NULL,
    event_version INT NOT NULL DEFAULT 1,

    -- Actor (who triggered this event)
    actor_type TEXT NOT NULL CHECK (actor_type IN ('user', 'service_principal', 'system')),
    actor_id TEXT NOT NULL,

    -- Tenant scope (required for tenant aggregates)
    org_id TEXT,

    -- Request correlation
    request_id TEXT NOT NULL,
    idempotency_key TEXT,

    -- Optional context
    app_id TEXT,
    env_id TEXT,
    correlation_id TEXT,
    causation_id BIGINT REFERENCES events(event_id),

    -- Event-specific data (MUST NOT contain secrets)
    payload JSONB NOT NULL,

    -- Enforce aggregate ordering: only one event can have a given seq for an aggregate
    CONSTRAINT events_aggregate_seq_unique UNIQUE (aggregate_type, aggregate_id, aggregate_seq)
);

-- Index for latest-by-aggregate queries
CREATE INDEX IF NOT EXISTS idx_events_aggregate_latest
    ON events (aggregate_type, aggregate_id, aggregate_seq DESC);

-- Index for org-scoped tail queries
CREATE INDEX IF NOT EXISTS idx_events_org_id
    ON events (org_id, event_id) WHERE org_id IS NOT NULL;

-- Index for filtering by event type
CREATE INDEX IF NOT EXISTS idx_events_event_type
    ON events (event_type, event_id);

-- Index for idempotency lookups
CREATE INDEX IF NOT EXISTS idx_events_idempotency
    ON events (org_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

-- Trigger to prevent updates and deletes (defense in depth)
CREATE OR REPLACE FUNCTION prevent_event_mutation()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Events table is append-only. UPDATE and DELETE operations are not permitted.';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_immutable_trigger
    BEFORE UPDATE OR DELETE ON events
    FOR EACH ROW
    EXECUTE FUNCTION prevent_event_mutation();

COMMENT ON TABLE events IS 'Append-only event log - source of truth for all state changes';
COMMENT ON COLUMN events.event_id IS 'Globally monotonic event identifier (total order)';
COMMENT ON COLUMN events.aggregate_seq IS 'Monotonic sequence within the aggregate (starts at 1)';
COMMENT ON COLUMN events.payload IS 'Event-specific data - MUST NOT contain secret values';
