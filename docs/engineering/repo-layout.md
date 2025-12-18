# Repo layout

This document defines the canonical repository layout for libghostty-vt PaaS.

Goals:
- Make ownership boundaries obvious across multiple teams.
- Keep interfaces explicit (API, schemas, events, contracts).
- Ensure every component is testable in isolation and together.
- Keep customer-facing surfaces (CLI, console, web terminal) first-class.

Changing the layout:
- Structural changes require an ADR (`docs/ADRs`) and an update to this doc.

## Top level directories

Recommended monorepo layout:

- docs/
  - adr/                      Architecture decision records
  - architecture/             System design docs (control plane, runtime, networking, storage, secrets, observability)
  - cli/                      Customer-facing CLI documentation
  - product/                  Product and UX docs
  - frontend/                 Console + web terminal docs
  - security/                 Threat model, attack surfaces, hardening guides
  - devops/                   Runbooks, oncall, deploy procedures
  - engineering/              Contributor guides (this folder)

- api/
  - openapi/                  REST API specifications (OpenAPI)
  - schemas/                  JSON Schema for manifests, WorkloadSpec, events
  - examples/                 Example payloads and fixtures used by tests
  - changelog/                API change notes and compatibility notes

- services/
  - control-plane/            API server + auth + reconciliation + scheduler entrypoint
  - node-agent/               Per-node agent: VM lifecycle, image cache, secrets materialization, volumes
  - ingress/                  L4 ingress controller / edge components (IPv6-first, IPv4 add-on, proxy protocol)
  - observability/            Log/metric collection components if not embedded elsewhere
  - web-console/              Backend for console if distinct from control-plane API

- cli/
  - ghostctl/                 The CLI product (primary customer interface)
  - internal/                 Shared CLI libs (formatting, output, HTTP client, auth, manifest parser)

- frontend/
  - console/                  Web console UI
  - web-terminal/             Web terminal app using libghostty-vt via WASM
  - packages/                 Shared frontend packages (design system, API client, terminal glue)

- libs/
  - id/                       Stable ID types, parsing, validation
  - events/                   Event type definitions, serializers, validators
  - reconcile/                Reconciliation framework (desired vs current, diffing, apply)
  - secrets-format/           Secret file format encoder/decoder and validators
  - networking/               Overlay utils, IPAM helpers, proxy protocol helpers
  - testing/                  Shared test harness utilities

- deploy/
  - environments/             dev, staging, prod overlays
  - terraform/                Infra as code (if applicable)
  - ansible/                  Host provisioning (if applicable)
  - nix/                      Nix modules or flake helpers (if applicable)
  - images/                   Base images used by runtime

- scripts/
  - bootstrap/                One-shot machine setup helpers
  - dev/                      Local dev orchestration helpers
  - ci/                       CI helper scripts (lint, sbom, signing)

- test/
  - integration/              Multi-service integration tests and fixtures
  - e2e/                      End-to-end tests exercising CLI against a real cluster
  - perf/                     Load and performance harnesses
  - chaos/                    Failure injection tests (optional)

- .github/
  - workflows/                CI pipelines

## Service structure conventions

Each folder in services/* must contain:
- README.md
  - What this component does
  - Interfaces it owns and consumes
  - How to run locally
  - How to test it
- cmd/ or main/ entrypoint
- config/ default config and example config
- internal/ implementation (avoid leaking internal packages)
- pkg/ public packages intended for reuse (keep minimal)

## Interface ownership rules

- Schemas and API contracts live in api/ and are treated as product surface.
- A service may not silently change a contract:
  - Update schema/OpenAPI first
  - Add contract tests
  - Add compatibility notes (docs/engineering/compatibility-and-versioning.md)
- Event types are contracts. Producers and consumers must share the same schema definition.

## Where to put new things

- New customer-facing CLI command or output behavior: cli/ghostctl + docs/cli.
- New resource type (org/project/app/env/release/workload/endpoint/volume/secret bundle): api/schemas + services/control-plane + docs/architecture + docs/cli.
- New runtime capability (VM lifecycle, volumes, networking): services/node-agent + libs/* + docs/architecture/runtime.
- New ingress capability (IPv4 add-on, proxy protocol v2, L4 mapping): services/ingress + api/schemas + docs/architecture/networking.
- New frontend UX for terminal/console: frontend/* + docs/frontend + docs/product.

## Naming and IDs

- Resource names are user-controlled, but resource IDs are stable and system-generated.
- Any persisted identifier must have:
  - Canonical string form
  - Strict parser + validator
  - Tests for roundtrip encoding

## Document map for contributors

Start here:
- docs/engineering/dev-environment.md
- docs/engineering/build-and-release.md
- docs/engineering/coding-standards.md
- docs/engineering/testing-strategy.md
- docs/engineering/compatibility-and-versioning.md
