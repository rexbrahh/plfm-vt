# docs/specs/networking/proxy-protocol-v2.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the platform’s usage of HAProxy PROXY Protocol v2:
- when it is injected
- trust boundaries and spoofing prevention
- how workloads should consume it
- compatibility rules and validation
- observability requirements

This spec does not reprint the full PROXY v2 byte-level protocol definition. It defines the platform contract and references the upstream specification for exact wire format.

Locked decision: client identity propagation uses PROXY protocol v2 (opt-in). See `docs/adr/0009-proxy-protocol-v2-client-ip.md`.

## Scope
This spec defines how the platform injects and uses PROXY v2.

This spec does not define:
- ingress routing rules (see `docs/specs/networking/ingress-l4.md`)
- HTTP header forwarding (L7, out of scope for v1)
- workload-level auth and rate limiting (future)

## Definitions
- **Client source identity**: the original client IP and port that connected to the edge.
- **Transport peer**: the immediate TCP peer of the workload (usually the edge, not the client).
- **PROXY v2 header**: a binary header prepended to an upstream TCP stream describing the client and connection metadata.
- **Trusted injection point**: platform-controlled edge ingress component that injects the header.
- **PROXY-aware backend**: a server that expects and parses PROXY protocol on a given listener.

## Platform stance (v1)
- PROXY v2 is supported for TCP and TLS passthrough routes.
- It is **off by default** and must be enabled explicitly per Route.
- When enabled, the edge prepends a PROXY v2 header to the upstream connection before any application bytes.
- The platform treats PROXY headers as trusted only when they originate from the platform edge.

## Route-level configuration
Routes include:
- `proxy_protocol` = `off` | `v2`
- `backend_expects_proxy_protocol` (bool)

Validation rules (normative):
- If `proxy_protocol=v2`, then `backend_expects_proxy_protocol` must be true, otherwise reject route creation/update.
- If a route is `tls_passthrough` and `proxy_protocol=v2`, the backend must accept PROXY v2 **before** the TLS ClientHello.

## Injection behavior (normative)
When a connection arrives at edge and is routed to a backend:

- If route `proxy_protocol=off`:
  - forward the stream unchanged (except routing itself)

- If route `proxy_protocol=v2`:
  1) open upstream TCP connection to backend
  2) write PROXY v2 header to upstream connection
  3) then begin forwarding client bytes to backend

The header must represent:
- original client source address and port
- original destination address and port as observed at the edge listener

Address families:
- If client is IPv6: use INET6 family in the header.
- If client is IPv4: use INET family.

Transport protocol:
- TCP stream indicates STREAM.
- Datagram mode is out of scope for v1.

Timeout behavior:
- If backend connect fails before header can be written, connection fails normally.
- If backend closes immediately after header write, record as upstream failure.

## Trust and spoofing prevention
This is critical. Incorrect handling makes the platform unsafe.

### Trust model
- Only platform edge components may inject PROXY v2.
- Workloads must treat PROXY v2 metadata as untrusted unless they are certain traffic originates from platform edge.

Platform enforcement requirements:
1) Backends that expect PROXY v2 must not be reachable directly from the public internet.
2) The platform must not expose “backend internal ports” directly.
3) If a workload runs a proxy that accepts PROXY v2, it must be on the routed backend port only.

Practical measures:
- Only edge nodes can reach instance overlay IPv6 addresses (enforced by overlay routing and firewall policy).
- If there is any internal east-west traffic that can reach instance addresses, then tenants could spoof PROXY if they can connect. In that case:
  - either disallow tenant-to-tenant overlay connectivity, or
  - require additional constraints (mTLS between edge and backend), which is out of scope for v1.

v1 recommendation:
- treat instance overlay addresses as not directly reachable by tenant workloads by default. Keep overlay as infra-only plane.

### “PROXY only from edge” rule
If a backend expects PROXY v2 and receives a stream without a PROXY header, it should reject.
If it receives a malformed header, it should reject.

This is backend behavior, but platform docs should recommend it.

## Backend consumption patterns
Because PROXY v2 is protocol-agnostic, how you use it depends on workload type.

### Pattern A: Backend natively supports PROXY v2
Examples:
- HAProxy
- Nginx stream module (can accept PROXY)
- Envoy can accept PROXY
- Some databases and TCP servers may support it, many do not

In this pattern:
- You enable `proxy_protocol=v2` on the Route.
- Backend listener is configured to expect PROXY v2.

### Pattern B: Run a front proxy inside the microVM
If your app does not support PROXY v2:
- Run a small proxy (Nginx, HAProxy, Envoy) inside the microVM that:
  - accepts PROXY v2 from the edge
  - forwards to your app over localhost without PROXY
  - optionally translates client identity into app-specific headers (only for HTTP, but that implies L7 handling inside the microVM)

This pattern is recommended in v1 rather than shipping a platform “stripper adapter”.

### Pattern C: Do not enable PROXY v2
If you do not need client IP:
- keep it off for maximum compatibility.

## Compatibility and defaults
### Default off (v1)
Default is OFF to avoid breaking workloads.

Reasons:
- PROXY v2 prepends bytes, which breaks any server that does not parse it.
- Many raw TCP services will not support it.

### Per-port semantics
PROXY v2 expectation is per-listener/port.
- If a process exposes multiple ports, you can enable PROXY v2 on one route and not on another.
- The manifest port declarations remain unchanged. PROXY is a route-level behavior.

### TLS passthrough compatibility
If you enable PROXY v2 on TLS passthrough:
- the backend must accept PROXY then proceed with TLS handshake.
- This often means running a proxy (like HAProxy or Envoy) at the backend port.

## Observability requirements
Edge must emit:
- count of connections with PROXY v2 enabled
- upstream write failures for PROXY header
- malformed header errors (if edge detects it, usually backend detects it)
- per-route upstream error rates

Agent/control plane should surface:
- common failure signature when PROXY is enabled but backend is not PROXY-aware:
  - TLS handshake failures, protocol errors, immediate connection closes

CLI user experience requirement:
- When creating a route with PROXY enabled, CLI should warn clearly:
  - “Your backend must support PROXY v2 on this port.”

## Testing requirements
Automated tests must validate:
1) For a route with PROXY off, backend sees the first bytes as the real protocol payload.
2) For a route with PROXY on, backend sees a valid PROXY v2 header then payload.
3) Client IP and port in the header match the actual client.
4) For IPv6 clients, INET6 address family is used and correct.
5) Spoofing prevention:
- ensure workloads cannot connect directly to backend port bypassing edge in the default network policy.

## Operational guidance
- Use PROXY v2 primarily when you need:
  - accurate client IP in logs
  - abuse controls
  - IP allowlists at backend

If you only need it for HTTP apps, you may prefer an L7 mode later or run an internal proxy in the microVM.

## Open questions (explicitly deferred)
- Whether to introduce an opt-in “edge-to-backend mTLS” feature to cryptographically assert edge identity. Not in v1.
