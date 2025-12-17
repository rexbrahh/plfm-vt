# docs/specs/secrets/delivery.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define secrets delivery semantics:
- where secrets are mounted in the microVM
- when secrets are updated and how rotation works
- how secrets integrate with the scheduler and WorkloadSpec
- restart semantics (v1 default)
- failure behavior and reason codes
- observability requirements

Locked decision: secrets are delivered as a fixed-format file. See `docs/adr/0010-secrets-delivery-file-format.md`.

Exact file format is defined in:
- `docs/specs/secrets/format.md`

Encryption-at-rest is defined in:
- `docs/specs/secrets/encryption-at-rest.md`

## Scope
This spec defines delivery and lifecycle semantics.

This spec does not define:
- the on-disk file syntax (`format.md`)
- storage encryption design (`encryption-at-rest.md`)
- exec transport or guest init details beyond delivery requirements (runtime specs)

## Definitions
- **Secret bundle**: env-scoped container for secrets (one bundle per env in v1).
- **Secret version**: immutable version of a secret bundle.
- **Secrets file**: mounted file inside microVM containing secrets in platform format.
- **Rotation**: producing a new secret version and applying it to running workloads.

## v1 stance
1) Secrets are scoped to `(org, app, env)`.
2) Secrets are delivered to workloads as a file inside the microVM at a fixed path.
3) Secrets rotation uses restart semantics by default:
- new secret version triggers a rollout restart (no hot reload)
4) Secrets material must not be stored in plaintext on host persistent disk by default.
5) Secrets delivery failures must fail the instance clearly.

## Delivery endpoint inside guest (fixed)
### Mount path (v1 fixed)
- `/run/secrets/platform.env`

### Filesystem expectations
- `/run` is tmpfs (see runtime boot spec)
- secrets file is written by guest init into tmpfs

Rationale:
- tmpfs reduces persistent exposure risk
- ensures file disappears on microVM stop

### Permissions (v1 default)
- mode `0400`
- owner `uid=0`, `gid=0`

If the workload is configured to run as non-root and must read secrets:
- guest init may write the file as:
  - owner set to workload uid/gid
  - mode `0400`

v1 recommendation:
- keep it simple. Default root-only and provide a manifest/env option later if needed. If you add it now, document it explicitly in manifest spec.

## Control plane model
### Bundle and versioning
- Each env may have at most one active secret bundle (v1).
- Bundle has an immutable id.
- Updates produce new secret versions:
  - version id is immutable
  - version id references encrypted-at-rest storage

Events:
- `secret_bundle.created`
- `secret_bundle.version_set`

Materialized view:
- `secret_bundles_view` stores:
  - env_id
  - bundle_id
  - current_version_id
  - updated_at

## Scheduler integration
### Required data in WorkloadSpec
WorkloadSpec must carry enough info for agent and guest init to deliver secrets:
- `secrets.required` (bool)
- `secrets.secret_version_id` (string, optional)
- `secrets.mount_path` (fixed to `/run/secrets/platform.env`)
- optional uid/gid/mode if supported

### Required semantics
- If a process type sets `secrets.required=true` and there is no current_version_id for the env:
  - scheduler must treat the group as unschedulable with reason `secrets_missing`
  - scheduler must not allocate instances that will crash-loop

- If secrets are not required and env has no bundle:
  - WorkloadSpec must either omit secrets section or explicitly indicate no secrets.

## Host agent and guest init responsibilities
### Delivery mechanism
v1 normative approach:
- secret material is transferred from host agent to guest init over a control channel (vsock).
- guest init writes the secrets file to `/run/secrets/platform.env` with correct permissions.

Host-side rules:
- host agent must not persist plaintext secrets to disk in normal operation.
- if a temporary file is unavoidable:
  - it must be on tmpfs
  - it must be deleted immediately after use

Guest-side rules:
- guest init must never log secrets content
- guest init must write file atomically:
  - write to temp path
  - fsync if applicable (tmpfs may ignore)
  - rename to final path

Atomicity prevents partial reads.

### Ordering at boot (normative)
Guest init must:
1) set up `/run` tmpfs
2) write secrets file (if required or provided)
3) mount volumes
4) start workload command

If secrets are required and cannot be delivered:
- guest init must fail fast and exit non-zero.

## Rotation semantics (v1)
### Default: restart-based rollout
When a new secret version is set for an env:
- the desired secrets version for every process type in that env changes
- this changes the group_spec_hash
- scheduler triggers a rollout restart

Stateless process types:
- rolling replacement (surge then drain) per scheduler rollout rules

Stateful process types (volumes):
- replace-in-place (drain/stop then start) because volumes are exclusive

### No hot reload in v1
The platform does not rewrite secrets files in-place for running instances as a correctness guarantee.

Reasons:
- many apps read secrets only at startup
- in-place mutation creates inconsistent behavior and hard-to-debug incidents
- restart semantics align with event log and scheduling model

### Rollback of secrets
If a secrets update breaks workloads:
- operator/user can roll back by setting secrets to the previous version:
  - either by reapplying the old content (creates a new version identical to old), or
  - by selecting a previous version id if you add that API later

v1 recommendation:
- simplest is reapply old content, producing a new version with same data_hash.
- track data_hash so you can see equivalence.

## Failure behavior and reason codes
### Missing secrets when required
If secrets.required is true and env has no bundle/version:
- scheduler should not allocate instances
If it somehow happens and instance starts anyway:
- agent or guest init must fail with:
  - reason code `secrets_missing`

### Secrets injection failure
If secrets exist but cannot be delivered or written:
- fail instance with:
  - reason code `secrets_injection_failed`
  - reason_detail examples:
    - `decrypt_failed`
    - `vsock_transfer_failed`
    - `write_failed`
    - `permissions_failed`
    - `format_invalid` (should not happen if validated at ingest)

### Secrets leaked in logs
This is a severe incident.
- must be treated as security incident
- rotate affected secrets immediately
- add a postmortem and tooling to prevent recurrence

## Validation at ingest (control plane)
When secrets are set via API:
- parse and validate against `docs/specs/secrets/format.md`
- canonicalize and compute data_hash
- encrypt and store
- emit secret version event with version_id and data_hash (no secret material)

Reject:
- invalid header
- invalid keys
- duplicate keys
- file too large
- too many keys

## Observability requirements
Control plane:
- track secret bundle current_version_id
- track secret updates (audit)

Agent:
- metrics:
  - secrets delivery success/failure counts
  - delivery latency
- logs:
  - instance_id, env_id, secret_version_id (metadata only)
  - failure reason_detail

CLI UX:
- `platform secrets set` prints:
  - new version id
  - data_hash
  - which env it applies to
- `platform env describe` shows current secret version id (metadata only)
- a debug command may render secrets file locally:
  - `platform secrets render` (requires authorization and may use local input, not server export)

v1 recommended security stance:
- do not provide “download secret material from server” by default.

## Open questions (future)
- Opt-in hot reload:
  - explicit `secrets.reload = hot` with signal semantics
- Per-process secrets bundles:
  - v1 is env-scoped only
- Multiple secret bundles per env:
  - not v1
- Non-root read policies as a first-class manifest option:
  - can be added, but must be audited and carefully validated
