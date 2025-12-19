-- Migration: 00005_create_projects_view
-- Description: Create projects_view materialized table
-- See: docs/specs/state/materialized-views.md

CREATE TABLE IF NOT EXISTS projects_view (
    project_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    name TEXT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_org_name
    ON projects_view (org_id, name) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_projects_org_id
    ON projects_view (org_id) WHERE NOT is_deleted;

COMMENT ON TABLE projects_view IS 'Materialized view of projects (from project.* events)';
