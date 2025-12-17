# Build and release

This document standardizes how we build and ship:
- CLI (primary customer interface)
- control-plane services
- node-agent runtime
- ingress components
- web console + web terminal (WASM)

## Build principles

- Reproducible builds are the default.
- CI is the source of truth for release artifacts.
- Every artifact is versioned and traceable to a commit.
- Supply chain outputs are required for shipped artifacts:
  - SBOM
  - signatures or provenance (where feasible)

## Artifact types

CLI:
- macOS (arm64, amd64)
- Linux (amd64, arm64)
- Windows (amd64)

Services:
- OCI images for control-plane, ingress, node-agent, observability components

Frontend:
- static assets for console and web-terminal
- WASM bundles for libghostty-vt integration

## Local build commands (standard)

Define these targets and keep them working:
- `just fmt`
- `just lint`
- `just test`
- `just build`
- `just build-cli`
- `just build-images`
- `just build-web`

Optional but recommended:
- `just sbom`
- `just verify` (runs fmt + lint + test)

## CI pipeline shape

Stage 1: Validation
- formatting
- linting
- schema validation for `api/` (OpenAPI, JSON schema)
- generated code checks (no dirty tree)

Stage 2: Tests
- unit tests
- integration tests
- contract tests (API and schema compatibility)

Stage 3: Build
- build CLI binaries (matrix)
- build OCI images
- build frontend assets
- attach build metadata (commit, timestamp, version)

Stage 4: Supply chain
- generate SBOMs
- sign artifacts
- provenance attestation (if used)

Stage 5: Publish
- publish images to registry
- publish CLI releases
- publish frontend assets

## Versioning overview

Follow the compatibility policy in:
- docs/engineering/compatibility-and-versioning.md

In short:
- CLI is SemVer.
- API and schema changes are tracked and versioned explicitly.
- Runtime and ingress components must remain compatible with the control-plane for a defined window.

## Release process (operator checklist)

1. Prepare the release PR
- bump version(s)
- update changelog
- confirm compatibility notes if schemas changed
- verify `just verify` passes locally

2. Merge to main
- CI must pass on main for release

3. Tag the release
- `vX.Y.Z` tags for CLI and user-facing surfaces
- service images are stamped with:
  - `vX.Y.Z`
  - `git-<sha>`

4. CI publishes artifacts
- CLI binaries attached to release
- images pushed to registry
- SBOM and signatures attached

5. Post-release validation
- smoke test:
  - install CLI from release artifact
  - `ghostctl version`
  - `ghostctl status` against staging
- deploy to staging via documented procedure
- monitor events and error rates

## Rollback strategy

- Services: redeploy previous image tag.
- CLI: customers may lag. Never assume immediate CLI upgrade.
- Schema: avoid irreversible migrations without a rollback plan.

## Release gating rules

A release must be blocked if:
- contract tests fail
- compatibility checks fail (old CLI cannot talk to current server within policy)
- secrets redaction tests fail (no secrets must appear in logs)
- performance regression thresholds are exceeded (if perf gates exist)

## Releasing schema changes

When changing:
- manifest schema
- WorkloadSpec schema
- event types

You must:
- update schema docs in `api/schemas`
- add migration or defaulting behavior
- add compatibility tests
- include a note in compatibility doc and changelog
