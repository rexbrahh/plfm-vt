# docs/ADRs/0002-artifact-oci-image-plus-manifest.md

## Title

Workload artifact is an OCI image plus a platform manifest

## Status

Locked

## Context

We need a workload artifact format that:

* Is standard, debuggable, and toolable with existing ecosystem tooling
* Works for any language and framework
* Can be pulled, cached, verified, and rolled back reliably
* Maps cleanly onto our runtime boundary (microVM per environment, Firecracker)
* Avoids “platform specific packaging” and avoids re-inventing a build system in v1

We also need a place to declare runtime configuration that should not live inside the container image, such as ports, resource requirements, health checks, secrets mounts, and volumes.

## Decision

1. **A Release is defined by two inputs:**

* **OCI image reference pinned by digest** (required for immutability)
* **Platform manifest** (versioned config file that describes how to run the image)

2. **OCI images are pushed to and pulled from a standard OCI registry.**
   We support standard registry auth and pull semantics.

3. **The platform manifest is the authoritative runtime configuration.**
   It declares at minimum:

* Exposed ports and protocol intent (L4 routing inputs)
* Process entrypoint overrides (if any) and environment variables
* Resource requirements and limits (cpu request, memory hard cap, disk needs)
* Health checks and rollout semantics
* Volume mount intent and attachment rules (if volumes exist)
* Secrets injection intent (what secret bundle, where it is mounted, permissions)
* Networking intent that the platform controls (public exposure, internal only)

4. **We do not encode platform runtime policy into the image.**
   The image may contain application code and its dependencies. The platform manifest contains deployment policy and platform level configuration.

5. **The platform manifest must have an explicit schema version** and must be validated by the CLI and control plane. Defaults are applied in a deterministic way.

## Definitions

* **OCI image**: an image compatible with OCI image spec and registry distribution.
* **Manifest**: a small platform owned config file, likely `<platform>.toml` or similar, that is validated against `docs/specs/manifest/manifest-schema.md`.
* **Release**: an immutable tuple `(image_digest, manifest_content_hash)` plus metadata, stored in the control plane event log.

## What this explicitly enables

* Any user can build with existing tooling (Docker, BuildKit, nix, Bazel, anything that emits OCI)
* Deterministic rollbacks by digest, not by tag
* Effective caching at the host agent (content addressed)
* Clean separation between app build concerns and platform runtime concerns
* A stable contract between CLI, scheduler, and host agent

## What this explicitly does NOT mean

* We are not shipping buildpacks in v1.
* We are not accepting zip uploads, tarballs, or arbitrary filesystem bundles as a first class artifact.
* We are not supporting docker-compose, helm charts, or “multi service” bundles in v1.
* We are not supporting “mutable tag deploys” as the source of truth. Tags may exist for convenience, but a Release always pins a digest.
* We are not letting the image self-declare platform policy that overrides the manifest.

## Rationale

* OCI is the most interoperable artifact format that already has mature tooling, registries, caching, and introspection.
* The manifest is necessary to avoid baking platform coupling into images and to keep configuration reviewable, diffable, and auditable.
* Digest pinning is required to make release immutability and audit logs meaningful.

## Consequences

### Positive

* Lowest friction for users and for us
* Avoids inventing a custom packaging ecosystem
* Clear rollback and audit semantics
* Allows gradual introduction of signing and policy enforcement later

### Negative

* We must define an OCI to microVM root filesystem strategy early
* Users must run a build step that emits OCI images
* Manifest schema evolution must be managed carefully

## Alternatives considered

1. **Single custom artifact that bundles image plus config**
   Rejected because it recreates registry, tooling, and distribution problems.

2. **Git based deploy only**
   Rejected because it forces us to own builds and language tooling, and it is not compatible with “OCI only v1”.

3. **Tag based deploys**
   Rejected because tags are mutable and break reproducibility and auditability.

## Invariants to enforce

* A Release must include an **image digest**, not only a tag.
* Manifest validation must be deterministic and identical in CLI and control plane.
* The scheduler and host agent must operate only on the Release tuple, not on mutable references.
* The manifest cannot request privileged host access or host filesystem mounts in v1.

## Non-goals

* Building images on the platform in v1
* Supporting multiple artifact formats in v1
* Automatic inference of ports, health checks, or resources from the image
* Supply chain policy enforcement as a hard requirement in v1 (signing can be added later)

## Open questions

* **Manifest format:** TOML vs YAML vs JSON. Recommendation: TOML for human editing and stable typing, but we should choose based on your CLI ergonomics.
* **Multi-arch support in v1:** do we require users to push single-arch images or do we support OCI indexes and select per host arch? My recommendation: allow OCI indexes but require digest pinning to the index digest.

Proceed to **ADR 0003** when you’re ready.
