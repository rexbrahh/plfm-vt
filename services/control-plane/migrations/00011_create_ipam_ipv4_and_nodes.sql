-- Migration: 00011_create_ipam_ipv4_and_nodes
-- Description: IPv4 add-on allocations and node overlay IPAM (v1)
-- See: docs/specs/networking/ipam.md, docs/specs/networking/ipv4-addon.md

--------------------------------------------------------------------------------
-- ipam_ipv4_pool: Operator-provided IPv4 addresses available for allocation
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ipam_ipv4_pool (
    ipv4_address INET PRIMARY KEY,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    removed_at TIMESTAMPTZ,
    is_available BOOLEAN NOT NULL DEFAULT true
);

CREATE INDEX IF NOT EXISTS idx_ipam_ipv4_pool_available
    ON ipam_ipv4_pool (is_available) WHERE is_available = true;

COMMENT ON TABLE ipam_ipv4_pool IS 'Operator-managed pool of allocatable public IPv4 addresses';

--------------------------------------------------------------------------------
-- ipam_ipv4_allocations: Per-environment dedicated IPv4 allocations
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ipam_ipv4_allocations (
    allocation_id TEXT PRIMARY KEY,
    env_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    ipv4_address INET NOT NULL,
    allocated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ,
    cooldown_until TIMESTAMPTZ,

    CONSTRAINT ipam_ipv4_allocations_active_env_unique
        UNIQUE (env_id) DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT ipam_ipv4_allocations_active_ip_unique
        UNIQUE (ipv4_address) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX IF NOT EXISTS idx_ipam_ipv4_allocations_org_id
    ON ipam_ipv4_allocations (org_id);

CREATE INDEX IF NOT EXISTS idx_ipam_ipv4_allocations_env_id
    ON ipam_ipv4_allocations (env_id);

CREATE INDEX IF NOT EXISTS idx_ipam_ipv4_allocations_cooldown
    ON ipam_ipv4_allocations (cooldown_until)
    WHERE cooldown_until IS NOT NULL;

COMMENT ON TABLE ipam_ipv4_allocations IS 'Dedicated IPv4 allocations per environment (paid add-on)';
COMMENT ON COLUMN ipam_ipv4_allocations.cooldown_until IS 'Address reuse blocked until this time (24h default)';

--------------------------------------------------------------------------------
-- ipam_nodes: Node overlay IPv6 allocations
--------------------------------------------------------------------------------
CREATE SEQUENCE IF NOT EXISTS ipam_node_suffix_seq
    AS BIGINT
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

CREATE TABLE IF NOT EXISTS ipam_nodes (
    node_id TEXT PRIMARY KEY,
    ipv6_suffix BIGINT NOT NULL UNIQUE,
    overlay_ipv6 INET NOT NULL UNIQUE,
    allocated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ,
    cooldown_until TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_ipam_nodes_overlay_ipv6
    ON ipam_nodes (overlay_ipv6);

CREATE INDEX IF NOT EXISTS idx_ipam_nodes_cooldown
    ON ipam_nodes (cooldown_until)
    WHERE cooldown_until IS NOT NULL;

COMMENT ON TABLE ipam_nodes IS 'Node overlay IPv6 allocations for WireGuard mesh';
COMMENT ON COLUMN ipam_nodes.cooldown_until IS 'Address reuse blocked until this time (30d recommended)';

--------------------------------------------------------------------------------
-- org_quotas: Per-org quota limits (operator-configurable overrides)
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS org_quotas (
    org_id TEXT NOT NULL,
    dimension TEXT NOT NULL,
    limit_value BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (org_id, dimension)
);

CREATE INDEX IF NOT EXISTS idx_org_quotas_org_id
    ON org_quotas (org_id);

COMMENT ON TABLE org_quotas IS 'Per-org quota limits (overrides tier defaults)';
COMMENT ON COLUMN org_quotas.dimension IS 'Quota dimension: max_instances, max_total_memory_bytes, max_ipv4_allocations, etc.';

-- env_networking checkpoint already pre-populated in 00002_create_projection_checkpoints.sql
