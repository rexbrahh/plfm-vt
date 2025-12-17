# docs/adr/0010-secrets-delivery-file-format.md

## Title

Secrets are delivered to workloads as a platform-managed file with a fixed format

## Status

Locked

## Context

We need a secrets model that:

* is easy for users to consume across languages
* avoids “secret sprawl” in environment variables
* is auditable and versionable
* supports rotation without requiring application rebuilds
* fits a microVM isolation boundary and a control plane event log model
* can be provided via CLI for local dev and debugging in a consistent way

We also want a stable contract so later we can add integrations (Vault, KMS, etc) without breaking workloads.

## Decision

1. **The primary delivery mechanism for workload secrets is a file mounted into the microVM**, using a fixed, versioned format defined by the platform.

2. **The platform CLI can render the same secrets file format locally** (or stream it to stdout) for local development and debugging, but the in-VM mechanism remains file-based.

3. **Secrets are environment-scoped.**

* a secret bundle is attached to `(org, app, env)`
* workloads for that env receive the bundle
* workloads for other envs must never receive it

4. **Secrets are not injected by default into environment variables.**

* env var injection may exist as an opt-in compatibility feature later
* v1 default is file-based delivery only

5. **The secrets file format is stable and versioned.**

* format includes metadata (format version, bundle id, generation time, optional key ids)
* content is key-value, with explicit encoding rules
* file permissions and ownership rules are part of the spec

## Format requirements (explicit)

These are required properties; the exact on-disk syntax is specified in `docs/specs/secrets/format.md`.

* Deterministic parsing
* Supports binary-safe values (either via base64 fields or explicit encoding)
* Does not allow ambiguous whitespace rules that break cross-language parsers
* Can represent hierarchical keys cleanly (or prohibits them explicitly)
* Includes a header with a format version

Examples of acceptable shapes:

* simple `KEY=VALUE` with strict escaping rules
* JSON with fixed schema
* TOML with strict typing rules

The exact choice is deferred to the secrets format spec, but the “fixed file” delivery is locked by this ADR.

## Rationale

* A file is a universal interface across languages and avoids env var leakage into process listings, crash dumps, and child processes by default.
* Mounting a file aligns with the microVM boundary and makes rotation and auditing easier.
* A fixed format prevents a slow drift into “everyone does secrets differently” across the platform.

## Consequences

### Positive

* Consistent secrets consumption across the ecosystem
* Easier rotation story (replace file atomically)
* Better containment of secrets exposure compared to env var defaults
* Easier auditing: “which env had which secret version”

### Negative

* Some frameworks assume env vars, so compatibility work may be needed
* Users must adapt their app to read from a file (or add small wrappers)
* We must implement secure file mounting, permissions, and update semantics carefully

## Alternatives considered

1. **Environment variables as default**
   Rejected because env vars leak easily and create unclear lifecycle boundaries.

2. **Sidecar secrets agent inside the VM**
   Rejected for v1 due to complexity, extra moving parts, and unclear ownership.

3. **External secret store dependency (Vault mandatory)**
   Rejected for v1 to keep dependencies minimal and reduce operational burden.

## Invariants to enforce

* Secrets are always scoped to `(org, app, env)` and cannot cross those boundaries.
* Secrets at rest are encrypted in the control plane storage model (how is defined elsewhere).
* Secrets file permissions are restrictive by default (root or dedicated app user readable only).
* Secrets file updates are atomic from the workload point of view.
* The platform must provide an audit trail: creation, update, rotation, access intent.

## What this explicitly does NOT mean

* We are not exposing secrets through a public metadata endpoint in v1.
* We are not promising dynamic per-request secret minting in v1.
* We are not letting workloads read other tenants’ secrets under any circumstances.
* We are not guaranteeing that secrets never appear in logs. Applications still must behave responsibly.

## Open questions

* Exact file syntax: env-style vs JSON vs TOML (must be decided in the secrets format spec).
* Rotation semantics: automatic refresh, on-deploy only, or periodic reconciliation.
* Whether we provide a standard library or helper for common languages (nice-to-have, not required for v1).

Proceed to **ADR 0011** when ready.
