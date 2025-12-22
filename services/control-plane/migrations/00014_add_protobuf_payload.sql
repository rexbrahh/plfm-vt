-- Migration: 00014_add_protobuf_payload
-- Description: Add protobuf payload storage for wire-format alignment
-- See: docs/specs/wire-formats/wire-formats.md

-- Add columns for protobuf-encoded payloads per EventEnvelope spec.
-- During migration, both payload (JSON) and payload_bytes (protobuf) may be populated.
-- New events SHOULD populate payload_bytes; payload remains for backward compatibility.

-- Payload type URL (fully qualified protobuf message type)
-- Example: "type.googleapis.com/plfm.events.v1.OrgCreatedPayload"
ALTER TABLE events ADD COLUMN IF NOT EXISTS payload_type_url TEXT;

-- Protobuf-encoded payload bytes
ALTER TABLE events ADD COLUMN IF NOT EXISTS payload_bytes BYTEA;

-- Schema version for this event's payload (for evolution tracking)
-- This is separate from event_version which tracks the event type version
ALTER TABLE events ADD COLUMN IF NOT EXISTS payload_schema_version INT DEFAULT 1;

-- Traceparent for distributed tracing (W3C Trace Context)
ALTER TABLE events ADD COLUMN IF NOT EXISTS traceparent TEXT;

-- Additional tags for flexible metadata
ALTER TABLE events ADD COLUMN IF NOT EXISTS tags JSONB DEFAULT '{}';

-- Index for querying by payload type (useful for schema migrations)
CREATE INDEX IF NOT EXISTS idx_events_payload_type_url
    ON events (payload_type_url, event_id) WHERE payload_type_url IS NOT NULL;

COMMENT ON COLUMN events.payload_type_url IS 'Fully qualified protobuf message type URL for payload_bytes';
COMMENT ON COLUMN events.payload_bytes IS 'Protobuf-encoded event payload (preferred over JSON payload)';
COMMENT ON COLUMN events.payload_schema_version IS 'Schema version of the payload structure';
COMMENT ON COLUMN events.traceparent IS 'W3C Trace Context traceparent header for distributed tracing';
COMMENT ON COLUMN events.tags IS 'Additional key-value metadata tags';
