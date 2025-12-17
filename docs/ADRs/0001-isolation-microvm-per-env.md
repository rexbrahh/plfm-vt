Here’s the consolidated project reality I’m carrying forward, in plain terms, so the ADRs stay coherent:

* **What this is:** a developer focused PaaS. Users deploy **OCI images** plus a **small manifest**. A **CLI** is the primary product surface. A web terminal frontend can exist, but it is not the control plane.
* **How it works at a high level:** a **control plane** (Postgres, event log, materialized views) accepts deploy intents, schedules workloads onto hosts, and publishes routing and runtime config. Hosts run a **node agent** that boots **Firecracker microVMs**. Hosts are connected by a **WireGuard overlay**. Traffic enters via **L4 ingress** (SNI passthrough first). **IPv6-first**, optional **paid IPv4**.
* **Why it exists:** give individual developers and small teams a “boring reliable” way to run services with clean ergonomics, strong isolation, and predictable networking, without inheriting the entire Kubernetes complexity stack.
* **What it is NOT:** not Kubernetes, not a generic VM hosting panel, not a serverless runtime, not an L7-first product, not a multi writer shared storage system by default, not a platform that assumes IPv4 everywhere.

Now I’ll do **ADR 0001 only**.

---

# docs/adr/0001-isolation-microvm-per-env.md

## Title

Isolation boundary is a dedicated microVM per application environment

## Status

Locked

## Context

We are building a PaaS that runs untrusted or semi trusted user workloads. We need a clear isolation boundary that:

* Reduces blast radius between tenants
* Makes network and secrets handling unambiguous
* Keeps the runtime model consistent across languages and frameworks
* Scales across multiple physical hosts without redesigning isolation later

We have already chosen Firecracker as the microVM technology in a separate ADR. This ADR defines the isolation boundary, not the VM implementation.

## Definitions

* **App**: a named service owned by a tenant (org).
* **Environment**: a deployable instance of an app, for example `prod`, `staging`, `dev`. It has its own config, secrets, routes, and releases.
* **Release**: an immutable deploy artifact reference (OCI digest plus manifest).
* **Workload instance**: one running replica of a release.
* **MicroVM**: a Firecracker VM that contains the workload runtime boundary.

## Decision

1. **Each workload instance runs inside its own dedicated microVM.**
   No microVM runs workload instances from two different environments.

2. **An environment may scale to multiple microVMs**, but each microVM is still exclusive to that environment.

3. **The microVM boundary is the tenant safety boundary** for CPU, memory, filesystem, process namespace, and kernel attack surface. We do not rely on “containers only” isolation for multi tenant safety in v1.

4. **Environment scoped configuration is applied at the microVM boundary**, including:

* environment variables
* startup scripts
* dependency injection mechanisms we control (for example mounted secrets file)
* network identity (allocated addresses and ports)
* volume mounts scoped to that environment

## What this enables

* Stronger multi tenancy with a simpler mental model: “your env runs in its own VM(s)”
* Clear mapping for secrets: secrets are injected into the microVM for that env only
* Clear mapping for networking: per env IP allocation and ingress routing do not need per process filtering
* Predictable failure domains: a compromised workload instance is contained to its microVM

## What this explicitly does NOT mean

* We are not promising one physical host per environment.
* We are not preventing multiple microVMs from different tenants sharing the same host. The host is shared, the microVM is the boundary.
* We are not building a “multi process per VM platform” where unrelated workloads share a microVM for density.
* We are not implementing “per request microVMs” (serverless style). MicroVMs are for long lived service instances.

## Rationale

* Containers alone are not a sufficient comfort level for running arbitrary tenant code on shared metal, especially early in the product when hardening is incomplete.
* A microVM boundary makes security, routing, and debugging rules easier to reason about and document.
* The performance and density cost is acceptable given the product intent and the ability to oversubscribe CPU while hard capping memory (separate ADR).

## Consequences

### Positive

* Simpler security story and simpler customer story
* Cleaner separation for env scoped config and secrets
* Makes later features easier: snapshots, migration, per env networking policies

### Negative

* Higher overhead than container only isolation (boot time, memory overhead, image prep complexity)
* Requires a node agent and host level orchestration from day one
* Forces us to define an image to microVM rootfs strategy early

## Alternatives considered

1. **Containers only** (namespaces + cgroups + seccomp)
   Rejected for v1 multi tenant confidence.

2. **One microVM per tenant org, multiple envs inside**
   Rejected because envs should be isolation boundaries. Mixing staging and prod inside the same VM creates avoidable blast radius and complicates secrets and routing.

3. **One microVM per host, containerize inside**
   Rejected because it collapses isolation to the host boundary and recreates container multi tenancy issues.

## Invariants to enforce

* A microVM may only be associated with exactly one `(org, app, env, release)` at a time.
* Secrets for `(org, app, env)` must never be mounted into a microVM for a different env.
* Network identity (addresses, ports, routes) must be derived from env level allocation, not from workload self assertion.

## Non-goals

* Live migration between hosts in v1
* Multi region semantics in v1 (this ADR is compatible with it later)
* Cross env shared volumes by default

## Open questions

* Do we allow multiple process types in the same env (web, worker) inside the same microVM, or require separate microVMs per process type?
  My recommendation: separate microVMs per process type unless you have a strong reason, because it keeps scheduling and scaling clean.

If this matches your intent, we proceed to **ADR 0002** next. If you want to tweak the definition of “environment” or “process type” before moving on, say so now, because it affects several later ADRs.
