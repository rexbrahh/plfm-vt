# Authorization and scopes

Last updated: 2025-12-17

This document defines the authorization model for the control plane, CLI, and web console. The goal is to make privilege boundaries explicit, minimize confused deputy risk, and support safe automation.

## Principles

- Default deny: access is denied unless explicitly granted.
- Stable ids: authorization is evaluated on immutable resource ids, not human names.
- Least privilege: scopes are narrow and time bound where practical.
- Separation of duties: sensitive actions require elevated roles or additional confirmation.
- Defense in depth: enforce authz at every hop that can change state.

## Identity types

### Human users

- Auth via SSO (OIDC) and optional MFA.
- Personal tokens are short lived where feasible.
- Session tokens for web console are bound to origin and use secure cookies.

### Service identities

- Org or project scoped service accounts for CI and automation.
- Tokens are audience restricted and have explicit scopes.
- Rotate and revoke without affecting human access.

### Node identities

- Each host node has an identity used for control plane to host plane communications.
- Node identities cannot act as customers and cannot request customer scoped data beyond what is required for placement and reconciliation.

## Resource hierarchy

Authorization is evaluated relative to the resource tree.

- org
  - project
    - app
      - env
        - release
        - workload (instance groups, instances)
        - endpoint
        - volume
        - secret bundle
        - events and logs (streams and queries)

Invariants:
- No token may cross org boundaries.
- Most operations must be scoped to an env. Cross env operations should be explicit and rare (for example copying config between envs).

## Roles

Roles are coarse, scopes are fine. A role maps to a set of allowed actions per resource type.

Suggested baseline roles per org or project:

- Owner
  - Full control including billing and org policy.
- Admin
  - Full control excluding billing and org ownership changes.
- Operator
  - Can deploy, scale, view logs, and manage endpoints. Cannot manage org membership or security policy.
- Developer
  - Can deploy to specific envs, view logs, and manage app config. Cannot manage endpoints or volumes unless granted.
- Viewer
  - Read only access to non sensitive resources.
- Billing
  - Billing only, no runtime access.

Suggested baseline roles per app env:

- Env Admin
- Env Operator
- Env Viewer

## Actions and scopes

Scopes should be explicit in tokens and in audit logs.

### Common verbs

- `read`
- `write`
- `delete`
- `list`
- `exec` (interactive session or remote command execution)
- `logs:read`
- `events:read`
- `secrets:write`
- `secrets:read-metadata` (never secret values)
- `endpoints:write`
- `volumes:write`
- `releases:promote`

### Examples

- Deploy pipeline token for a single env:
  - `org:{org_id}:project:{project_id}:app:{app_id}:env:{env_id}:releases:write`
  - `...:workloads:write`
  - `...:secrets:read-metadata`
  - `...:events:read`

- Read only production observer:
  - `...:workloads:read`
  - `...:logs:read`
  - `...:events:read`

- Break glass token (rare, audited, time limited):
  - `org:{org_id}:*` with explicit expiry and mandatory reason field.

## Sensitive operations

Operations that should have additional protections beyond normal scopes.

- Managing org membership and role bindings.
- Creating or updating endpoints that expose workloads publicly.
- Enabling Proxy Protocol v2 on an endpoint.
- Reading logs for production envs if logs can contain sensitive data.
- Starting interactive exec sessions into workloads.
- Creating, rotating, or deleting secret bundles.
- Volume snapshot restore into an env.

Recommended protections:
- Step up authentication (MFA) for web console.
- Just in time elevation for human users.
- Mandatory reason strings stored in audit logs.
- Separate scopes for sensitive actions, not implied by broad write scopes.

## Policy evaluation model

- Evaluate authz in a central policy engine or library shared by services.
- Use subject (identity), action, resource (immutable id), and context (env, region, time, IP).
- Deny if any required resource ownership checks fail.
- Deny if scope does not match action and resource path.

Context based constraints that are reasonable in v1:
- Token expiry and not before timestamps.
- Token audience and issuer binding.
- IP allowlists for high risk tokens (optional per org).
- Environment protection flags (for example production requires step up auth).

## Confused deputy prevention

Rules:

- Never accept resource ids from a caller and use them as authority without verifying ownership.
- Internal services must authenticate to each other and include an explicit actor identity.
- Host agents must not be able to request secrets or volumes without a control plane signed instruction that binds:
  - node id
  - workload id
  - env id
  - secret bundle ids and volume ids
  - expiry

## Eventual consistency considerations

- Authorization decisions for interactive requests must be made against authoritative control plane state, not eventually consistent materialized views.
- Caches may be used for read performance, but must not widen access on cache miss or staleness.
- Signed instructions to host agents provide a stable security boundary even if host agents have stale local state.

## Token management

- Default personal tokens should be short lived with refresh tokens where supported.
- Service account tokens should have explicit maximum TTL, with rotation tooling.
- Revocation must be fast. Maintain a revocation list for high risk tokens and sessions.
- Store tokens securely in CLI keychain where available, and avoid printing tokens in terminal output.

## Enforcement points

Authz must be enforced at:

- API gateway or edge API service.
- Each control plane service that mutates state.
- Scheduler actions that change placement or scaling decisions.
- Host agent actions (validate signed instructions and local policy).
- Log and event read paths, including streaming endpoints.

## Testing

- Golden tests for policy rules.
- Negative tests for cross org and cross env access.
- Fuzz tests for resource parsing and scope evaluation.
- Replay tests: ensure request ids and short lived tokens reduce replay risk on sensitive endpoints.
