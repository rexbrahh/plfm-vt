# Implementing a new resource end-to-end

This is a scoped checklist for adding a new resource (or a major capability) safely.

A “resource” here means something like: org, project, app, env, release, workload, endpoint, volume, secret bundle, event stream.

The goal is consistent behavior across:
- API contracts
- reconciliation semantics
- events and introspection
- CLI product experience
- compatibility guarantees

## 0. Define the resource precisely

Write down:
- user-facing name (singular and plural)
- stable ID format and validation rules
- lifecycle states and terminal states
- desired fields (spec) vs observed fields (status)
- which component owns reconciliation (control-plane only, node-agent, ingress, or both)

If the resource affects networking, secrets, or workload isolation, treat it as security-sensitive by default.

## 1. Contracts first (schemas and API)

1) Add or update schema in `api/schemas/`
- Include a `version` field if this resource will evolve (recommended).
- Define required vs optional fields explicitly.
- Add defaults where safe.

2) Update OpenAPI in `api/openapi/`
- CRUD routes (or the minimal subset you intend to support)
- error shapes (consistent error codes and messages)
- pagination semantics for list endpoints

3) Add example payloads in `api/examples/`
- minimal valid object
- typical object
- failure examples (validation, conflict, not found)

4) Add contract tests
- schema validation tests for the examples
- OpenAPI lint and validation in CI

## 2. Persistence and IDs

- Implement canonical ID parsing and validation (one library, reused everywhere).
- Store stable IDs, do not rely on names for identity.
- Define uniqueness and constraints (per org, per project, etc).
- Plan migrations before shipping breaking changes.

## 3. Reconciliation and state model

Decide the reconciliation boundary:
- control-plane reconciles into desired assignments and high-level intents
- node-agent reconciles runtime state (VMs, mounts, secrets materialization)
- ingress reconciles edge configuration (L4 routing, IPs, proxy protocol)

Implement:
- spec to desired state translation (server-side)
- status updates from observers (node-agent, ingress)
- convergence rules:
  - what does “ready” mean
  - what does “degraded” mean
  - which failures are retryable and how backoff works

Hard rule:
- reconcile loops must be safe to run repeatedly and must tolerate partial failure.

## 4. Events and introspection (required)

Define event types for:
- create, update, delete requests (receipt or acknowledgement)
- reconciliation actions started and completed
- failures (with clear error categories)
- state transitions (pending to ready, ready to degraded, etc)

Add:
- event payload schema
- producer emission points in code
- consumer tooling support (CLI events tailing, filtering by resource ID)

If a user cannot debug it with events and describe output, it is not done.

## 5. CLI product surface (required)

Add CLI commands:
- list
- describe
- create or apply (depending on workflow)
- update (or apply)
- delete (if supported)

CLI behavior rules:
- mutations print a receipt:
  - what was requested (desired)
  - what is currently observed (may lag)
  - next commands to wait or inspect (`events tail`, `describe`, `status`)
- include `--json` output with a stable schema
- consistent exit codes for:
  - validation errors
  - auth errors
  - not found
  - conflicts
  - transient server or network failures

If the resource is part of release creation, ensure any required gates are enforced (env and secrets workflow gate is a first-class example).

## 6. Security and secrets (if applicable)

If the resource touches secrets or sensitive configuration:
- never log secret values
- add redaction tests
- ensure errors do not include secret payloads
- ensure storage is encrypted at rest where required by policy
- ensure access control checks exist on every API entry point

If the resource causes a new attack surface (ingress, VM lifecycle, file mounts):
- add a short threat note in `docs/security/` describing:
  - attacker goal
  - entry points
  - mitigations
  - what to monitor

## 7. Observability hooks (minimum bar)

Add:
- metrics for reconcile success and failure counts
- latency for reconcile convergence where meaningful
- structured logs with correlation IDs (request ID, resource ID)

Ensure:
- logs are structured
- logs are deterministic enough for tests where needed
- no sensitive fields are emitted

## 8. Testing requirements by layer

Minimum required for merge:
- unit tests for validation and core logic
- contract tests for schema and API
- at least one integration path that proves reconciliation and status updates

Recommended before release:
- E2E test through CLI for the happy path
- failure test for at least one realistic failure mode (retry, timeout, partial availability)

For networking or runtime resources:
- add a small perf baseline scenario once behavior is stable

## 9. Compatibility and rollout plan

If you changed any contract:
- update `compatibility-and-versioning.md`
- add fixtures that simulate older client inputs where needed
- ensure server defaulting and unknown-field behavior is correct

Rollout:
- add feature discovery or capability flags if old components might not support it
- prefer additive changes first
- avoid irreversible migrations without rollback steps

## 10. Documentation updates

Update:
- relevant architecture doc
- relevant CLI doc
- engineering docs if new patterns were introduced
- examples and templates if this is user-facing

A resource is complete only when a new contributor can implement and debug it using docs alone.
