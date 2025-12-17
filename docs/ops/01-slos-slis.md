# SLOs and SLIs

This document defines what "reliable" means in measurable terms. Targets start conservative and tighten as we mature.

## Principles

- SLOs are defined from the customer perspective.
- We measure outcomes, not intentions.
- We avoid "100%" targets. Error budgets are necessary to ship.

## Service taxonomy

- **Control plane**: API, auth, scheduler, reconciler, secrets delivery, metadata store (Postgres)
- **Edge**: L4 ingress, IPv6 by default, optional dedicated IPv4 add-on, optional Proxy Protocol v2
- **Data plane**: hosts + agent, Firecracker supervisor, WireGuard overlay, storage
- **Customer surfaces**: CLI, console, events/log streams, deployment experience

## Default measurement windows

- **SLO window**: 30 days rolling
- **Reporting**: weekly review, monthly formal report
- **Alerting**: multi-window burn rate (fast and slow)

## Candidate SLOs (v1)

These are reasonable starting points for an early PaaS. Adjust after we have real traffic patterns.

### Control plane API

| SLO | Target | SLI definition | Notes |
|---|---:|---|---|
| Availability | 99.9% | `good = 2xx/3xx`, `bad = 5xx + timeouts` on public API | Exclude planned maintenance windows with prior notice |
| Read latency | 95% < 250 ms | p95 latency of read endpoints (GET) | Region scoped |
| Write latency | 95% < 500 ms | p95 latency of mutation endpoints (POST/PUT/DELETE) | Region scoped |

### Release and reconciliation

| SLO | Target | SLI definition | Notes |
|---|---:|---|---|
| Release create success | 99.5% | `good = release created and accepted`, `bad = failure` | Separate validation failures from infra failures |
| Time to converge | 99% < 2 min | from desired state commit to all targeted instances reporting "ready" | Measures scheduler + agent + image + secrets |
| Deployment stuck rate | < 0.5% | fraction of releases requiring manual intervention | Manual includes "force", "repair", "recreate" |

### Edge connectivity (L4)

| SLO | Target | SLI definition | Notes |
|---|---:|---|---|
| TCP connect success | 99.95% | `good = successful connect within 3s`, `bad = timeout/reset/refused` | Per region, per endpoint |
| TLS passthrough success | 99.95% | `good = end-to-end handshake`, `bad = handshake failure` | Only for customers using TLS passthrough |
| Median connect latency | < 150 ms | client to edge connect p50 | Use synthetic probes from multiple networks |

### Logs and events

| SLO | Target | SLI definition | Notes |
|---|---:|---|---|
| Event stream availability | 99.9% | successful subscription and delivery (no gaps > 60s) | If the customer is connected |
| Log delivery freshness | 99% < 10 s | ingestion timestamp to customer stream timestamp | Sampled per workload |

### Storage

| SLO | Target | SLI definition | Notes |
|---|---:|---|---|
| Volume attach success | 99.9% | attach and mount completes | Does not include app misconfig |
| Snapshot success | 99.9% | snapshot job succeeds | Backups are covered separately |

## Error budget policy

- We treat the 30 day error budget as the budget to spend on change.
- If burn rate exceeds threshold, we slow feature rollouts and focus on reliability.

Suggested policy:

- **> 2x burn** (projected to exhaust in < 15 days): stop risky changes for the affected subsystem
- **> 5x burn** (projected to exhaust in < 6 days): freeze deploys, open incident, assign fix owners
- **budget exhausted**: only reliability work and customer-impacting fixes until budget is back in range

## Burn-rate alerting (recommended)

For each SLO, define two alert windows:

- Fast burn: 5m window with 1h projection
- Slow burn: 30m window with 6h projection

Example (availability 99.9%, error budget 0.1%):

- Page if 5m error rate > 14.4x budget rate
- Page if 30m error rate > 6x budget rate

Exact numbers depend on chosen policy and operational tolerance.

## SLI instrumentation requirements

Every service must emit:

- request count, latency histograms, and error count
- saturation signals (CPU, memory, queue depth) as secondary signals
- correlation IDs across API, scheduler, reconciler, agent

Every alert must link to a runbook in `docs/ops/runbooks/`.

## SLO review process

Monthly:

1. Review SLO compliance and top error sources
2. Propose changes to SLOs only with data (do not guess)
3. Review whether alerting caused unnecessary pages
4. Update runbooks based on real incidents
