# docs/adr/0012-scheduling-cpu-oversubscribe-mem-hardcap.md

## Title

Scheduling oversubscribes CPU but hard-caps memory

## Status

Locked

## Context

We need a scheduling and resource model that:

* provides predictable safety boundaries on shared hosts
* maximizes utilization without compromising host stability
* is simple to communicate to users and to enforce reliably
* fits microVM isolation and bare metal economics

CPU is a time-shared resource and can be multiplexed with acceptable variability for many workloads. Memory exhaustion, however, can destabilize the host and cause cascading failures.

## Decision

1. **CPU is treated as a soft resource and may be oversubscribed.**

* users declare a CPU request or share target
* the scheduler can place more total requested CPU on a host than its physical core count
* the runtime enforces fairness via cgroups quotas/shares rather than strict reservation

2. **Memory is treated as a hard resource and is not oversubscribed.**

* each workload declares a memory limit (hard cap)
* the scheduler ensures the sum of memory hard caps on a host does not exceed a safe threshold
* the runtime enforces memory limits strictly; exceeding memory results in workload termination or OOM inside the microVM, not host instability

3. **The scheduler uses memory as the primary placement constraint.**

* a workload cannot be scheduled if its memory hard cap cannot be satisfied
* memory headroom is reserved for host services and platform overhead

4. **Platform overhead is explicitly budgeted.**

* Firecracker overhead, page cache behavior, and host agent memory are accounted for
* we maintain per-host “allocatable memory” separate from “physical memory”

## Rationale

* CPU oversubscription improves density and cost efficiency without breaking safety as long as fairness is enforced.
* Memory oversubscription is a common cause of host instability and cascading failures; hard caps are the simplest reliable protection.
* This model is widely understood and easy to message: “CPU may burst, memory is strict.”

## Consequences

### Positive

* Better host utilization and better economics
* Predictable failure behavior: memory limits are enforceable and local
* Easier scheduling logic in early versions

### Negative

* CPU-heavy workloads may experience contention under high density
* Some users will mis-size memory and get OOMs until they learn
* We must implement clear observability and error messages around memory exhaustion and CPU throttling

## Alternatives considered

1. **No oversubscription (strict CPU and memory reservation)**
   Rejected because it wastes capacity and reduces economic viability on bare metal.

2. **Oversubscribe both CPU and memory**
   Rejected due to high risk of host instability and noisy-neighbor amplification.

3. **Strict CPU pinning**
   Rejected for v1 because it reduces flexibility and increases scheduling complexity without being necessary for the target user base.

## Invariants to enforce

* Scheduler must never place workloads such that the sum of hard memory caps exceeds allocatable memory.
* Runtime must enforce per-microVM memory limits reliably.
* CPU fairness must be enforced (shares/quotas) so one tenant cannot starve others.
* Host reserved memory must be configurable and versioned as part of node configuration.
* Observability must clearly surface: CPU throttling, memory usage vs cap, and OOM events.

## What this explicitly does NOT mean

* We are not guaranteeing CPU performance isolation equivalent to dedicated cores in v1.
* We are not promising real-time scheduling or low jitter guarantees.
* We are not allowing workloads to request “unlimited memory”.
* We are not relying on host swap as a safety mechanism.

## Open questions

* How we expose CPU requests: cores, millicores, shares, or a simpler tier model.
* Whether we allow memory “request” vs “limit” or only a single hard limit in v1.
* Whether we implement per-tenant fairness at scheduling time in addition to runtime quotas.

---

That completes ADR 0001 through 0012 in explicit statement form. If you want, the next step is to add the standard ADR header boilerplate you prefer (date, owner, reviewers) and then we move on to the non-ADR specs that these decisions imply (manifest schema, event types, routing objects, secrets file syntax).
