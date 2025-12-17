# ADR-013: Target NixOS as the Bare Metal Host OS

**Status:** Proposed
**Date:** 2025-12-17
**Owners:** Platform, Runtime, SRE

## Context

libghostty-vt PaaS runs customer workloads in **microVMs** (Firecracker on KVM) and relies on host-level primitives for networking, isolation, storage, logging, and reconciliation-driven operations.

So far, the docs describe a **Linux host** implicitly, without committing to a specific host distribution. This leaves several “hidden contracts” underspecified:

* How nodes are provisioned and upgraded safely
* How host services are composed and rolled back
* How kernel, nftables, WireGuard, cgroup v2, and systemd settings are kept consistent across the fleet
* How to prevent configuration drift across nodes

We want v1 to be predictable, reproducible, and operator-friendly with strong rollback semantics. The team already uses Nix heavily.

## Decision

We will standardize v1 on:

* **Bare metal host OS:** **NixOS**
* **Hypervisor stack:** Linux KVM with Firecracker as the VMM on the NixOS host
* **Guest OS:** not required to be NixOS. Guest images may be NixOS or other Linux distros, as long as they satisfy the guest contract.

NixOS is the only supported host OS for v1 production clusters. “Linux host” language in docs becomes “NixOS host” where it matters, and generic where it does not.

## Rationale

### Why NixOS for the host

* **Reproducible node state:** pinned inputs reduce configuration drift across a fleet.
* **Atomic upgrades with fast rollback:** `nixos-rebuild switch` / rollback fits the operational model for a small team running critical infra.
* **Declarative, reviewable ops:** host configuration becomes code, enabling safe changes and easier incident audits.
* **Better alignment with reconciliation:** our control plane already thinks in desired vs current state; NixOS makes host desired state explicit and enforceable.

### Costs and risks we accept

* **Hiring and familiarity:** fewer operators have deep NixOS experience than Ubuntu or Debian.
* **Supply chain surface:** binary caches and substituters introduce trust management requirements.
* **Footguns:** secrets must never enter the Nix store; careless module design can leak sensitive data.
* **Operational sharp edges:** Nix store growth, GC behavior, and build failures can impact node lifecycle if not managed.

We accept these risks because the team is already Nix-native, and the benefits materially reduce fleet drift and “snowflake node” incidents in v1.

## Non-goals

* Supporting multiple host OS distributions in v1.
* Requiring customers to use NixOS inside microVMs.
* Using Nix to build or store any customer secret material.

## Host contract

NixOS hosts must provide, at minimum:

* **Kernel and virtualization:** KVM enabled, appropriate CPU virtualization flags, stable kernel baseline
* **Resource control:** cgroup v2 enabled and used for per-microVM accounting and limits
* **Networking:** nftables, IPv6-first posture, WireGuard overlay, bridge and tap orchestration for microVMs
* **Storage:** stable mount points for image cache, volume backends, logs, and host state
* **Service manager:** systemd is the control surface for host daemons
* **Observability:** journald is authoritative for host services, exporters and shippers are standardized

## Security requirements specific to NixOS

### Absolute rule: no secrets in the Nix store

* No secrets in Nix expressions, module options that render into derivations, or any build inputs.
* Any secret delivery must land in runtime paths such as `/run`, dedicated encrypted host paths, or mounted volumes with strict permissions.
* CI must block merges that introduce secret patterns into Nix files.

### Binary cache trust model

* We must explicitly define trusted substituters and signing keys.
* Key rotation and compromise procedures must be documented.
* Production nodes must not accept arbitrary public caches by default.

### Hardening and sandboxing

* Host services should use systemd hardening (capabilities, filesystem protections, seccomp where applicable).
* Firewall policy is declarative and versioned.

## Operational model

### Provisioning

* Nodes are provisioned from a pinned NixOS configuration (flake or pinned nixpkgs) with an explicit “node role” module.
* Provisioning is repeatable and produces identical host state for the same commit.

### Upgrades

* Upgrades are atomic, with canary-first rollout.
* Rollback is a first-class incident response tool and is documented as such.

### Garbage collection

* Nix store GC policy is explicit, not ad hoc.
* Disk pressure behavior is defined to protect microVM runtime paths and caches.

## Consequences

### Positive

* Strong reduction in configuration drift and “works on one node” behavior
* Faster and safer node upgrades
* Clear host service composition and ownership
* Better auditability and repeatable environments for debugging

### Negative

* Narrower hiring pool for on-call SRE experience
* More up-front work to define secure Nix patterns and CI safeguards
* Potential for build or cache issues to block node changes if processes are sloppy

## Alternatives considered

### Ubuntu or Debian hosts with configuration management

Pros: broad familiarity, easier hiring
Cons: drift is common, rollback is harder, host state is less reproducible without significant tooling

### “Any Linux host” as a goal

Pros: flexibility
Cons: pushes real decisions into undefined operational behavior, increases long-term incident risk in v1

We are choosing the option that best matches the team and reduces fleet risk early.

## Documentation changes required

This ADR requires both “big” doc updates and “small” wording and assumption edits.

### Big changes

1. **Add a Host OS chapter**

* New doc: `docs/runtime/host-os.md` or `docs/ops/host-os.md`
* Contents: host contract, service topology, filesystem layout, kernel requirements, upgrade and rollback procedure, GC policy, and security constraints around Nix.

2. **Add a Node lifecycle chapter**

* New doc: `docs/ops/node-lifecycle.md`
* Contents: provisioning, enrollment, draining, eviction, upgrades, rollback, decommission.

3. **Add a Supply chain chapter for host builds**

* New doc: `docs/security/supply-chain.md`
* Contents: substituters, signing keys, pinning strategy, cache policy, incident playbooks.

4. **Update the DevOps module to be NixOS-first**

* Replace generic “set up Linux host” guidance with a NixOS bootstrap and deployment workflow.

### Small changes across existing docs

* **Runtime docs**

  * Replace “Linux host” assumptions with “NixOS host” where host behavior is required.
  * Define canonical paths for caches, logs, images, and host state.

* **Networking docs**

  * State nftables as the canonical firewall implementation on hosts.
  * Document WireGuard interface naming and persistence as declarative config.
  * Explicitly state IPv6 default posture at the host edge.

* **Scheduler and reconciliation docs**

  * Clarify which actions are “host reconciliation” vs “guest reconciliation”.
  * Specify systemd units and dependencies for the host agents.

* **Secrets docs**

  * Add a hard callout that secrets must not enter the Nix store.
  * Document runtime-only secret rendering paths and permission model.

* **Observability docs**

  * Clarify journald as the authoritative host log source.
  * Document standard exporters and service units.

* **Security docs**

  * Add NixOS-specific threat considerations: cache poisoning, key compromise, store leakage risks.
  * Add CI checks for secret patterns in Nix.

## Implementation plan

1. Land this ADR.
2. Add `host-os.md` with the host contract and required invariants.
3. Update ops docs to define provisioning, upgrades, rollback, and GC policy.
4. Update security docs to codify cache trust and “no secrets in store” enforcement.
5. Sweep existing docs for “Linux host” language and replace with precise host contract references.
6. Add CI guardrails:

   * block secrets in Nix sources
   * enforce pinned inputs
   * lint for forbidden Nix patterns that embed runtime secrets

## Open questions to resolve in follow-up ADRs

* Pinned nixpkgs strategy and update cadence
* Binary cache approach: self-hosted vs trusted third-party, key management, and rotation
* Kernel baseline choice and upgrade policy
* Exact host filesystem layout for images, volumes, and logs

If you want, I can also draft the companion `docs/runtime/host-os.md` so the ADR does not stay abstract.
