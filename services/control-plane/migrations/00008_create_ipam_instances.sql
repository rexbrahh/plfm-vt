-- Migration: 00008_create_ipam_instances
-- Description: Instance IPv6 IPAM allocations (v1)

--------------------------------------------------------------------------------
-- Sequence for allocating instance IPv6 suffixes
--------------------------------------------------------------------------------
CREATE SEQUENCE IF NOT EXISTS ipam_instance_suffix_seq
    AS BIGINT
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--------------------------------------------------------------------------------
-- ipam_instances
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ipam_instances (
    instance_id TEXT PRIMARY KEY,
    ipv6_suffix BIGINT NOT NULL UNIQUE,
    overlay_ipv6 INET NOT NULL UNIQUE,
    allocated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ,
    cooldown_until TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_ipam_instances_overlay_ipv6
    ON ipam_instances (overlay_ipv6);

COMMENT ON TABLE ipam_instances IS 'Instance overlay IPv6 allocations (v1)';
COMMENT ON COLUMN ipam_instances.ipv6_suffix IS 'Sequential suffix allocated from ipam_instance_suffix_seq';
