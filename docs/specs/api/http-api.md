# docs/specs/api/http-api.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document specifies the control plane HTTP API at a human-readable level:
- resource model and URL structure
- endpoint inventory
- request/response shapes (high level)
- pagination and filtering
- idempotency rules
- error model

The machine-readable source of truth is:
- `docs/specs/api/openapi.yaml`

This document must remain consistent with OpenAPI.

## API principles (v1)
- Org-scoped paths for all tenant resources.
- Opaque ids in URLs. No user-facing reliance on internal DB ids.
- Idempotent writes where users will retry (deploy, create route, create volume, secrets updates).
- Stable error codes for scripting.
- Never leak secrets in responses unless explicitly requested and authorized (and that should be rare).

## Base URL and versioning
- Base path: `/v1`
- All endpoints are versioned by path.
- Future breaking changes go to `/v2`.

## Authentication
- All endpoints require `Authorization: Bearer <access_token>` except a small set under `/v1/auth/*`.
- Auth flows are defined in `docs/specs/api/auth.md`.

## Resource naming conventions
- Plural nouns in paths: `/orgs`, `/apps`, `/envs`, `/routes`, `/volumes`.
- Subresources use nested paths when ownership is strict:
  - `/orgs/{org_id}/apps/{app_id}/envs/{env_id}`
- Prefer explicit relationships over “magic defaults”.

## Common headers
### Request
- `Authorization: Bearer ...` (required)
- `Idempotency-Key: <opaque string>` (recommended for write endpoints)
- `X-Request-Id: <opaque string>` (optional, client-provided)

### Response
- `X-Request-Id: <opaque string>` (server-generated if not provided)

## Idempotency
### When required
Write endpoints that may be retried must accept idempotency keys:
- deploy creation
- release creation (if separated)
- route create/update
- volume create/attach
- secrets update

### Semantics
- If the same idempotency key is reused for the same org and endpoint:
  - return the original successful response
- If reused with different request body:
  - return `409 conflict` with code `idempotency_key_reuse`

Idempotency keys are scoped:
- by org
- by endpoint
- by actor identity

Retention:
- server retains idempotency records for at least 24 hours in v1.

## Pagination
List endpoints support cursor pagination.

Request query parameters:
- `limit` (int, default 50, max 200)
- `cursor` (opaque string)

Response fields:
- `items` (array)
- `next_cursor` (string or null)

Filtering:
- Where relevant, support query params like:
  - `app_id=`
  - `env_id=`
  - `status=`
  - `created_after=`
  - `created_before=`

## Error model (global)
All errors return JSON:
- `code` (stable string)
- `message` (human-readable)
- `request_id`
- `retryable` (bool)
- optional `details` (object)

HTTP status mapping (v1):
- 400: `invalid_argument`
- 401: `unauthorized`
- 403: `forbidden`
- 404: `not_found`
- 409: `conflict`
- 412: `precondition_failed`
- 429: `rate_limited`
- 500: `internal`
- 503: `unavailable`

The `code` field is the stable contract; the HTTP status is secondary.

## Core domain resources

### Org
Represents tenant boundary.

Fields (high level):
- `id`
- `name`
- `created_at`

### Project
A named grouping within an org (for organization, policy, and future quotas).

Fields:
- `id`
- `org_id`
- `name`
- `resource_version`
- `created_at`

### App
A named service in an org.

Fields:
- `id`
- `org_id`
- `name`
- `created_at`

### Environment (env)
Deploy target for an app.

Fields:
- `id`
- `app_id`
- `name` (prod, staging)
- `created_at`

### Release
Immutable deploy artifact (image digest + manifest hash).

Fields:
- `id`
- `app_id`
- `image_digest`
- `manifest_hash`
- optional resolved digests per arch
- `created_at`

### Process type
Named entrypoint within an env (web, worker). Defined by manifest and reflected in env state.

### Instance
One running microVM corresponding to a desired instance slot.

Fields:
- `id` (instance_id)
- `env_id`
- `process_type`
- `node_id`
- `status` (booting, ready, draining, stopped)
- `generation`
- `created_at`

### Route
Hostname and listener binding to an env/process.

Fields:
- `id`
- `env_id`
- `hostname`
- `listen_port`
- `protocol_hint` (tls_passthrough, tcp_raw)
- `backend_process_type`
- `backend_port`
- `proxy_protocol` (off, v2)
- `ipv4_required`
- `created_at`

### Secrets
Environment-scoped secret bundle and versions.

Fields:
- bundle id
- env id
- version id
- created_at
- updated_at

### Volume
Local persistent volume and attachments.

Fields:
- volume id
- size
- filesystem
- created_at
- attachments (env/process/mount path)

## Endpoint inventory (v1)

### Auth endpoints
See `docs/specs/api/auth.md` for exact flow behavior.

- `POST /v1/auth/device/start`
- `POST /v1/auth/device/token`
- `POST /v1/auth/token`
- `POST /v1/auth/token/refresh`
- `POST /v1/auth/token/revoke`
- `GET  /v1/auth/whoami`

### Orgs and membership
- `GET  /v1/orgs`
  - list orgs user belongs to
- `GET  /v1/orgs/{org_id}`
- `GET  /v1/orgs/{org_id}/members`
- `POST /v1/orgs/{org_id}/members` (admin)
- `PATCH /v1/orgs/{org_id}/members/{member_id}` (admin)
- `DELETE /v1/orgs/{org_id}/members/{member_id}` (admin)

### Projects
- `GET  /v1/orgs/{org_id}/projects`
- `POST /v1/orgs/{org_id}/projects`
- `GET  /v1/orgs/{org_id}/projects/{project_id}`

Validation:
- project name unique per org

### Apps
- `GET  /v1/orgs/{org_id}/apps`
- `POST /v1/orgs/{org_id}/apps`
- `GET  /v1/orgs/{org_id}/apps/{app_id}`
- `PATCH /v1/orgs/{org_id}/apps/{app_id}`
- `DELETE /v1/orgs/{org_id}/apps/{app_id}`

Validation:
- app name unique per org

### Environments
- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs`
- `POST /v1/orgs/{org_id}/apps/{app_id}/envs`
- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}`
- `PATCH /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}`
- `DELETE /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}`

Validation:
- env name unique per app

### Releases
Two patterns exist. Pick one. v1 recommendation: explicit release creation.

#### Pattern A (explicit release object)
- `POST /v1/orgs/{org_id}/apps/{app_id}/releases`
  - request includes image digest (or tag to resolve) plus manifest contents (or manifest hash with upload separately)
  - response returns release id

- `GET /v1/orgs/{org_id}/apps/{app_id}/releases`
- `GET /v1/orgs/{org_id}/apps/{app_id}/releases/{release_id}`

#### Pattern B (deploy creates release implicitly)
If you collapse release creation into deploy creation, document it and keep release id stable in responses.

### Deploys and promotions
Deploy is the act of selecting a release for an env/process and rolling it out.

- `POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys`
  - request:
    - release id (or image+manifest for implicit release creation)
    - target process types (optional, default all)
    - rollout strategy (v1 only rolling, optional)
  - response:
    - deploy id
    - status

- `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys`
- `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys/{deploy_id}`

Rollback is a deploy selecting an older release:
- `POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/rollbacks`
  - request: release id to roll back to
  - response: deploy id

Idempotency:
- deploy and rollback creation must be idempotent.

Read-your-writes:
- deploy creation should return a stable deploy object and initial status.

### Scale (desired counts)
Scaling is env + process type desired replica counts.

- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale`
- `PUT  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale`
  - request sets desired replicas per process type
  - response returns updated desired scale state

Rules:
- scale changes create events and trigger scheduler reconciliation.

### Instances (runtime view)
- `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances`
  - filter by process_type and status
- `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{instance_id}`

This is view-only. Tenants cannot directly start/stop instances; they set desired state.

### Routes
Routes are env-scoped and bind hostnames and ports.

- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes`
- `POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes`
- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}`
- `PATCH /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}`
- `DELETE /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}`

Validation:
- hostname unique across platform, or at minimum across org (decision must be explicit in routing spec).
- backend port must be declared in manifest for target process type.
- if `proxy_protocol=v2`, require explicit acknowledgement in request.

IPv4 add-on linkage:
- if route requires IPv4 or binds raw TCP ports that require IPv4, the env must have IPv4 add-on enabled.

### Secrets
Secrets are env-scoped bundles with versions.

- `GET  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets`
  - returns metadata only (bundle id, version id, updated_at)

- `PUT  /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets`
  - request: secrets in the platform file format or as key/value map
  - response: new version metadata
  - idempotent

- Optional, high risk:
  - `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets/material`
    - only if org enables secrets export and caller has `secrets:read-material`
    - default stance is to not ship this in v1 unless required

### Volumes
Volumes exist and are attached via mounts.

- `GET  /v1/orgs/{org_id}/volumes`
- `POST /v1/orgs/{org_id}/volumes`
- `GET  /v1/orgs/{org_id}/volumes/{volume_id}`
- `DELETE /v1/orgs/{org_id}/volumes/{volume_id}`

Attachments (env-scoped):
- `POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments`
  - request:
    - volume_id
    - process_type
    - mount_path
    - read_only
  - response: attachment id

- `DELETE /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments/{attachment_id}`

Snapshots/backups (operator vs tenant)
v1 recommendation:
- tenants can request snapshot of their volume, but backup scheduling is platform policy.

- `POST /v1/orgs/{org_id}/volumes/{volume_id}/snapshots`
- `GET  /v1/orgs/{org_id}/volumes/{volume_id}/snapshots`
- `POST /v1/orgs/{org_id}/volumes/{volume_id}/restore`
  - creates a new volume from a snapshot

### Logs
Logs are read-only.

- `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs`
  - query params:
    - `process_type`
    - `instance_id`
    - `since`
    - `until`
    - `tail_lines`
  - response:
    - either a chunked stream or a paginated set

v1 recommendation:
- Provide a streaming endpoint for tailing:
  - `GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs/stream`

Rate limits:
- enforce max concurrent streams per org and per token.

### Exec
Exec is high risk and must be audited.

Two-step model:
1) create grant
- `POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{instance_id}/exec`
  - request: command and optional tty flag
  - response: exec session id, short-lived token, connect URL

2) connect to session
- either WebSocket or a dedicated streaming endpoint, defined in OpenAPI

Rules:
- exec grants expire quickly (example 60 seconds to connect)
- sessions have max duration and are terminated by server
- full audit required

### Events (debugging)
Expose an org-scoped event tail for debugging.

- `GET /v1/orgs/{org_id}/events`
  - query:
    - `after_event_id`
    - `limit`
    - optional filters: env_id, app_id, event_type

This endpoint is key for “why is it not converging”.

## Concurrency control and preconditions
For updates that can conflict (routes, scale), support one of:
- `If-Match` with an object version, or
- explicit `expected_version` field in request

v1 recommendation:
- return `resource_version` on objects and accept `expected_version` on PATCH/PUT.
- On mismatch, return `409 conflict` with code `version_conflict`.

## Minimal object fields (consistency requirements)
Every resource returned should include:
- `id`
- `org_id` (if applicable)
- `created_at`
- `updated_at` (if mutable)
- `resource_version` (for mutable objects)
- `links` (optional, but helpful for UI later)

## Notes on OpenAPI
- This document is narrative. The OpenAPI file must be updated to match.
- The OpenAPI file is what generates clients and validates server behavior.

## Open questions to resolve next
- Release creation pattern A vs B (recommendation: A, explicit release object).
- Exec transport: WebSocket vs server-sent events vs gRPC tunnel.
- Whether to expose secrets material read at all in v1.
- Hostname uniqueness scope: global vs per org (recommendation: global across platform, enforced by route creation).

## Implementation plan

### Current code status
- **API skeleton**: Basic HTTP server exists in `services/control-plane/src/api/`.
- **Auth endpoints**: Device flow partially implemented.
- **Resource endpoints**: Org/app/env CRUD scaffolded; not complete.
- **OpenAPI spec**: Draft exists at `docs/specs/api/openapi.yaml`.

### Remaining work
| Task | Owner | Milestone | Status |
|------|-------|-----------|--------|
| Auth endpoints (device flow, token, refresh, revoke) | Team Control | M1 | Partial |
| Org/project/app/env CRUD endpoints | Team Control | M1 | Partial |
| Release creation endpoint (Pattern A) | Team Control | M2 | Not started |
| Deploy and rollback endpoints | Team Control | M2 | Not started |
| Scale endpoints (GET/PUT) | Team Control | M2 | Not started |
| Instance list endpoint (runtime view) | Team Control | M1 | Not started |
| Route CRUD endpoints | Team Control | M4 | Not started |
| Secrets PUT endpoint with versioning | Team Control | M5 | Not started |
| Volume and attachment endpoints | Team Control | M1 | Not started |
| Logs streaming endpoint | Team Control | M7 | Not started |
| Exec grant and session endpoints | Team Control | M7 | Not started |
| Events tail endpoint | Team Control | M7 | Not started |
| Idempotency key handling middleware | Team Control | M1 | Not started |
| Pagination and cursor implementation | Team Control | M1 | Not started |
| Error model standardization | Team Control | M1 | Not started |
| OpenAPI spec alignment with implementation | Team Control | M2 | Not started |

### Dependencies
- Auth spec (`auth.md`) must be finalized for token flows.
- Event model must be complete for events endpoint.
- Materialized views must be populated for list endpoints.

### Acceptance criteria
1. All v1 endpoints documented in OpenAPI and implemented.
2. Idempotency keys work for deploy, route, volume, secrets endpoints.
3. Pagination works consistently across all list endpoints.
4. Error responses match documented error model with stable codes.
5. CLI can complete full deploy flow using only HTTP API.
6. OpenAPI spec validates against implementation (CI check).
7. Rate limiting enforced per org and per token.
