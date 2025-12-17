# docs/specs/runtime/image-fetch-and-cache.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-16

## Purpose
Define how the host agent:
- pulls OCI images (by digest)
- verifies integrity
- caches artifacts
- converts OCI images into Firecracker bootable root disks
- evicts cached data safely

Locked decision: artifact is OCI image + manifest. Release pins an image digest. See `docs/adr/0002-artifact-oci-image-plus-manifest.md`.  
Locked decision: runtime is Firecracker microVMs. See `docs/adr/0003-runtime-firecracker.md`.  
Boot/rootfs strategy is defined in `docs/specs/runtime/firecracker-boot.md`.

## Scope
This spec defines host-side artifact handling.

This spec does not define:
- registry authentication UI/flows (control plane auth covers tokens; registry auth is separate)
- supply-chain signing enforcement (future; see security docs)
- volume backup storage (storage specs)

## Definitions
- **Image ref**: human-friendly OCI reference, may include a tag.
- **Index digest**: sha256 digest of OCI index manifest (multi-arch).
- **Resolved digest**: sha256 digest of the arch-specific image manifest that will actually run on a node.
- **Layer**: OCI filesystem layer blob.
- **Root disk**: ext4 image file built from unpacked OCI layers.
- **Release cache key**: content-addressed key derived from resolved digest (and optionally init/kernel version if they influence root disk layout).

## Core invariants
1) **Agents run only digest-pinned images.**  
   Tags may be used by users, but must be resolved to digests before execution.

2) **Cache is content-addressed.**  
   Cached artifacts are keyed by digest (and other immutable inputs when necessary).

3) **Root disks are immutable per digest.**  
   Root disk built from a given resolved digest must not change once created.

4) **Cache eviction must never break running instances.**  
   In-use artifacts are pinned and cannot be evicted.

## Image pull behavior

### Required pull input
The WorkloadSpec must provide:
- `image.index_digest` (optional)
- `image.resolved_digest` (required)
- `image.os` (linux)
- `image.arch` (amd64, arm64)

The host agent must:
- pull by resolved_digest
- never pull by tag for execution

### Registry access
Agent can pull from registries using:
- anonymous access (public images), or
- credentials provided by control plane (future), or
- node-local registry credentials (operator-managed)

v1 recommendation:
- start with public images or operator-configured credentials.
- keep tenant-managed private registry auth minimal until product needs it.

### Multi-arch behavior
If the Release was created from an OCI index:
- control plane records the index digest and resolved digest.
- agent uses the resolved digest for its arch.

If resolved digest is missing:
- agent must fail with a clear error. Resolution must occur in control plane for auditability.

## Integrity verification
At minimum:
- Verify the fetched manifest digest matches `resolved_digest`.
- Verify layer blob digests as they are downloaded.

Any mismatch:
- fail instance start with `image_pull_failed`
- do not cache partial corrupt artifacts

## Cache layout (recommended)
Agent maintains a root directory, example:
- `/var/lib/trc-agent/`

Subdirectories:
- `oci/` for raw OCI artifacts (manifests, blobs)
- `unpacked/` for unpacked root filesystem trees (optional, intermediate)
- `rootdisks/` for built ext4 root disks keyed by digest
- `instances/` for per-instance runtime dirs (Firecracker sockets, scratch disks)
- `tmp/` for transient build workspace

Keyed paths:
- `oci/blobs/sha256/<digest>`
- `oci/manifests/sha256/<digest>`
- `rootdisks/sha256/<resolved_digest>.ext4`
- `rootdisks/sha256/<resolved_digest>.meta.json`

## Root disk build pipeline (v1)

### Inputs
- OCI resolved digest
- platform root disk format version (string)
- optional: guest init version (only if baked into root disk, not recommended)
- optional: kernel version (does not affect disk contents)

Cache key:
- `rootdisk_key = sha256(resolved_digest + rootdisk_format_version)`

### Steps (normative)
1) Ensure OCI manifest and layers for resolved digest exist locally (pull if not).
2) Unpack layers into a build directory using standard OCI layer application rules:
   - apply layers in order
   - respect whiteouts
   - preserve file permissions and ownership
3) Create an ext4 filesystem image sized appropriately:
   - v1 approach: compute used bytes + headroom factor (example 1.2x) with a minimum size (example 512Mi)
   - cap size to a reasonable upper bound unless user config demands bigger
4) Populate ext4 image with unpacked filesystem tree.
5) Mark root disk immutable:
   - store a metadata file with:
     - resolved_digest
     - build timestamp
     - size bytes
     - filesystem type
     - rootdisk_format_version
     - checksum of the ext4 image file (optional)
6) Make the ext4 image read-only at attach time (Firecracker drive config).

### Filesystem assumptions
- Root disk is ext4.
- Overlayfs lowerdir is the root disk mounted read-only.

### Determinism
Root disk contents must be deterministic for a given resolved digest and rootdisk_format_version.
- Do not inject timestamps into filesystem.
- Do not add platform-specific files unless they are part of the image itself.

If you need platform files (like guest init), they should not be copied into the root disk in v1.
- v1 recommendation: guest init is provided via a small initramfs or separate mechanism, not by mutating user images.

## Scratch disk (per instance)
- Scratch disk size comes from WorkloadSpec `ephemeral_disk_bytes`.
- Scratch disk is not cached and is deleted when instance is deleted.
- Scratch disk contains:
  - overlay upperdir/workdir
  - any writable filesystem changes of the workload

## Artifact pinning and reference counting
The agent must track which cached artifacts are in use.

Pinned artifacts include:
- root disk for a resolved digest used by any running instance
- OCI blobs needed to rebuild root disk for any desired running instance (optional; can rebuild by pulling again)

Reference tracking (recommended):
- Maintain a small local database mapping:
  - instance_id -> resolved_digest, rootdisk_key
  - rootdisk_key -> refcount

Eviction must not remove artifacts with refcount > 0.

## Eviction policy
### Goals
- avoid disk full incidents
- keep cache warm for recent releases
- never break running workloads

### What can be evicted
- unused root disks (refcount = 0)
- unused OCI blobs/manifests that are not required for pinned root disks (optional)
- leftover unpacked directories and build temp

### Eviction strategy (v1 recommendation)
- LRU by last accessed timestamp for root disks
- Size-based thresholds:
  - target cache max bytes (operator config)
  - high-water mark triggers eviction

### Failure behavior
If disk pressure is high and eviction cannot free enough space:
- agent must:
  - mark node unschedulable (or degraded) via control plane report
  - fail new instance starts with `rootfs_build_failed` or a dedicated `disk_full` reason
  - surface alertable metrics

## Concurrency and locking
The agent must handle concurrent starts for the same resolved digest.

Rules:
- Root disk build is guarded by a per-rootdisk_key lock:
  - first builder builds
  - others wait or reuse existing disk

Partial build artifacts must not be treated as valid.
- Use atomic rename:
  - build to temp path
  - fsync
  - rename to final path

## Registry failures and retries
Retries:
- retry transient network errors with backoff
- do not retry digest mismatch (fail fast)

Timeouts:
- each pull has a bounded timeout and is cancelable when instance desired state changes to stopped.

If registry is unavailable:
- running instances continue
- new instances may fail if root disk not cached

## Observability requirements
Agent metrics:
- image pull latency and error counts
- bytes downloaded
- cache hit/miss rate for root disks
- root disk build time
- cache size and eviction counts
- disk pressure gauges for cache and volume pools

Agent logs:
- include resolved_digest and instance_id for all actions
- never log registry credentials

## Security notes
- Do not execute anything from image during build.
- Treat OCI blobs as untrusted input.
- Validate tar extraction to prevent path traversal.
- Ensure unpack step enforces:
  - no absolute-path writes outside build root
  - correct handling of symlinks

## Open questions (future)
- Optional image signing verification (cosign) and policy enforcement.
- Shared registry mirror and prefetch strategies.
- Whether to support lazy layer fetching (likely not needed in v1).
