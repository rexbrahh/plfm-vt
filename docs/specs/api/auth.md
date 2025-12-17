# docs/specs/api/auth.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
This document specifies authentication and authorization for the control plane API.

It defines:
- token types and lifetimes
- CLI login flow (device flow)
- service principals for automation
- scopes and permission model
- org context rules
- revocation and auditing requirements

This document does not define the full endpoint list. That lives in:
- `docs/specs/api/http-api.md`
- `docs/specs/api/openapi.yaml`

## Goals (v1)
- Strong org isolation. No cross-tenant reads or writes.
- CLI-first UX for humans.
- Scriptable automation via service principals.
- Short-lived access tokens with revocation.
- Explicit scopes with least privilege.
- Auditable sensitive actions.

## Non-goals (v1)
- SSO and enterprise IdP integrations.
- Fine-grained ABAC policy language.
- Public anonymous access.
- Long-lived, non-expiring bearer tokens by default.

## Client types
### Human user
A person using the CLI (and later, a web UI).

### Service principal
A non-human identity used by CI and automation.

### Infrastructure identity
Node agents and edge components. These do not use bearer tokens for normal operation. They authenticate with mTLS and are enrolled by operators. Node enrollment is a separate spec.

## Transport security
- All HTTP API calls are over TLS.
- No plaintext HTTP is supported.
- Tokens are bearer tokens. Leakage equals access until revoked or expired.

## Token model

### Token types
#### Access token
- Short-lived bearer token used on all authenticated API calls.
- Presented as `Authorization: Bearer <token>`.

#### Refresh token
- Longer-lived token used to obtain new access tokens.
- Never sent to endpoints other than the refresh and revoke endpoints.
- Stored securely by the CLI.

#### Device code
- Short-lived code used during device flow login.

#### Service principal credential
- A client credential (client id and secret) used only to mint access tokens.
- This is not used directly as a bearer token.

### Token format
- Tokens are opaque strings.
- Tokens must be stored hashed at rest in the control plane database.
- Tokens include a prefix to indicate type, example:
  - `trc_at_...` access token
  - `trc_rt_...` refresh token
  - `trc_dc_...` device code

Rationale:
- Opaque tokens allow server-side revocation without JWT validation complexity.
- Hashing reduces damage if the DB is exposed.

### Lifetimes (recommended defaults)
- Access token: 15 minutes
- Refresh token: 30 days
- Device code: 10 minutes
- Service principal access token: 15 minutes (same as normal access token)

The server may tighten lifetimes later. Clients must handle expiry and refresh.

## Org context
Every authenticated request is authorized in an org context.

Rules:
- Requests that operate on tenant resources must include `org_id` in the URL path.
- The server verifies the caller is a member of that org and has the required scope.
- Tokens are not permanently bound to a single org. A user can operate on multiple orgs, but each request is scoped to exactly one org by the path.

Example path shape (illustrative):
- `/v1/orgs/{org_id}/apps`
- `/v1/orgs/{org_id}/envs/{env_id}/deploys`

The `whoami` endpoint returns the list of orgs and roles for the token subject.

## Authentication flows

### 1) CLI login for humans (device flow)
The CLI uses a device authorization flow, similar to OAuth device code.

#### Step A: start device authorization
Client calls:
- `POST /v1/auth/device/start`

Response fields:
- `device_code` (opaque)
- `user_code` (short code user types)
- `verification_uri` (URL for user)
- `verification_uri_complete` (URL with code embedded, optional)
- `expires_in_seconds`
- `poll_interval_seconds`

#### Step B: user approves in browser
User opens `verification_uri` and authenticates (web UI or hosted auth page), then approves the device login request.

The approval UI must show:
- requesting device name (CLI provided)
- requested scopes
- org context is not selected here, membership is evaluated later per request

#### Step C: CLI polls for token
Client calls:
- `POST /v1/auth/device/token`

Request fields:
- `device_code`

Responses:
- Success returns:
  - `access_token`
  - `refresh_token`
  - `expires_in_seconds`
- Pending returns a structured error:
  - `authorization_pending`
- Slow down returns:
  - `slow_down`
- Expired returns:
  - `expired_token`
- Denied returns:
  - `access_denied`

Poll rules:
- CLI must not poll faster than `poll_interval_seconds`.
- Server may rate limit device polling per device_code and per IP.

### 2) Refreshing an access token
Client calls:
- `POST /v1/auth/token/refresh`

Request fields:
- `refresh_token`

Response:
- new `access_token`
- optional new `refresh_token` (rotation)

Refresh token rotation:
- Recommended in v1. Each refresh issues a new refresh token and revokes the old one.
- If rotation is enabled, the client must store the new refresh token immediately.

### 3) Revoking tokens
Client calls:
- `POST /v1/auth/token/revoke`

Request fields:
- one of:
  - `refresh_token`
  - `access_token` (optional support)

Server behavior:
- Revoking a refresh token also revokes its active access tokens if the server tracks linkage.
- Revocation is idempotent.

### 4) Service principals (automation)
Service principals authenticate using client credentials to mint short-lived access tokens.

Client calls:
- `POST /v1/auth/token`

Request fields:
- `grant_type = "client_credentials"`
- `client_id`
- `client_secret`
- optional `scopes` override (must be a subset of the principal’s allowed scopes)

Response:
- `access_token`
- `expires_in_seconds`

Rules:
- Client credentials are created and rotated by org admins.
- Client secrets are only shown once at creation time.
- Client secrets are stored hashed at rest.

Service principals do not receive refresh tokens by default in v1. They are expected to request new access tokens as needed.

### 5) Introspection and identity
Endpoint:
- `GET /v1/auth/whoami`

Returns:
- subject type (user or service principal)
- subject id
- display name (if user)
- org memberships (org id, role)
- effective scopes (either global or per org, depending on implementation)

This endpoint exists to make CLI context selection reliable and debuggable.

## Authorization model

### Scopes
Scopes are the enforcement unit. Roles map to scope bundles.

Scope naming conventions:
- `resource:action` where action is one of: `read`, `write`, `admin`
- Some sensitive actions use more explicit verbs.

Recommended v1 scope set:
- `orgs:read`
- `orgs:admin` (org membership, billing settings)
- `apps:read`, `apps:write`
- `envs:read`, `envs:write`
- `releases:read`, `releases:write`
- `deploys:write` (create deploy intents)
- `rollbacks:write`
- `routes:read`, `routes:write`
- `volumes:read`, `volumes:write`
- `secrets:read-metadata`, `secrets:write`
- `secrets:read-material` (high risk, disabled by default in v1 unless explicitly enabled)
- `logs:read`
- `exec:write` (high risk)
- `billing:read`, `billing:write`
- `nodes:admin` (operator-only, infrastructure scope)

### Roles (recommended mapping)
This is guidance. The exact mapping is configured by the control plane but must be documented.

- Owner:
  - includes all org scopes except `nodes:admin`
  - may include `secrets:read-material` only if org enables it
- Admin:
  - similar to Owner but may exclude billing and secrets material
- Developer:
  - deploy, rollback, routes write, logs read, volumes write
  - secrets write but not secrets material read
  - exec write optional, off by default
- ReadOnly:
  - read scopes and logs read
  - no write scopes

### Least privilege rules
- Deploy rights do not imply secrets material read.
- Logs read does not imply exec.
- Route write does not imply IPv4 allocation unless explicitly granted.
- Infrastructure actions (node enrollment, overlay membership) are never tenant-scoped.

## Sensitive operation requirements
The following actions must be audited with actor identity and request metadata:
- secrets create and update
- any secrets material read (if enabled)
- route create, update, delete
- IPv4 add-on enablement and port changes
- exec session grants
- org membership and role changes
- service principal creation and rotation

Audit events must not include raw secret material.

## Error model (auth-specific)
Auth endpoints return structured errors with:
- `code` (stable string)
- `message` (human-readable)
- `retryable` (bool)
- optional `details` (object)

Common codes:
- `unauthorized`
- `forbidden`
- `token_expired`
- `token_revoked`
- `insufficient_scope`
- `org_access_denied`
- `authorization_pending`
- `slow_down`
- `expired_token`
- `access_denied`

## Rate limits (minimum)
- Device token polling is rate-limited per device_code and per IP.
- Login start is rate-limited per IP.
- Token minting endpoints are rate-limited per subject.
- Logs and exec endpoints have additional rate limits (specified elsewhere).

## Security notes
- Tokens are bearer tokens. Treat them like passwords.
- Refresh tokens must be stored securely by clients.
- Client secrets must never appear in logs.
- Revocation must be fast and reliable.

## Open questions (to resolve in api/http-api and implementation)
- Whether access tokens are stored and validated via DB lookup on every request, or via cached introspection with short TTL.
- Whether to support personal access tokens (PATs) in v1, or require device flow for humans and client credentials for automation.
- Whether to allow `secrets:read-material` at all in v1, or gate it behind an org-level "allow secrets export" setting.
