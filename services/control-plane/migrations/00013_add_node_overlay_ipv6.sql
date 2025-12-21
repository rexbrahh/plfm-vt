-- Description: Add overlay IPv6 to nodes_view
-- See: docs/specs/networking/ipam.md

ALTER TABLE nodes_view
    ADD COLUMN IF NOT EXISTS overlay_ipv6 INET;

CREATE INDEX IF NOT EXISTS idx_nodes_overlay_ipv6
    ON nodes_view (overlay_ipv6);

COMMENT ON COLUMN nodes_view.overlay_ipv6 IS 'Node overlay IPv6 address (/128)';

UPDATE nodes_view n
SET overlay_ipv6 = i.overlay_ipv6
FROM ipam_nodes i
WHERE n.node_id = i.node_id
  AND n.overlay_ipv6 IS NULL;
