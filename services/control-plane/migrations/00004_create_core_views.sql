-- Migration: 00004_create_core_views
-- Description: Create core materialized view tables
-- See: docs/specs/state/materialized-views.md

-- These are the primary projection tables that materialize event log state.
-- Each table has:
--   - resource_version for optimistic concurrency
--   - created_at, updated_at for operational visibility
--   - is_deleted (or deleted_at) for soft deletes where applicable

--------------------------------------------------------------------------------
-- 1) orgs_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS orgs_view (
    org_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE orgs_view IS 'Materialized view of organizations (from org.created, org.updated events)';

--------------------------------------------------------------------------------
-- 2) org_members_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS org_members_view (
    member_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_org_members_org_email
    ON org_members_view (org_id, email) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_org_members_org_id
    ON org_members_view (org_id) WHERE NOT is_deleted;

COMMENT ON TABLE org_members_view IS 'Materialized view of org membership (from org_member.* events)';

--------------------------------------------------------------------------------
-- 3) service_principals_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS service_principals_view (
    service_principal_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    name TEXT NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]',
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS idx_service_principals_org_id
    ON service_principals_view (org_id) WHERE NOT is_deleted;

COMMENT ON TABLE service_principals_view IS 'Materialized view of service principals (from service_principal.* events)';

--------------------------------------------------------------------------------
-- 4) apps_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS apps_view (
    app_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_apps_org_name
    ON apps_view (org_id, name) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_apps_org_id
    ON apps_view (org_id) WHERE NOT is_deleted;

COMMENT ON TABLE apps_view IS 'Materialized view of apps (from app.* events)';

--------------------------------------------------------------------------------
-- 5) envs_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS envs_view (
    env_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    name TEXT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_envs_app_name
    ON envs_view (app_id, name) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_envs_org_id
    ON envs_view (org_id) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_envs_app_id
    ON envs_view (app_id) WHERE NOT is_deleted;

COMMENT ON TABLE envs_view IS 'Materialized view of environments (from env.* events)';

--------------------------------------------------------------------------------
-- 6) releases_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS releases_view (
    release_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    image_ref TEXT NOT NULL,
    index_or_manifest_digest TEXT NOT NULL,
    resolved_digests JSONB NOT NULL DEFAULT '{}',
    manifest_schema_version INT NOT NULL,
    manifest_hash TEXT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_releases_org_id
    ON releases_view (org_id);

CREATE INDEX IF NOT EXISTS idx_releases_app_id
    ON releases_view (app_id);

COMMENT ON TABLE releases_view IS 'Materialized view of releases (immutable, from release.created events)';

--------------------------------------------------------------------------------
-- 7) deploys_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS deploys_view (
    deploy_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('deploy', 'rollback')),
    release_id TEXT NOT NULL,
    process_types JSONB NOT NULL DEFAULT '[]',
    status TEXT NOT NULL,
    message TEXT,
    failed_reason TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_deploys_org_id
    ON deploys_view (org_id);

CREATE INDEX IF NOT EXISTS idx_deploys_env_id
    ON deploys_view (env_id);

CREATE INDEX IF NOT EXISTS idx_deploys_status
    ON deploys_view (status);

COMMENT ON TABLE deploys_view IS 'Materialized view of deploys (from deploy.* events)';

--------------------------------------------------------------------------------
-- 8) env_desired_releases_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS env_desired_releases_view (
    env_id TEXT NOT NULL,
    process_type TEXT NOT NULL,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    release_id TEXT NOT NULL,
    deploy_id TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    PRIMARY KEY (env_id, process_type)
);

CREATE INDEX IF NOT EXISTS idx_env_desired_releases_org_id
    ON env_desired_releases_view (org_id);

COMMENT ON TABLE env_desired_releases_view IS 'Desired release per (env_id, process_type) - scheduler input';

--------------------------------------------------------------------------------
-- 9) env_scale_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS env_scale_view (
    env_id TEXT NOT NULL,
    process_type TEXT NOT NULL,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    desired_replicas INT NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    
    PRIMARY KEY (env_id, process_type)
);

CREATE INDEX IF NOT EXISTS idx_env_scale_org_id
    ON env_scale_view (org_id);

COMMENT ON TABLE env_scale_view IS 'Desired replica count per (env_id, process_type)';

--------------------------------------------------------------------------------
-- 10) env_networking_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS env_networking_view (
    env_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    ipv4_enabled BOOLEAN NOT NULL DEFAULT false,
    ipv4_address INET,
    ipv4_allocation_id TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_env_networking_org_id
    ON env_networking_view (org_id);

COMMENT ON TABLE env_networking_view IS 'Env-level networking state (IPv4 add-on)';

--------------------------------------------------------------------------------
-- 11) routes_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS routes_view (
    route_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    hostname TEXT NOT NULL,
    listen_port INT NOT NULL,
    protocol_hint TEXT,
    backend_process_type TEXT NOT NULL,
    backend_port INT NOT NULL,
    proxy_protocol BOOLEAN NOT NULL DEFAULT false,
    ipv4_required BOOLEAN NOT NULL DEFAULT false,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_routes_hostname
    ON routes_view (hostname) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_routes_org_id
    ON routes_view (org_id) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_routes_env_id
    ON routes_view (env_id) WHERE NOT is_deleted;

COMMENT ON TABLE routes_view IS 'Materialized view of routes (from route.* events)';

--------------------------------------------------------------------------------
-- 12) secret_bundles_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS secret_bundles_view (
    bundle_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    format TEXT NOT NULL DEFAULT 'platform_env_v1',
    current_version_id TEXT,
    current_data_hash TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_secret_bundles_env_id
    ON secret_bundles_view (env_id);

CREATE INDEX IF NOT EXISTS idx_secret_bundles_org_id
    ON secret_bundles_view (org_id);

COMMENT ON TABLE secret_bundles_view IS 'Secret bundle metadata (from secret_bundle.* events) - NO secret material';

--------------------------------------------------------------------------------
-- 13) volumes_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS volumes_view (
    volume_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    name TEXT,
    size_bytes BIGINT NOT NULL,
    filesystem TEXT NOT NULL,
    backup_enabled BOOLEAN NOT NULL DEFAULT false,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS idx_volumes_org_id
    ON volumes_view (org_id) WHERE NOT is_deleted;

COMMENT ON TABLE volumes_view IS 'Materialized view of volumes (from volume.* events)';

--------------------------------------------------------------------------------
-- 14) volume_attachments_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS volume_attachments_view (
    attachment_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    volume_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    process_type TEXT NOT NULL,
    mount_path TEXT NOT NULL,
    read_only BOOLEAN NOT NULL DEFAULT false,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted BOOLEAN NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_volume_attachments_unique
    ON volume_attachments_view (env_id, process_type, mount_path) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_volume_attachments_org_id
    ON volume_attachments_view (org_id) WHERE NOT is_deleted;

CREATE INDEX IF NOT EXISTS idx_volume_attachments_volume_id
    ON volume_attachments_view (volume_id) WHERE NOT is_deleted;

COMMENT ON TABLE volume_attachments_view IS 'Materialized view of volume attachments (from volume_attachment.* events)';

--------------------------------------------------------------------------------
-- 15) snapshots_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS snapshots_view (
    snapshot_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    volume_id TEXT NOT NULL,
    status TEXT NOT NULL,
    size_bytes BIGINT,
    note TEXT,
    failed_reason TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_snapshots_org_id
    ON snapshots_view (org_id);

CREATE INDEX IF NOT EXISTS idx_snapshots_volume_id
    ON snapshots_view (volume_id);

COMMENT ON TABLE snapshots_view IS 'Materialized view of snapshots (from snapshot.* events)';

--------------------------------------------------------------------------------
-- 16) restore_jobs_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS restore_jobs_view (
    restore_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    source_volume_id TEXT NOT NULL,
    status TEXT NOT NULL,
    new_volume_id TEXT,
    failed_reason TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_restore_jobs_org_id
    ON restore_jobs_view (org_id);

COMMENT ON TABLE restore_jobs_view IS 'Materialized view of restore jobs (from restore_job.* events)';

--------------------------------------------------------------------------------
-- 17) instances_desired_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS instances_desired_view (
    instance_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    process_type TEXT NOT NULL,
    node_id TEXT NOT NULL,
    desired_state TEXT NOT NULL CHECK (desired_state IN ('running', 'draining', 'stopped')),
    release_id TEXT NOT NULL,
    secrets_version_id TEXT,
    overlay_ipv6 INET NOT NULL,
    resources_snapshot JSONB NOT NULL DEFAULT '{}',
    spec_hash TEXT NOT NULL,
    generation INT NOT NULL DEFAULT 1,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_instances_desired_org_id
    ON instances_desired_view (org_id);

CREATE INDEX IF NOT EXISTS idx_instances_desired_env_id
    ON instances_desired_view (env_id);

CREATE INDEX IF NOT EXISTS idx_instances_desired_node_id
    ON instances_desired_view (node_id);

CREATE INDEX IF NOT EXISTS idx_instances_desired_state
    ON instances_desired_view (desired_state);

COMMENT ON TABLE instances_desired_view IS 'Desired instances (scheduler output, from instance.allocated, instance.desired_state_changed)';

--------------------------------------------------------------------------------
-- 18) instances_status_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS instances_status_view (
    instance_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    status TEXT NOT NULL,
    boot_id TEXT,
    microvm_id TEXT,
    exit_code INT,
    reason_code TEXT,
    reason_detail TEXT,
    reported_at TIMESTAMPTZ NOT NULL,
    resource_version INT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_instances_status_org_id
    ON instances_status_view (org_id);

CREATE INDEX IF NOT EXISTS idx_instances_status_env_id
    ON instances_status_view (env_id);

CREATE INDEX IF NOT EXISTS idx_instances_status_node_id
    ON instances_status_view (node_id);

CREATE INDEX IF NOT EXISTS idx_instances_status_status
    ON instances_status_view (status);

COMMENT ON TABLE instances_status_view IS 'Latest reported runtime status per instance (from instance.status_changed)';

--------------------------------------------------------------------------------
-- 19) exec_sessions_view
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS exec_sessions_view (
    exec_session_id TEXT PRIMARY KEY,
    org_id TEXT NOT NULL,
    env_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    requested_command JSONB NOT NULL,
    tty BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL CHECK (status IN ('granted', 'connected', 'ended')),
    expires_at TIMESTAMPTZ NOT NULL,
    connected_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    exit_code INT,
    end_reason TEXT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_exec_sessions_org_id
    ON exec_sessions_view (org_id);

CREATE INDEX IF NOT EXISTS idx_exec_sessions_instance_id
    ON exec_sessions_view (instance_id);

COMMENT ON TABLE exec_sessions_view IS 'Exec session metadata for auditing (from exec_session.* events)';

--------------------------------------------------------------------------------
-- 20) nodes_view (infrastructure, not tenant-readable by default)
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS nodes_view (
    node_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('active', 'draining', 'disabled', 'degraded', 'offline')),
    wireguard_public_key TEXT NOT NULL,
    agent_mtls_subject TEXT NOT NULL,
    public_ipv6 INET,
    public_ipv4 INET,
    labels JSONB NOT NULL DEFAULT '{}',
    allocatable JSONB NOT NULL DEFAULT '{}',
    mtu INT,
    resource_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_nodes_state
    ON nodes_view (state);

COMMENT ON TABLE nodes_view IS 'Node metadata and state (infrastructure, from node.* events)';
