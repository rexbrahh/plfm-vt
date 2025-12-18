# docs/ADRs/0003-runtime-firecracker.md

## Title

Runtime for workload isolation is Firecracker microVMs

## Status

Locked

## Context

We need a runtime that can execute user workloads with strong isolation on shared bare metal, while keeping operational complexity and resource overhead reasonable.

Constraints already locked elsewhere:

* Isolation boundary is a dedicated microVM per environment.
* Artifact is OCI image plus platform manifest.
* IPv6-first networking, WireGuard overlay, L4 ingress first.

This ADR chooses the concrete microVM technology and the operational posture around it.

## Decision

1. **Firecracker is the microVM runtime** used to run all workload instances in v1.

2. **The host agent owns the Firecracker lifecycle**, including:

* creating and configuring microVMs
* attaching root filesystem and optional volumes
* enforcing CPU and memory limits at the host boundary
* wiring networking (tap, routing, overlay attachment)
* collecting logs and health
* terminating and garbage collecting microVMs

3. **Each microVM boots a Linux kernel** and runs a minimal init that launches the workload entrypoint derived from the Release (OCI digest plus manifest).

4. **Host hardening is part of the runtime contract**, not optional:

* Firecracker runs jailed (jailer or equivalent containment)
* seccomp policy is applied for the VMM process
* cgroups enforce resource constraints
* filesystem access is restricted to per microVM directories and attached block devices

5. **Control channels between host and guest use explicit mechanisms** (vsock is the default intent), not ad hoc host filesystem mounts or privileged host networking.

## Rationale

* Firecracker is purpose built for multi tenant microVM workloads and has a widely adopted operational model.
* It keeps the isolation boundary crisp and consistent with our product positioning.
* It avoids the complexity of full VM stacks while being materially stronger than containers only isolation for early platform maturity.

## Consequences

### Positive

* Strong isolation boundary with a clean story for secrets and networking
* Clear operational ownership: host agent is the single point of control for workload lifecycle
* Enables future features without redesign: snapshots, migration experiments, hardened templates

### Negative

* Higher baseline overhead than containers only (boot time, memory overhead, rootfs prep)
* We must own kernel selection, rootfs strategy, and guest init behavior
* More moving parts for debugging early (VMM, guest init, networking inside VM)

## Alternatives considered

1. **Containers only (namespaces + cgroups + seccomp)**
   Rejected for v1 multi tenant confidence and blast radius reasons.

2. **QEMU/KVM full VM stack**
   Rejected due to operational complexity and heavier overhead for the intended product.

3. **gVisor / Kata only**
   Rejected because the isolation and operational model is either weaker than microVM or still converges to microVM style complexity, but with less direct control.

## Invariants to enforce

* A workload instance cannot execute outside Firecracker in v1.
* A microVM cannot be configured with host privileged devices or arbitrary host mounts.
* MicroVM runtime config must be derived from the Release and validated specs, not from guest self assertion.
* Guest networking must not bypass platform controlled routing and policy.

## What this explicitly does NOT mean

* We are not offering general purpose VMs where users SSH into arbitrary images with full control of the guest OS.
* We are not running nested virtualization or user supplied kernels in v1.
* We are not allowing privileged containers inside the guest as a supported feature.
* We are not committing to live migration in v1.

## Open questions

* Root filesystem strategy: ext4 block image per release, overlay approach, or initramfs based layouts.
* Guest init: custom tiny init vs standard init, and how we model process types.
* How much we standardize the guest environment vs letting images define more (within manifest constraints).

Proceed to **ADR 0004** when ready.
