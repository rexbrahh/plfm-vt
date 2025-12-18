# docs/README.md

## What this directory is
This `docs/` tree is the long-lived, human-readable source of truth for the platform. It exists so we can build for months without losing context, and so new contributors can onboard without tribal knowledge.

## What the project is
A developer-focused PaaS:

- Users ship **OCI images** plus a small **platform manifest**.
- The primary product surface is a **CLI** (onboarding, deploy, logs, exec, rollback, scale, routes, volumes, secrets).
- Workloads run in **microVMs** (Firecracker) on bare metal hosts.
- Hosts are connected by a **WireGuard IPv6 overlay**.
- Ingress is **L4-first** with **SNI passthrough** for TLS.
- IPv6 is the default everywhere. IPv4 is a paid add-on when needed (especially for raw TCP).

## What is authoritative
When documents disagree, this is the precedence order:

1) `docs/ADRs/**`  
   Irreversible decisions. If a decision changes, we add a new ADR. We do not rewrite history.

2) `docs/specs/**`  
   Contracts and invariants. These are the behavioral truth for implementations.

3) `docs/specs/api/openapi.yaml`  
   The client-facing API contract. Generated clients should treat this as the truth.

4) Everything else (`docs/architecture/**`, `docs/product/**`, `docs/ops/**`, etc)  
   Narrative, guidance, examples, and runbooks. Useful, but they must not contradict ADRs and specs.

## Doc classes and expectations

### ADRs
- Purpose: record choices so we do not re-litigate them later.
- Change rule: add a new ADR if you are changing a locked decision.

### Specs
- Purpose: define contracts between components and with users.
- Change rule: changes require updating implementation and tests (or blocking implementation work until updated).

### Architecture narratives
- Purpose: explain how the system fits together and why.
- Change rule: should track the current ADRs and specs.

### Ops runbooks
- Purpose: the steps you follow at 3am.
- Change rule: must be executable, and should be updated after incidents.

## Maturity levels
Every doc should include these fields near the top:

- `Status`: planned | stub | draft | reviewed | locked
- `Owner`: @handle or team name
- `Last reviewed`: YYYY-MM-DD

Meaning:
- planned: exists in the index, not written yet
- stub: outline exists, not trustworthy
- draft: content exists, may change
- reviewed: content is considered accurate and usable
- locked: do not change without a new ADR or explicit review process

## How to update docs without creating chaos
- If you change behavior, update the spec and add tests in the same change set.
- If you change an irreversible decision, add a new ADR and update `DECISIONS-LOCKED.md`.
- If a narrative doc conflicts with a spec, fix the narrative doc.

## Quick navigation
Start here:
- `docs/DECISIONS-LOCKED.md`
- `docs/NONGOALS.md`
- `docs/product/03-core-user-flows.md`
- `docs/architecture/00-system-overview.md`
- `docs/specs/manifest/manifest-schema.md`
- `docs/specs/manifest/workload-spec.md`
- `docs/specs/state/event-log.md`
