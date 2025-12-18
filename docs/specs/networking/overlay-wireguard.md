# docs/specs/networking/overlay-wireguard.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the platform overlay network built on WireGuard:
- interface and addressing expectations
- peer config structure and AllowedIPs rules
- node enrollment and membership distribution
- key rotation and revocation
- update propagation and safety (avoid partitions)
- MTU policy and ICMPv6 requirements

Locked decision: overlay is WireGuard full mesh in v1. See `docs/ADRs/0004-overlay-wireguard-full-mesh.md`.  
Locked decision: IPv6-first. See `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`.

## Scope
This spec defines overlay behavior between nodes (hosts and edge nodes).

This spec does not define:
- per-instance guest networking (see `docs/specs/runtime/networking-inside-vm.md`)
- IP allocation strategy details (see `docs/specs/networking/ipam.md`)
- ingress routing rules (see `docs/specs/networking/ingress-l4.md`)

## Definitions
- **Node**: a host running the node agent (includes worker nodes and edge nodes).
- **Overlay**: encrypted node-to-node network built using WireGuard.
- **Underlay**: public network connectivity between nodes.
- **wg interface**: WireGuard network interface, v1 name is `wg0`.
- **AllowedIPs**: WireGuard routing filter that defines what destination prefixes a peer is allowed to receive.

## High-level overlay contract (v1)
1) Every node runs WireGuard interface `wg0`.
2) Topology is full mesh:
- every node has a peer entry for every other node
- no transit gateway is required in v1

3) Overlay is IPv6-first:
- each node has one or more stable IPv6 addresses on wg0
- overlay traffic uses IPv6 by default

4) Overlay membership is control-plane-managed:
- nodes do not self-assign AllowedIPs
- nodes do not add peers autonomously
- the control plane issues membership state and changes

5) MTU is standardized:
- default MTU is 1420
- the platform must not break ICMPv6 Packet Too Big, or IPv6 PMTUD will fail

## Interface configuration

### Interface name
- `wg0`

### Listen port
- Default: 51820/udp
- Operator can override per cluster, but it must be consistent across all nodes.

### Node overlay addresses
- Each node is assigned:
  - `node_overlay_ipv6` (single /128 on wg0)
- Optional future:
  - a node prefix for routing instance addresses through the node. v1 recommendation is not required.

### MTU
- Default MTU: 1420
- Allowed range: 1280..9000
- MTU must be consistent across nodes unless a cluster-wide override is applied.

## AllowedIPs rules (normative)
AllowedIPs is the primary safety mechanism for preventing routing spoofing.

### What AllowedIPs must include
For each peer node P, a node N configures:

- AllowedIPs for peer P includes:
  - `P.node_overlay_ipv6/128`

Optionally, if you later route per-instance prefixes via nodes:
- AllowedIPs may include a per-node instance prefix:
  - `P.instance_prefix/64` or `/80` (example)
This is deferred to IPAM and routing design. Do not add it without updating those specs.

### What AllowedIPs must not include
- `::/0` must never be set for any peer.
- Another node’s overlay /128 must never appear under the wrong peer.
- Tenant instance addresses must not be globally routed via the overlay without explicit IPAM rules.

## Peer endpoints
Each peer entry includes:
- `PublicKey`
- `Endpoint` (underlay IP:port)
- `AllowedIPs` (as above)
- `PersistentKeepalive` (optional)

Endpoint selection (v1):
- Prefer public IPv6 endpoint if available.
- Otherwise use public IPv4 endpoint.

PersistentKeepalive guidance:
- If a node is behind NAT (less likely for dedicated servers), set keepalive to 25 seconds.
- If nodes are on public routable addresses with no NAT, keepalive can be omitted.

## Enrollment and membership distribution

### Enrollment goals
- A random internet host must not be able to join the overlay.
- A node must have a stable identity tied to:
  - an mTLS identity for control plane RPC
  - a WireGuard public key for overlay membership
- Enrollment must be auditable and revocable.

### Enrollment flow (v1 recommendation)
1) Operator generates a short-lived, single-use `enrollment_token`.
2) Node agent generates:
- WireGuard keypair
- mTLS keypair (or CSR)
3) Node agent calls control plane `EnrollNode` over TLS:
- presents enrollment_token
- sends WireGuard public key
- sends mTLS CSR or public key
- sends underlay endpoint hints (public IPv6/IPv4, optional)

4) Control plane responds with:
- `node_id`
- signed mTLS cert (or instructions to fetch)
- assigned `node_overlay_ipv6`
- cluster overlay parameters (listen port, MTU)
- peer set (list of other nodes with public keys, endpoints, AllowedIPs)

5) Node agent applies config and brings wg0 up.

Audit requirements:
- `node.enrolled` event must be recorded with:
  - node_id
  - wireguard public key fingerprint
  - operator identity (actor_id)
  - timestamp

## Config distribution model
Nodes must receive overlay config updates reliably.

### Desired model
- Control plane is the source of truth for the peer set.
- Node agent consumes overlay updates by:
  - event stream (preferred)
  - or polling

### Update atomicity requirement
Peer updates must avoid splitting the mesh.

Rule:
- Node agent applies peer set updates in a single reconciliation step:
  - compute desired peer set
  - add missing peers
  - update endpoints and AllowedIPs
  - remove revoked peers

This process must be idempotent.

### Drift correction
Nodes must periodically reconcile local wg0 config against desired state even if no new events arrive.

Recommended interval:
- every 30 seconds or 60 seconds

## Key rotation

### Rotation goals
- Support periodic key rotation without full cluster downtime.
- Support emergency rotation if a key is compromised.
- Keep membership changes auditable.

### Rotation procedure (safe, v1)
For node X rotating its key:
1) Node generates a new WireGuard keypair.
2) Node sends `RotateWireGuardKey` request to control plane using mTLS identity.
3) Control plane records rotation intent and returns approval (may be automatic).
4) Control plane updates desired peer sets for all nodes:
- for peer X, replace PublicKey with new key
- keep AllowedIPs unchanged
- update a `key_generation` number for debugging

5) Nodes reconcile config. After a grace period, old key is invalid everywhere.

Important:
- Do not allow two active keys for the same AllowedIPs simultaneously unless you have explicit support for it. In v1, prefer a coordinated cutover.

Audit:
- `node.key_rotated` (if you add it) or `node.enrolled` style event with rotation metadata.

### Emergency revocation
If a node is compromised:
1) Operator sets node state to `disabled`.
2) Control plane removes the node from peer sets.
3) Nodes remove the peer entry for the compromised node.
4) Scheduler stops placing new instances on that node.
5) Incident tooling triggers:
- route backend removal
- instance rescheduling where possible

This must be a runbook.

## Revocation and removal
When a node is decommissioned:
- Remove it from peer sets and revoke its certs.
- Release its overlay address allocation only after it is fully removed from all peers, or keep it reserved for a cooling-off period.

## MTU and ICMPv6 requirements

### MTU policy
- Default MTU: 1420
- MTU is cluster-wide, not per-node.
- If underlay requires smaller MTU, lower MTU cluster-wide.

### ICMPv6 Packet Too Big
- Nodes must not block ICMPv6 Packet Too Big on paths relevant to overlay traffic.
- If ICMPv6 is blocked, PMTUD breaks and you will see random hangs and stalls.

This is an operator requirement. Document it in host firewall defaults.

## Observability requirements
Nodes and control plane must expose:
- peer count
- handshake age per peer
- bytes sent/received per peer
- error counters for config apply
- node overlay reachability checks (optional)

Alerting (minimum):
- node has no successful handshakes with any peers for N minutes
- sudden peer count drop
- repeated key rotation failures

## Failure behavior
### Underlay outage
- Overlay handshakes fail.
- Node is effectively partitioned.
- Scheduler should mark node as unschedulable when control plane cannot reach it.

### Partial mesh partition
- Some peers are reachable, others are not.
- Edge must remove unreachable backend instances from routing sets.

### Control plane down
- Overlay continues operating on last applied peer set.
- No membership changes are possible until control plane returns.

## Compliance tests (required)
Automated or semi-automated tests must verify:
1) Two nodes can establish a handshake and exchange IPv6 packets over wg0.
2) AllowedIPs prevent spoofing:
- node A cannot route traffic to node C through node B unless explicitly allowed
3) MTU defaults do not break basic connectivity.
4) Removing a peer from desired set results in local peer removal within bounded time.
5) Key rotation results in successful handshakes with the new key and no handshakes with the old key after cutover.

## Open questions (explicitly deferred)
- Transition from full mesh to hub topology (planned v2). This requires a new spec and likely an ADR update.
- Whether to route per-instance prefixes via nodes (requires IPAM spec alignment).
