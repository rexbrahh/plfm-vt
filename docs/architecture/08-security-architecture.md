# docs/architecture/08-security-architecture.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document describes the platform security architecture at a systems level.

It answers:
- What we are defending
- Who we trust and who we do not
- What boundaries exist
- What controls exist in v1
- What is explicitly out of scope

Authoritative details live in:
- `docs/security/*` (threat model and policies)
- `docs/specs/secrets/*`
- `docs/specs/runtime/*`
- `docs/specs/networking/*`
- `docs/specs/api/*`
- ADRs in `docs/adr/*`

## Security goals (v1)
1) Prevent cross-tenant access in the control plane.
2) Contain a compromised tenant workload to its microVM (and ideally to a single instance).
3) Prevent secrets leakage across environments.
4) Prevent routing ownership violations (a tenant cannot hijack another tenant’s hostname or traffic).
5) Make sensitive actions auditable and attributable.
6) Keep the data plane serving traffic safely during control plane outages.
7) Keep the platform operable under attack (abuse controls, rate limits, and incident tooling).

## Non-goals (security)
See `docs/NONGOALS.md`. Specifically:
- We do not promise a WAF or CDN in v1.
- We do not promise full HTTP-layer security controls without an opt-in L7 mode.
- We do not support privileged host access from workloads.
- We do not support user-provided kernels or arbitrary VM images in v1.

## Threat model snapshot (what we assume)
### Adversaries
- A tenant running malicious workload code.
- A tenant attempting to access other tenants’ data or traffic.
- An external attacker probing ingress, ports, and control plane APIs.
- A compromised node (host) in the fleet.
- A compromised credential (API token) for a user or service principal.

### Assets
- Secrets material (API keys, DB passwords, signing keys if any).
- Tenant isolation boundaries (org and env boundaries).
- Routing bindings (hostnames and ports).
- Control plane state (event log, views).
- Workload integrity (running the intended release and configuration).
- Logs and telemetry (may contain sensitive metadata).

### Assumptions
- Workloads are untrusted.
- The platform control plane and edge are trusted components.
- Hosts are trusted infrastructure, but we assume compromise is possible and plan for containment and revocation.

## Trust boundaries
### Control plane boundary
Trusted. Holds tenant metadata and encrypted secrets. Enforces authn and authz.

### Edge boundary
Trusted. Enforces ingress routing rules, optional PROXY v2 injection, and route ownership constraints as configured by control plane.

### Host boundary
Trusted infrastructure code (node agent and host OS). Runs untrusted workloads inside microVMs.

### Workload boundary
Untrusted. A workload can be malicious and can attempt to escape, probe, or exfiltrate.

## Primary isolation controls
### MicroVM isolation (mandatory)
- Each instance runs in its own Firecracker microVM.
- Instances are env-scoped and process-type scoped.
- This is the primary containment boundary.

See ADR 0001 and ADR 0003.

### Firecracker hardening (mandatory)
Minimum expected controls:
- Firecracker runs jailed (jailer or equivalent containment).
- A seccomp policy is applied to the VMM process.
- cgroup v2 enforces CPU fairness and memory hard caps.
- Host filesystem access is restricted to per-microVM directories and explicit block devices.

See `docs/specs/runtime/limits-and-isolation.md`.

### Host OS hardening (required posture)
We aim for reproducible host configuration (NixOS intent), but security requirements apply regardless of distro:
- minimal services
- locked-down SSH and admin access
- regular kernel and security patch cadence
- nftables baseline policy
- strong separation of volume pool and image cache locations
- file permissions and ownership enforced on runtime directories

## Identity and access control
### User and service principal auth
- All control plane API calls are authenticated.
- Tokens have expiration and explicit scopes.
- Least privilege: deploy rights do not imply secrets material read.

See `docs/specs/api/auth.md`.

### Node identity
- Nodes authenticate to control plane via mTLS.
- Nodes join overlay via WireGuard keys.
- Enrollment is operator-controlled and auditable.
- Revocation removes node access to control plane and overlay membership.

See `docs/specs/networking/overlay-wireguard.md`.

## Authorization model (high level)
All resources are owned by an org and generally scoped down to app and env:
- org -> app -> env -> route, secrets, deployments, instances

Key constraints:
- A tenant cannot bind hostnames already owned by another tenant.
- A tenant cannot access other tenants’ logs, exec, secrets metadata, or secrets material.
- Exec is a high-risk permission and requires explicit scope.

See `docs/architecture/05-multi-tenancy-and-identity.md`.

## Secrets security
### Storage at rest
- Secrets are stored encrypted at rest.
- Keys and rotation policies are defined in `docs/specs/secrets/encryption-at-rest.md`.
- Audit log records secrets version changes without logging secret material.

### Delivery to workloads
- Secrets are delivered as a mounted file in a fixed format.
- File permissions are restrictive by default (root-only readable unless explicitly configured).
- Secrets are scoped to `(org, app, env)` and must never cross that boundary.
- Default rotation semantics are restart-based rollouts, not hot reload.

See ADR 0010 and `docs/specs/secrets/*`.

### Logging rules for secrets
- No raw secret material in logs, metrics, traces, or events.
- Tooling must redact common secret patterns where feasible.
- Debug endpoints must not dump secret files.

## Networking security
### Overlay
- WireGuard overlay encrypts node-to-node traffic.
- Membership is controlled by control plane.
- Allowed IPs are allocated by IPAM, no overlaps.
- MTU is standardized and ICMPv6 Packet Too Big must not be blocked.

See ADR 0004 and `docs/specs/networking/overlay-wireguard.md`.

### Ingress (L4-first)
- Default is TLS passthrough with SNI inspection for routing.
- No mandatory TLS termination at edge in v1, so platform does not store tenant TLS private keys by default.
- Raw TCP exposure is explicit and audited.
- IPv4 is not default and requires a paid add-on allocation.

See ADR 0007 and ADR 0008.

### Route ownership and hijack prevention
Route objects enforce:
- exclusive hostname ownership per environment
- explicit bindings with audit events
- conflict detection at creation time
- atomic updates at edge (no partial state that can route hostnames incorrectly)

### PROXY protocol v2 spoofing prevention
If PROXY v2 is enabled:
- Only platform edge components may inject PROXY headers.
- Workload ports that accept PROXY must not be exposed in a way that allows public clients to send arbitrary PROXY headers.
- This is enforced by route config and by network policy on the host/edge.

See ADR 0009.

### Egress posture and abuse
Default v1 stance is permissive egress, but we must:
- retain enough metadata to investigate abuse
- reserve the right to rate-limit or block abusive patterns at the operator level
- provide a path to stricter per-org or per-env egress policies later

## Supply chain and artifact integrity
### Artifact immutability
- Releases are digest-pinned OCI images plus manifest hash.
- Nodes pull by digest, not tags.
- This provides reproducibility and reduces tampering risk.

See ADR 0002.

### Signing and verification (future)
- Image signing and verification (cosign or similar) is a likely future control.
- Not required in v1, but the architecture should allow enforcing it later without changing the artifact model.

## Exec and interactive access security
Exec is high risk.

Requirements:
- Exec is brokered by control plane, never direct tenant-to-agent.
- Exec requires short-lived grants (time-bound).
- Exec is fully audited (who, when, to which instance, for how long).
- Exec must not grant host shell access.
- The exec channel must be authenticated end-to-end and bound to an instance identity.

See `docs/specs/runtime/*` and `docs/specs/api/*`.

## Observability and privacy
- Logs and metrics are tenant data. Access is scoped and audited.
- Default log retention is a product decision, but must be explicit.
- Avoid high-cardinality labels that can leak sensitive information.

See `docs/specs/observability/*`.

## Incident response expectations
A secure system includes response capability.

Minimum operator capabilities:
- revoke tokens and rotate credentials
- disable an org or env quickly
- disable or quarantine a node
- remove routes and unbind hostnames quickly
- rebuild projections from event log for forensic accuracy
- restore Postgres from backups with WAL replay
- restore volumes from backups

Runbooks should exist under `docs/ops/runbooks/*`.

## Security review checklist for new features
Every new feature must answer:
1) Does it expand a trust boundary or add a new one?
2) Does it introduce new secret handling paths?
3) Does it introduce a new externally reachable port or protocol?
4) Does it create cross-tenant shared state or shared execution?
5) Can it be abused to cause resource exhaustion?
6) Is it auditable? Can we attribute actions to an actor?
7) Does it preserve the locked ADR decisions, or does it require a new ADR?

## Next document
- `docs/architecture/09-observability-architecture.md`
