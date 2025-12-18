# docs/specs/networking/ingress-l4.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the L4 ingress behavior:
- TCP routing rules
- SNI inspection constraints for TLS passthrough routing
- port exposure and allocation rules
- backend selection and health gating
- PROXY protocol v2 injection behavior (mechanics only, wire format is separate)

Locked decisions:
- Ingress is L4-first with SNI passthrough by default: `docs/ADRs/0008-ingress-l4-sni-passthrough-first.md`
- IPv6-first, IPv4 is a paid add-on: `docs/ADRs/0007-network-ipv6-first-ipv4-paid.md`
- PROXY protocol v2 for client source identity (opt-in): `docs/ADRs/0009-proxy-protocol-v2-client-ip.md`

## Scope
This spec defines edge routing semantics for inbound connections.

This spec does not define:
- L7 routing or HTTP features (see `docs/specs/networking/ingress-l7.md`, optional later)
- overlay mechanics and peer config (see `docs/specs/networking/overlay-wireguard.md`)
- IP allocation strategy (see `docs/specs/networking/ipam.md`)
- guest networking config (see `docs/specs/runtime/networking-inside-vm.md`)
- PROXY protocol v2 byte-level format (see `docs/specs/networking/proxy-protocol-v2.md`)

## Definitions
- **Edge**: platform-controlled ingress nodes that accept external connections.
- **Listener**: a bound (public_ip, port) socket on an edge node that accepts TCP connections.
- **Route**: a control-plane object binding a hostname and listener port to an environment and backend target.
- **Backend**: a selected workload instance endpoint (overlay_ipv6 + port) to which the edge proxies the connection.
- **TLS passthrough**: edge does not terminate TLS; it only inspects ClientHello to read SNI for routing.
- **SNI**: Server Name Indication from the TLS ClientHello. Used for hostname routing.
- **Raw TCP**: protocols where edge does not parse payload. Routing is by (ip, port) only.
- **PROXY v2**: optional binary header prepended by edge to convey true client source ip and port.

## High-level contract (v1)
1) Ingress is TCP proxying at Layer 4.
2) For TLS passthrough routes, edge may inspect the TLS ClientHello to obtain SNI, without terminating TLS.
3) For raw TCP routes, edge does not inspect payload.
4) Routing is per-connection (not per-request).
5) Backend selection is based on control plane desired state and health gating.
6) IPv6 is the default public reachability. IPv4 requires an explicit add-on allocation.

## Routing inputs
Routing decisions may use only:
- destination listener port
- destination listener address (public IPv6 or dedicated IPv4 when enabled)
- for TLS passthrough: SNI extracted from ClientHello (best effort)

Routing must not depend on:
- HTTP headers (v1)
- TLS certificate contents (v1)
- application payload inspection (v1)

## Route types
Each Route declares a `protocol_hint`:

### 1) `tls_passthrough`
- Client connects to listener (typically 443).
- Edge attempts SNI inspection.
- Edge selects backend based on hostname match and proxies the TCP stream.

### 2) `tcp_raw`
- Client connects to listener (explicit port allocation).
- Edge does not inspect payload.
- Edge routes by listener (ip, port) binding to the backend set.

## Hostname normalization and matching
### Canonicalization (normative)
When the control plane stores and the edge matches hostnames:
- normalize to lower-case
- trim trailing dot
- apply IDNA / punycode normalization for international domain names
- reject invalid DNS name forms

### Matching rules (v1)
- Exact match only.
- No wildcard matching in v1.
- No regex matching.

### Uniqueness (v1 recommendation)
- Hostname must be globally unique across the platform for active routes.
- Attempting to create a route for an already-bound hostname fails with `409 conflict`.

Reason:
- prevents ambiguous routing and tenant hijack risk.

## SNI inspection behavior (tls_passthrough)
### What is allowed
- Read the first bytes of the TCP stream to parse a TLS ClientHello and extract SNI.

### What is not allowed
- Terminating TLS.
- Presenting certificates.
- Modifying TLS payload bytes (except optional PROXY v2 prefix when enabled for the route).

### Sniffing limits (normative)
Edge must implement bounded sniffing:
- **sniff_timeout_ms**: 200 ms default, configurable cluster-wide
- **max_sniff_bytes**: 8192 bytes default, configurable cluster-wide

If SNI is not obtained within these bounds, treat as “SNI unavailable”.

### Non-TLS on a TLS listener
If `protocol_hint=tls_passthrough` and the first bytes are not a TLS ClientHello:
- v1 default behavior: close the connection.
- v1 optional behavior (only if unambiguous): if the listener has exactly one route bound to that (ip, port) and the route explicitly allows non-TLS fallback, route without SNI. This must be explicit per route to avoid accidental hijacks.

### Clients without SNI
If SNI is unavailable:
- If routing would be ambiguous, close the connection.
- If routing is unambiguous because the listener has exactly one possible backend mapping, route to it.

v1 stance:
- We do not provide a general “default backend” per shared listener.
- We do not guess.

### Encrypted ClientHello (ECH)
If the client uses ECH, SNI may be hidden.
- In that case, SNI is effectively unavailable.
- The same “clients without SNI” rules apply.

## Listener binding and exposure rules
### Default IPv6 exposure
- Edge nodes listen on their public IPv6 addresses for:
  - `tcp/443` (default)
  - `tcp/80` (optional)

Multiple tenants share these listeners. Hostname routing disambiguates.

### Dedicated IPv4 exposure (paid add-on)
- When env has IPv4 add-on enabled, edge binds that env’s dedicated IPv4 on allowed ports.
- Raw TCP exposure that requires IPv4 is only available through this add-on.

Allocation unit (v1 stance):
- dedicated IPv4 is allocated per environment.

## Port allocation policy (v1)
### Allowed ports (baseline)
- 443 and 80 are allowed on IPv6 by default.
- Additional TCP ports are allowed only when explicitly requested and pass policy checks.

### Denylist (operator policy)
The platform should denylist ports that cause abuse or operational risk unless intentionally supported.
Example denylist candidates:
- 25 (SMTP)
- 23 (telnet)
- 137-139 (NetBIOS)
- any other operator-defined set

This list is operator-defined but must be documented and enforced consistently.

### Raw TCP port exposure
- Raw TCP ports must be explicitly requested via Route objects (or a dedicated port binding resource if you later separate it).
- The platform must record ownership and audit events for each port binding.
- The platform must prevent conflicting bindings on the same listener address.

## Backend sets and selection
### Backend identity
A backend is:
- `overlay_ipv6` of an instance
- `backend_port` inside the microVM

The backend_port must be declared in the process type’s manifest port declarations.

### Backend set derivation (v1 default)
The control plane derives the backend set:
- only instances with `status=ready` are included
- instances are selected by `(env_id, backend_process_type)`

The edge consumes backend sets as part of routing configuration updates.
This prevents edge from needing to query agents directly in v1.

### Health gating
A backend is eligible only if:
- instance status is ready in control plane view
- optional edge-side TCP probe (if enabled) succeeds

v1 recommendation:
- rely primarily on control plane readiness to avoid tight coupling.
- edge-side probes are an optional optimization and must not contradict control plane. If edge probe fails, edge may temporarily remove that backend locally until it becomes reachable again.

### Load balancing strategy (v1)
- Round-robin among eligible backends.
- Optional: consistent hashing by 5-tuple for connection stickiness (not required in v1).

The strategy must be deterministic per edge node and must not cause pathological imbalance under normal conditions.

## PROXY protocol v2 injection (when enabled)
### When enabled
A Route can enable `proxy_protocol=v2`.

v1 default:
- off unless explicitly enabled.

### Injection behavior (normative)
If enabled:
- Edge must prepend a valid PROXY v2 header to the upstream connection **before any application bytes**.
- This applies to both tls_passthrough and tcp_raw routes.

This means:
- If enabled for tls_passthrough, the backend will see PROXY header first, then TLS ClientHello bytes.

### Spoofing prevention
- Only edge components may inject PROXY headers.
- The platform must prevent public clients from reaching backend ports that accept PROXY headers without going through edge.

### Misconfiguration handling
If a route enables PROXY v2 but backend does not support it:
- traffic will fail.
- The platform should surface this clearly:
  - route validation requires `backend_expects_proxy_protocol=true` acknowledgment
  - observability should show upstream handshake failures

## Timeouts and connection handling (v1 defaults)
Edge must implement sensible defaults (operator configurable):
- connect timeout to backend: 2s
- idle timeout: none by default for raw TCP (or a large default), because many protocols hold long-lived connections
- max concurrent connections per route: optional policy knob (abuse control)

The edge must not terminate TLS sessions. It is a TCP relay.

## Configuration distribution and atomicity
Edge routing config is derived from control plane state and must be applied atomically.

Rules:
- config updates must not produce transient states that route a hostname to the wrong tenant
- route deletes remove bindings immediately in the applied config
- config reload must be safe under load where possible (avoid dropping established connections when reloading)

Control plane outage behavior:
- edge continues operating on last applied config

## Observability requirements (edge)
Edge must emit:
- per-listener connection rate and concurrent connections
- per-route connection rate and concurrent connections (bounded by route count)
- upstream connect failures and timeouts
- backend set size per route
- SNI sniff failures count (timeouts, not TLS, no SNI)
- PROXY v2 enabled route count

Edge logs (structured):
- include route_id, hostname (when safe), backend selection (overlay_ipv6), and error details
- do not log payload bytes

## Failure behavior summary
- Control plane down: existing routing continues.
- Backend unreachable: remove backend from eligible set, continue if other backends exist.
- No eligible backends: connection should fail fast (TCP reset or close) with metrics recorded.
- SNI missing and ambiguous: connection is closed.

## Compliance tests (required)
1) TLS passthrough route with SNI routes to correct backend without terminating TLS.
2) Connection without SNI is rejected when ambiguous and accepted when unambiguous.
3) Raw TCP route proxies bytes unchanged.
4) PROXY v2 injection prepends header and backend receives correct client ip and port.
5) Backend health gating prevents routing to instances not marked ready.
6) Port conflicts are rejected at route creation and cannot be applied at edge.

## Open questions (deferred)
- External traffic distribution across multiple edge nodes (DNS strategy, anycast later).
- Whether to support wildcard hostnames in a future version (would require explicit conflict semantics).
- Whether to support UDP in a future version (out of scope for v1).
