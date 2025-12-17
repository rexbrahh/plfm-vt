# Secret handling

Last updated: 2025-12-17

This document defines how secrets are created, stored, delivered, used, rotated, and audited in the platform.

## Goals

- Confidentiality: secret values are only accessible to authorized identities and to the intended workloads.
- Integrity: secret values cannot be modified without authorization and are protected against rollback.
- Minimize exposure: avoid putting secrets into logs, metrics, crash dumps, and process listings.
- Operability: support rotation, revocation, and safe rollout with clear status in desired vs current state.

## Threats we are designing against

- Stolen user tokens used to read or modify secrets.
- Cross tenant access to secret bundles due to scoping bugs.
- Secret leakage via logs, debug endpoints, or support tooling.
- Host compromise or malicious node attempting to read secrets for other tenants.
- Supply chain compromise of images attempting to exfiltrate secrets.
- Accidental exposure via CLI output, shell history, or config files.

## Secret types

- Environment variables (key value pairs).
- File based secrets (opaque bytes, certificates, private keys).
- Structured bundles (multiple keys and files delivered together).
- Build time secrets (should be avoided or treated as high risk, and never baked into images).

## Storage and encryption at rest

- Secrets are stored encrypted at rest using envelope encryption.
- Use a KMS root key to wrap per tenant or per env data keys.
- Rotate data keys periodically and on suspicion of compromise.
- Store only secret metadata in plaintext (name, ids, creation time, last rotation time).

Requirements:
- Secret values are never returned by read APIs after creation. Only metadata can be retrieved.
- Secret creation returns a receipt and a reference id, not the secret value again.

## Delivery model

Secrets are delivered via control plane reconciliation into a fixed file format on the host, then mounted or injected into the microVM.

### Invariants

- Host agents do not fetch secrets by querying a secrets API. They only act on signed reconciliation instructions.
- Instructions must bind secret bundle ids to a workload id and env id, and include an expiry.
- Delivery is idempotent and supports eventual consistency by reporting current vs desired.

### On host handling

- Render secret material into a dedicated directory on tmpfs when possible.
- Use atomic write patterns:
  - write to a new directory
  - fsync and permission set
  - atomically swap a symlink or rename into place
- Set permissions to the minimal readable set for the VM boundary.
- Do not store secrets in world readable locations, and do not persist secrets to host disk unless explicitly required.

### In guest handling

- Prefer file based secrets mounted read only.
- If environment variables are used, keep them minimal and document the leak risks:
  - exposure via crash dumps
  - exposure via process inspection inside the guest
- Provide a well defined mount path and file naming scheme so apps can consume secrets consistently.

## Access control

- Only identities with `secrets:write` can create or rotate secret bundles.
- Only `secrets:read-metadata` is available for listing and auditing. There is no `secrets:read-value` for customers.
- Workloads receive secret values only if they are explicitly attached in the env or workload config.

## Rotation and revocation

Rotation is a first class operation.

- Rotate a secret bundle by creating a new version id and updating the attachment to workloads.
- Support staged rollout:
  - deliver new version
  - restart or signal workloads
  - confirm healthy
  - revoke old version
- Revocation must immediately prevent new deliveries and should trigger a reconcile that removes secret material from hosts where safe.

Operational requirements:
- The platform should surface rotation status in events and in `describe` outputs.
- A rotated secret should not be retrievable from audit logs or past API responses.

## Preventing leaks

### Logging and events

- Never log secret values.
- Redact common secret patterns in logs and events, but treat redaction as a safety net, not the primary control.
- Ensure debug tooling never prints secret values even in verbose modes.

### CLI hygiene

- Avoid commands that require passing secret values on the command line.
- Prefer reading secret input from stdin or from a file path.
- Warn when the user tries to set a secret via shell argument because it can leak into shell history.

### Support and incident tooling

- Support staff access is role based and should not include secret values.
- Any break glass access must be time limited, require reason, and be heavily audited.

## Backup and snapshot interaction

- Secret stores must not be included in general backups that are exported without strict controls.
- If secret metadata is backed up, secret values remain encrypted and require KMS access to decrypt.
- Volume snapshots can contain secrets stored by the app. This is a customer responsibility. Provide documentation and guidance.

## Detection

- Alerts for unusual secret rotation frequency.
- Alerts for repeated denied attempts to mutate secrets.
- Alerts for host agent failures that could indicate tampering.
- Periodic verification that no secret values appear in logs (automated scanning).

## Testing

- Negative tests for cross tenant secret bundle access.
- Tests for host crash during secret delivery (atomicity).
- Tests for revoke and rotation correctness.
- Regression tests to ensure secrets never appear in logs or error messages.
