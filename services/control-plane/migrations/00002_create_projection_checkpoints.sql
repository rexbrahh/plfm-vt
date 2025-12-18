-- Migration: 00002_create_projection_checkpoints
-- Description: Create projection checkpoint table for event consumption tracking
-- See: docs/specs/state/materialized-views.md

-- Each projection maintains a durable checkpoint of the last applied event_id.
-- On startup, projections resume from (last_applied_event_id + 1).

CREATE TABLE IF NOT EXISTS projection_checkpoints (
    projection_name TEXT PRIMARY KEY,
    last_applied_event_id BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Pre-populate checkpoints for all known projections
INSERT INTO projection_checkpoints (projection_name, last_applied_event_id, updated_at)
VALUES
    ('orgs', 0, now()),
    ('org_members', 0, now()),
    ('service_principals', 0, now()),
    ('apps', 0, now()),
    ('envs', 0, now()),
    ('releases', 0, now()),
    ('deploys', 0, now()),
    ('env_desired_releases', 0, now()),
    ('env_scale', 0, now()),
    ('env_networking', 0, now()),
    ('routes', 0, now()),
    ('secret_bundles', 0, now()),
    ('volumes', 0, now()),
    ('volume_attachments', 0, now()),
    ('snapshots', 0, now()),
    ('restore_jobs', 0, now()),
    ('instances_desired', 0, now()),
    ('instances_status', 0, now()),
    ('exec_sessions', 0, now()),
    ('nodes', 0, now())
ON CONFLICT (projection_name) DO NOTHING;

COMMENT ON TABLE projection_checkpoints IS 'Tracks last processed event_id per projection for resumable consumption';
COMMENT ON COLUMN projection_checkpoints.last_applied_event_id IS 'Event ID of the last fully applied event (resume from this + 1)';
