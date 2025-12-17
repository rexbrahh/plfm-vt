# docs/architecture/05-multi-tenancy-and-identity.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document defines the multi-tenancy model and identity system for the platform.

It answers:
- Who owns what
- What boundaries exist
- How authentication works
- How authorization works
- What is audited

Authoritative details live in:
- `docs/specs/api/auth.md`
- `docs/specs/api/http-api.md`
- `docs/specs/state/event-types.md`
- `docs/specs/networking/overlay-wireguard.md`
- `docs/specs/secrets/*`

## High-level stance
- The platform is multi-tenant.
- Tenant boundaries are enforced in the control plane.
- Workload isolation is enforced by microVM boundaries.
- Environment is the primary boundary for config, secrets, routes, and runtime instances.

See:
- ADR 0001 (microVM per env)
- ADR 0005 (event log source of truth)
- ADR 0006 (Postgres)
- ADR 0010 (secrets file delivery)

## Tenancy hierarchy
### Org (tenant)
The top-level ownership boundary.
- Billing, quotas, and global access control anchor here.
- Most API requests are scoped to an org context.

### Project (optional)
A grouping mechanism within an org.
- Used to organize apps.
- Not required for correctness. It must not become a second tenant boundary.

### App
A named service owned by an org (and optionally under a project).
Examples: `api`, `web`, `worker`.

### Environment (env)
A deploy target for an app.
Examples: `prod`, `staging`.

Environment is the key boundary for:
- config
- secrets bundles and versions
- routing bindings (hostnames, ports)
- releases currently promoted
- runtime instances (microVMs)

## Resource ownership graph
All resources must have a clear owner. The control plane must reject ambiguous ownership.

### Core resources
- Org owns Projects
- Org owns Apps
- App owns Environments
- Environment owns:
  - Routes
  - Secrets bundle bindings
  - Volume attachments (volumes may be created at org scope but attachments are env scoped)
  - Desired scale settings per process type
  - Promoted releases per process type

### Runtime resources
- A Release belongs to an App, but is promoted into an Environment.
- Process type belongs to an Environment.
- Instance belongs to `(env, process_type)` and is one microVM.

### Infrastructure resources
- Nodes are platform infrastructure and are not “owned by” tenant orgs in the same way apps are.
- Node capacity is shared across tenants by scheduling.
- Node enrollment is an operator action, not a tenant feature.

## Boundary rules (explicit)
These are the rules we must be able to state and enforce.

### 1) Environment boundary
- Secrets for `(org, app, env)` must never be delivered to any other env.
- Routes bind hostnames and ports to one env at a time.
- A microVM instance runs exactly one process type for one env.

### 2) App boundary
- Releases are app-scoped. An env can only promote releases belonging to its app.
- A route must target a process type within the same env.

### 3) Org boundary
- An org cannot read, write, or list any resources owned by another org.
- Org boundaries apply to:
  - apps, envs, routes, secrets, volume metadata, deployments, logs access, exec access
  - IPv4 allocations and billing metadata
- Cross-org collaboration is not supported in v1 unless explicitly designed.

### 4) Infrastructure boundary
- Tenants never talk directly to host agents.
- Tenants never receive WireGuard or node enrollment credentials.
- Any “exec” or “logs” flow is brokered by the control plane with short-lived grants.

## Identity types
### Human users
People using the CLI or future UI.
- Authenticated via the platform auth flow.
- Authorized via org membership and roles.

### Service principals
Non-human identities used by automation.
Examples:
- CI deploy bots
- internal platform services

Service principals must have:
- explicit scopes
- explicit org membership
- least privilege by default

### Nodes (host identities)
Infrastructure identities used for:
- control plane RPC (mTLS)
- overlay membership (WireGuard keys)

Nodes are enrolled by operators, not by tenants.

## Authentication (authn)
### CLI authentication
The CLI authenticates to the control plane and obtains a token.
- The exact flow is defined in `docs/specs/api/auth.md`.
- The CLI must support non-interactive flows for automation via service principals.

Tokens must have:
- expiration
- audience
- scopes
- org context (or require org selection on each call)

### Node authentication
Node agents authenticate to the control plane using mTLS.
- Node cert issuance is tied to enrollment.
- Cert rotation is supported.
- Revocation is supported (disable node, remove access, rotate keys).

### Important rule
No identity gets indefinite credentials by default. Everything expires and is renewable.

## Authorization (authz)
### Model
Authorization combines:
- org membership
- roles (coarse-grained)
- scopes (fine-grained)

Recommended v1 roles (minimum):
- Owner
- Admin
- Developer
- ReadOnly

Roles map to scope sets, but scopes remain the enforcement primitive.

### Scope examples (illustrative)
- `apps:read`, `apps:write`
- `envs:read`, `envs:write`
- `deploy:write`, `rollback:write`
- `routes:read`, `routes:write`
- `secrets:read-metadata`, `secrets:write`
- `secrets:read-material` (high risk, should be rare)
- `volumes:read`, `volumes:write`
- `logs:read`
- `exec:write` (high risk, requires explicit opt-in)
- `billing:read`, `billing:write`
- `admin:nodes` (operator only)

Design requirement:
- It must be possible to grant deploy rights without granting secrets material read.
- It must be possible to grant logs read without exec.
- It must be possible to separate billing from engineering access.

### Resource-level checks
Every write must validate resource ownership:
- does this env belong to this org
- does this route hostname already belong to another org or env
- is this release owned by the app
- does this secret bundle belong to the env

Authorization is enforced before event appends.

## Audit logging
Because the control plane is event-based, auditability is first-class.

Every sensitive action must produce an audit-visible trail:
- secrets create and update
- route create, update, delete
- IPv4 add-on enablement and port changes
- exec session grants
- role and membership changes
- node enrollment, revocation, rotation actions

Event payloads should include:
- actor identity (user id or service principal id)
- actor org
- request metadata (request id, idempotency key)
- high-level change description

Rule:
- Never log raw secret material.

## Quotas and fairness
Multi-tenancy requires preventing one org from consuming the whole fleet.

v1 recommendation:
- enforce quotas at the org level (instances, total memory, routes, IPv4 allocations)
- enforce runtime fairness with cgroup CPU weights
- keep scheduling fairness simple and explicit

Quotas are part of product and billing, but enforcement belongs in the control plane.

## Threat model highlights
- A tenant must not be able to:
  - access another tenant’s resources in the control plane
  - spoof routing ownership (hostnames and ports)
  - access another env’s secrets
  - gain privileged host access through exec or runtime features
- A compromised workload instance must be contained to its microVM and not become a host compromise.

## Next document
- `docs/architecture/06-failure-model-and-degraded-modes.md`
