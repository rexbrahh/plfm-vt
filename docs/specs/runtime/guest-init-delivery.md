# Guest Init Delivery (v1)

Status: approved  
Owner: Runtime Team  
Last reviewed: 2025-12-17

## Purpose

Define how the guest init binary is packaged, delivered, and updated.

This spec addresses the "how does init get into the VM" question that was previously underspecified.

## Decision (v1)

**Guest init is delivered via a platform-managed initramfs that is paired with a platform-managed kernel.**

We do NOT mutate customer OCI images to install guest init.

## Rationale

Alternatives considered:

1. **Embed in customer image**: Rejected. Mutating customer images is fragile, creates upgrade complexity, and increases attack surface.

2. **Separate boot disk**: Rejected. Adds complexity to block device management and boot configuration.

3. **Kernel-embedded init**: Rejected. Too inflexible for iteration and debugging.

4. **Initramfs (chosen)**: Clean separation, fast iteration, standard Linux mechanism, minimal boot overhead.

## Artifact Naming

Kernel image:
```
vmlinuz-plfm-<kernel_version>-<build_id>
```

Initramfs image:
```
initramfs-plfm-guestinit-<guest_init_version>-<build_id>.img
```

Examples:
- `vmlinuz-plfm-6.1.50-20251217a`
- `initramfs-plfm-guestinit-1.0.0-20251217a.img`

Both artifacts are content-addressed in the host image cache and referenced by stable version labels.

## Artifact Storage

Artifacts are stored in:
- Host local cache: `/var/lib/plfm-agent/kernels/`
- Platform artifact registry (for distribution)

Cache structure:
```
/var/lib/plfm-agent/kernels/
  vmlinuz-plfm-6.1.50-20251217a
  vmlinuz-plfm-6.1.50-20251217a.sha256
  initramfs-plfm-guestinit-1.0.0-20251217a.img
  initramfs-plfm-guestinit-1.0.0-20251217a.img.sha256
  current -> vmlinuz-plfm-6.1.50-20251217a
  current-initramfs -> initramfs-plfm-guestinit-1.0.0-20251217a.img
```

## Initramfs Contents (v1)

The initramfs contains:

| Component | Purpose | Size Target |
|-----------|---------|-------------|
| guest-init binary | PID 1, boot orchestration | < 5 MB |
| busybox (optional) | Diagnostics, rescue shell | < 2 MB |
| /dev setup scripts | Device node creation | < 100 KB |
| CA bundle | mTLS (if needed) | < 500 KB |

Total target size: **< 20 MiB compressed**

### What is NOT in initramfs

- Customer application code
- Customer image layers
- Full glibc (use musl or static linking)
- Systemd or other init systems
- Package managers

## Build Process

### Build Inputs

1. Guest init source code (Rust or Go, static binary)
2. Kernel config and source
3. Busybox config (minimal applets)
4. Build metadata (version, timestamp, git SHA)

### Build Outputs

1. Kernel image (vmlinuz)
2. Initramfs image (cpio archive, gzip compressed)
3. Manifest file (JSON with versions, checksums, compatibility)

### Build Requirements

- Reproducible: same inputs produce identical outputs
- Signed: artifacts signed with platform signing key
- Verified: checksums validated before use

### Manifest File

```json
{
  "kernel": {
    "version": "6.1.50",
    "build_id": "20251217a",
    "sha256": "abc123...",
    "config_sha256": "def456..."
  },
  "initramfs": {
    "guest_init_version": "1.0.0",
    "guest_init_protocol": 1,
    "build_id": "20251217a",
    "sha256": "789abc...",
    "size_bytes": 15000000
  },
  "compatibility": {
    "min_agent_version": "1.0.0",
    "supported_protocols": [1]
  },
  "signature": "base64-encoded-signature"
}
```

## Boot Sequence

1. Firecracker loads kernel and initramfs.
2. Kernel boots, unpacks initramfs to rootfs.
3. Kernel runs `/init` (guest init binary).
4. Guest init:
   - Mounts /proc, /sys, /dev
   - Performs vsock config handshake
   - Mounts root disk (customer image) as overlay lowerdir
   - Mounts scratch disk for overlay upperdir
   - Pivots root to overlay
   - Configures networking, volumes, secrets
   - Execs customer workload

## Update and Rollback

### Update Flow

1. Control plane determines target kernel/initramfs version for cluster.
2. Host agents download new artifacts during maintenance window or rolling update.
3. Artifacts are verified (signature, checksum).
4. New instances use new version; existing instances continue on current version.
5. Rollback: control plane can specify previous version.

### Version Selection

Host agent advertises supported versions to control plane:
```json
{
  "kernel_versions": ["6.1.50-20251217a", "6.1.45-20251201a"],
  "guest_init_protocols": [1],
  "current_default": {
    "kernel": "6.1.50-20251217a",
    "initramfs": "guestinit-1.0.0-20251217a"
  }
}
```

Control plane selects version per instance or uses cluster default.

### Rollback Triggers

Automatic rollback if:
- Boot failure rate exceeds threshold (e.g., 10% of new instances fail)
- Guest init handshake timeout rate spikes

Manual rollback via operator command.

### Compatibility Matrix

| Guest Init Protocol | Min Agent Version | Max Agent Version |
|---------------------|-------------------|-------------------|
| 1                   | 1.0.0             | current           |

Agent MUST NOT run guest init with incompatible protocol.

## Acceptance Criteria

- Guest init + initramfs size: **<= 20 MiB compressed**
- Cold boot overhead attributable to initramfs: **<= 50 ms P99** on target hardware
- Artifact download time: **<= 5 seconds** on 100 Mbps link
- Signature verification time: **<= 100 ms**

## Security Requirements

- Artifacts MUST be signed with platform signing key.
- Agents MUST verify signatures before use.
- Agents MUST verify checksums after download.
- Old artifacts MUST be retained for rollback (retention policy: last 3 versions).
- Build process MUST be reproducible and auditable.

## Operational Requirements

- Operator can list available versions: `plfm admin kernels list`
- Operator can set cluster default: `plfm admin kernels set-default`
- Operator can force rollback: `plfm admin kernels rollback`
- Metrics: version distribution across fleet, boot success rate by version

## Open Questions (v2)

- Customer-provided kernel configs (unlikely, but asked)
- ARM64 kernel/initramfs variants
- Debug initramfs with additional tooling
- Automated canary rollout of new versions
