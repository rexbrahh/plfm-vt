# Engineering docs

This folder contains implementation guides for contributors working on libghostty-vt PaaS.

These docs exist to keep the codebase consistent across teams and to prevent “tribal knowledge” from becoming a dependency.

## Read this first

Start here, in this order:
1. dev-environment.md
2. build-and-release.md
3. coding-standards.md
4. testing-strategy.md
5. compatibility-and-versioning.md
6. performance-testing.md

## Project invariants (non-negotiable)

- CLI is the primary customer interface.
- Manifest-first workflow with a minimal required manifest.
- Desired vs current state is always explicit.
- Mutations are idempotent and safe to retry.
- Secrets are delivered via control-plane reconciliation into a fixed file format.
- Ingress is L4 by default and does not terminate connections.
- IPv6 is the default everywhere; dedicated IPv4 is an explicit add-on.
- Every mutating CLI command prints a receipt and points to next steps (wait, events, describe).

If you are about to violate one of these, stop and write an ADR first.

## Where to find what

- Repo structure: repo-layout.md
- Local setup and running the stack: dev-environment.md
- CI and shipping artifacts: build-and-release.md
- Style, logging, errors, invariants: coding-standards.md
- Test layers and CI gates: testing-strategy.md
- Versioning and schema evolution: compatibility-and-versioning.md
- Perf methodology and regression gates: performance-testing.md

## How to change a contract safely

Contracts include:
- OpenAPI endpoints and request/response shapes
- Manifest schema and WorkloadSpec
- Event types and status fields
- CLI JSON output and exit codes

Rules:
- Update schema or OpenAPI first.
- Add contract tests and fixtures.
- Update compatibility-and-versioning.md if behavior changes affect old clients.
- Ensure deterministic output and ordering (golden tests must pass).

## What “done” means for platform features

A feature is not “done” if any of these are missing:
- Schema changes validated locally and in CI
- Server behavior with explicit desired vs current semantics
- Events emitted for state transitions and failures
- CLI receipt output and `--json` output
- Tests at the right layer (unit plus at least one integration or E2E path)
- Compatibility notes if clients might lag
- Secrets redaction and log safety (if any sensitive path is touched)

## Documentation expectations for PRs

A PR should update docs when it changes:
- user-facing behavior
- any contract
- any operational procedure
- any component interface

If the change is architectural, add or update an ADR.

## Contributor workflow (recommended)

- Run `just verify` before pushing.
- Keep changes scoped. Prefer multiple PRs over a mega PR.
- Add tests with the change, not after.
- If you touched schemas, update fixtures and compatibility tests in the same PR.

## Quick links

- Implementing a new resource end-to-end: implementing-a-new-resource.md
- Core loop demo (request-id/idempotency/RYW): core-loop-demo.md
- Staff handoff and 4-week execution plan: staff-handoff.md
