# Service tiers and limits

This document defines how we think about quotas, fair use, and tiering. Exact numbers can evolve, but the model should be stable.

The platform is designed around scarce resources:
- CPU and RAM (capacity)
- storage and IOPS (capacity and contention)
- IPv4 addresses (global scarcity)
- control plane throughput (rate limits)

## Principles

- **Predictable limits**: users should know what they can do before they hit a wall.
- **Graceful failure**: hitting a limit should produce a clear error with next steps.
- **Scarcity pricing**: scarce resources (especially IPv4) should be explicit and paid.
- **No hidden throttles**: if we rate-limit, we say so, and we expose current usage.

## Tiers

### Free / trial tier (concept)
Designed for:
- learning
- prototypes
- small demos

Typical constraints:
- limited total vCPU and RAM usage
- limited number of apps/environments
- limited number of volumes and total storage
- limited log retention window
- no dedicated IPv4 add-on by default (or limited trial credits)

The point is a realistic experience without subsidizing abuse.

### Paid tier (concept)
Designed for production workloads.

Increases:
- higher compute and storage quotas
- more apps/environments
- longer log retention and higher streaming limits
- dedicated IPv4 add-on available
- higher support and reliability commitments

## Quotas and rate limits

Quotas are best expressed per org and per project. Suggested quota categories:

### Control plane quotas
- max apps per project
- max environments per app
- max releases retained per environment
- max endpoints per environment
- max volumes per environment

### Runtime quotas
- max total vCPU across all instances
- max total RAM across all instances
- max instance size (CPU, RAM)
- max instances per workload
- max concurrent exec sessions

### Network quotas
- max concurrent connections per endpoint (soft limit, enforced to protect the edge)
- egress bandwidth caps (if needed)
- request rate limits on endpoint configuration mutations

### Observability quotas
- log retention window
- log stream fan-out limits (how many tails at once)
- events retention window

## Fair use policy

Even in paid tiers, we may enforce guardrails for abusive patterns:
- excessive deploy churn
- unbounded log streaming fan-out
- resource thrashing (rapid scale up and down)

Enforcement should be transparent:
- users can see why an action was throttled
- users can see the current limiter status and how to raise limits

## IPv4 add-on policy

IPv4 is scarce. The default posture is:

- Endpoints get **IPv6** by default.
- **IPv4 is opt-in** and billed as an add-on per endpoint (or per allocated address, depending on implementation).

### UX requirements for IPv4
- The user must explicitly request IPv4.
- The system must clearly show which endpoints have IPv4.
- If a user needs standard web reachability on IPv4, the platform should guide them to enable the add-on (and explain cost).

### Abuse prevention
- IPv4 allocations should have org-level quotas.
- Automatic cleanup policies may apply for unused IPv4 allocations (with warnings), especially in free tiers.
