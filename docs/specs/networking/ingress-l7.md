# docs/specs/networking/ingress-l7.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define the optional Layer 7 ingress mode for HTTP(S):
- what “L7 mode” means in this platform
- how it is enabled (opt-in)
- what behavior is guaranteed
- what headers are injected and which are stripped
- how TLS termination and certificate management works (when enabled)
- how L7 stays isolated from the v1 L4-first data plane

Locked decision: ingress is L4-first and SNI passthrough by default. L7 is optional and must not contaminate v1. See `docs/ADRs/0008-ingress-l4-sni-passthrough-first.md`.

## Scope
This spec defines the future L7 mode contract.

This spec does not define:
- the v1 L4 ingress behavior (see `docs/specs/networking/ingress-l4.md`)
- overlay mechanics (see `docs/specs/networking/overlay-wireguard.md`)
- IP allocation (see `docs/specs/networking/ipam.md`)
- PROXY protocol v2 details (see `docs/specs/networking/proxy-protocol-v2.md`)

## Design stance
- L7 is an opt-in feature per Route.
- L7 must be implemented so that the L4 plane continues to function unchanged.
- L7 must not become a dependency for raw TCP support.
- L7 mode is for HTTP(S). It does not add UDP support.

## Terminology
- **L7 termination**: edge terminates TLS and parses HTTP.
- **HTTP route**: a route that can match host and optionally path, then forward to a backend.
- **Backend**: still an instance endpoint over the overlay, typically `overlay_ipv6:port`.
- **L7 edge**: the ingress component in HTTP mode (may be the same binary as L4, but must remain logically separable).

## Enabling L7 mode
### Route model extension
In v1, `protocol_hint` is `tls_passthrough` or `tcp_raw`.

L7 introduces a new protocol hint:
- `http_terminate`

Rules:
- Existing routes remain unchanged.
- L7 routes are separate and opt-in only.

API and event model implication:
- Adding `http_terminate` is a versioned expansion of the Route schema.
- This must be implemented as an additive change with explicit validation.

### Mutual exclusivity
A hostname and listen port combination must not have both:
- a `tls_passthrough` route and a `http_terminate` route active at the same time

Reason:
- ambiguous routing and inconsistent TLS handling.

## L7 routing inputs and matching
### Matching keys (v1 for L7 mode)
- Host header (and SNI if HTTPS, because TLS is terminated)
- Optional path prefix matching (future, but supported in L7 mode conceptually)

v1 recommendation for first L7 release:
- Host-only routing, exact match only, no wildcard hostnames.
- Path routing can be added later as a non-breaking extension.

### Normalization rules
- Hostname normalization matches L4 rules:
  - lower-case
  - trim trailing dot
  - IDNA normalization

## TLS termination and certificates
L7 mode requires the platform to handle TLS private keys. This is a major security boundary change.

### Certificate sources (future options)
L7 must support at least one of these, explicitly:

Option A (recommended first): platform-managed ACME
- User proves ownership by DNS or HTTP challenge (implementation-specific).
- Platform stores private keys encrypted at rest.
- Platform auto-renews certificates.

Option B: user-provided cert bundle
- User uploads certificate and key to the platform (high risk).
- Platform stores it encrypted at rest.
- Strict access control and auditing are required.

v1 recommendation for the first L7 release:
- Start with platform-managed ACME only.
- Avoid user-provided cert upload until you have a strong secret storage story for edge-managed secrets.

### Key storage requirements (mandatory if L7 exists)
- Keys are encrypted at rest.
- Access is limited to edge components that require them.
- Every issuance, renewal, and deletion is audited.
- No keys ever appear in logs.

### TLS policies
When terminating TLS, the platform must define:
- supported TLS versions (minimum TLS 1.2, prefer TLS 1.3)
- ciphersuite policy
- ALPN policy (h2 optional, h1 required)

These are operator-configurable but must have safe defaults.

## HTTP forwarding behavior
### Header trust model
Clients can spoof headers like `X-Forwarded-For`. The edge must sanitize.

Normative rules:
- Strip incoming:
  - `X-Forwarded-For`
  - `X-Forwarded-Proto`
  - `X-Forwarded-Host`
  - `Forwarded`
- Then set platform-controlled values.

### Required injected headers
When forwarding to backend:
- `X-Forwarded-For`: client IP (append or set, but only with platform-controlled value)
- `X-Forwarded-Proto`: `http` or `https`
- `X-Forwarded-Host`: original host
- `Forwarded`: RFC-compliant form is preferred if you choose it
- `X-Request-Id`: stable per request for correlation (if not provided, generate)

### Client IP for backends
In L7 mode, client IP propagation is via HTTP headers, not PROXY v2, because we are already parsing HTTP.

Rule:
- If L7 mode is enabled, PROXY v2 is not required for that route.
- If you still want it for some reason, it must be explicit and carefully validated, but default is “no PROXY in L7”.

## Backend protocol and ports
Backends remain unchanged:
- backend is `overlay_ipv6:port`
- backend port must be declared in the manifest port list for the process type

v1 recommendation for first L7 release:
- Forward HTTP/1.1 to backend.
- Support websockets by preserving upgrade headers.

HTTP/2 to backend is optional later.

## Health checks
L7 can optionally provide HTTP health checks at edge.

Rules:
- Health checks must not rely on private endpoints unless explicitly configured.
- A backend is eligible only if:
  - control plane says instance is ready
  - optional L7 HTTP check passes (if enabled)

The control plane remains the primary readiness source.

## Rate limiting and abuse controls (future)
L7 mode creates an obvious place to apply:
- per-route rate limits
- per-org quotas
- basic request size caps

v1 recommendation for first L7 release:
- include only basic protections:
  - max header size
  - max request body size (configurable)
  - connection rate limits at listener level

Full WAF features are explicitly out of scope.

## Observability requirements (L7-specific)
L7 edge must emit:
- request rate, error rate, latency (p50/p95/p99)
- status code counts per route (bounded)
- backend selection errors
- TLS handshake error counts
- certificate issuance and renewal metrics

Logs:
- structured request logs are optional but must be privacy-aware.
- Do not log full URLs with secrets in query strings.
- Provide sampling controls.

Tracing:
- Propagate `traceparent` if present, or create one.
- Provide trace ids for correlation with control plane and backend logs.

## Isolation from v1 L4 plane
This is the most important part.

Requirements:
1) L4 plane must work without L7 components.
2) L7 code paths must not change L4 behavior.
3) L7 configuration must be separate from L4 configuration:
- do not reuse L4 routing tables for L7 matching

Operational rule:
- Disabling L7 mode must not require any change to the L4 runtime.

## Failure behavior
- If L7 edge is down, L7 routes are down.
- L4 routes should remain functional.
- If certificate renewal fails, the platform must surface alerts well before expiry.

## Open questions (explicitly deferred)
- ACME challenge mechanism (DNS vs HTTP). DNS is safer at scale but requires DNS provider integration.
- Whether to support shared IPv4 tiers for L7 only (would be a product decision and likely an ADR).
- Whether to support wildcard hostnames in L7 mode (would require explicit conflict semantics).
