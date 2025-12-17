# Surface area v1

This document defines what ships in v1, what is explicitly deferred, and how those decisions show up in UX.

The goal is to keep the v1 promise small but complete: deploy real services, expose L4 endpoints, operate and debug, and pay for what you use.

## v1 includes

### Core resources
- Orgs and projects
- Apps and environments (prod, staging, dev)
- Releases (immutable)
- Workloads and instances
- Endpoints (L4)
- Volumes
- Secret bundles (delivered as a fixed file format)
- Events and log streams

### Runtime and scheduling
- MicroVM-based isolation per instance (implementation detail, but it shapes reliability)
- Reconciliation loop that converges desired state toward current state
- Health checks and restart behavior with event visibility
- Basic placement rules (enough to support volumes and quotas)

### Deploy and operations
- Deploy from OCI images
- Rollback by selecting a prior release
- Exec into running instances
- Tail logs and events
- Describe current vs desired state

### Networking
- L4 ingress with explicit endpoints
- IPv6 addresses by default for endpoints
- Dedicated IPv4 as an explicit paid add-on for endpoints that need IPv4 reachability
- Optional Proxy Protocol v2 for L4 metadata needs

### UX surfaces
- CLI as the primary interface
- Web console that is a terminal, not a form-based dashboard (libghostty-vt embedded via WASM)

## v1 explicitly does not include

These are intentionally deferred to keep v1 focused and shippable.

### L7 platform features
- Hostname-based HTTP routing (shared 443 across many apps)
- Managed TLS termination at the platform edge
- HTTP middleware features (rewrites, WAF integration, header manipulation)

Users can still run HTTP and TLS in their own app over an L4 endpoint.

### Autoscaling and advanced orchestration
- request-based autoscaling
- scheduled scaling
- complex deployment strategies (canary, blue-green as a product feature)

v1 focuses on deterministic deploys and manual scaling.

### Managed data services
- managed Postgres, Redis, queues
- one-click add-ons marketplace

Users can bring their own services or self-host inside the platform.

### Multi-region and global scheduling
- automatic multi-region placement
- cross-region failover orchestration
- global anycast guarantees (beyond what L4 ingress provides)

v1 can start in a single region or a small number of regions with explicit user choice.

### Enterprise features
- advanced RBAC and policy frameworks
- SSO integrations beyond basic login
- audit exports and compliance tooling

## UX implications

### “Explicitly deferred” is part of the product
The CLI and docs should make it obvious when something is not supported.

Examples:
- If a user tries to configure HTTP host routing, the system should explain: not in v1, and what to do instead.
- If a user expects shared 443, the system should suggest: allocate a dedicated IPv6 endpoint (and IPv4 add-on if needed), or wait for L7 features.

### Everything is inspectable
For every resource type in v1, the UX must support:
- list
- describe
- events
- logs where relevant
- machine-readable output

If we ship a resource without introspection, it will become a support burden.

### Eventual consistency is visible
Because the system reconciles, state transitions take time. UX must support:
- receipts on mutation
- `wait` and `events` as first-class
- clear indication of desired vs current
