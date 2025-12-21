-- Migration: 00012_add_command_to_releases
-- Description: Add command column to releases_view for workload entrypoint
-- See: docs/specs/manifest/workload-spec.md (command field spec)

-- Add command column to releases_view
-- The command is an array of strings representing the fully resolved entrypoint
-- Stored as JSONB for flexibility and proper array handling
ALTER TABLE releases_view
    ADD COLUMN IF NOT EXISTS command JSONB NOT NULL DEFAULT '[]';

COMMENT ON COLUMN releases_view.command IS 'Fully resolved entrypoint command (array of strings from manifest)';
