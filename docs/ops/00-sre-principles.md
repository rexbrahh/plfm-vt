# SRE principles

This module defines how we operate the platform. It is not optional. If we ship a feature, we also ship the operational ability to keep it reliable.

## Scope

Applies to:

- Control plane: API, scheduler, reconciler, secrets delivery, image cache, event/log services, Postgres
- Data plane: hosts, host-agent, Firecracker supervisor, networking overlay (WireGuard), storage, edge ingress
- Customer surfaces: CLI, console, endpoint connectivity, app deployments, logs/metrics delivery

## Definitions

- **Desired state**: the configuration stored in the control plane (apps, envs, releases, endpoints, volumes, secrets)
- **Actual state**: what is currently running on hosts and edge
- **Reconciliation**: control plane and agents continuously converge actual toward desired, eventually consistent
- **Runbook**: a written, tested procedure to diagnose and mitigate an alert or incident
- **SLO / SLI**: service level objective / indicator, measured from user impact, not internal causes
- **Error budget**: allowable unreliability implied by an SLO

## Core principles

### 1) Reliability is a product feature

If the customer cannot trust deployments, networking, or recovery, the product is not working. Reliability work ships as first class product work.

### 2) Runbooks are mandatory

- Every page-triggering alert must link to a runbook.
- Every operationally risky system must have at least one failure-mode runbook.
- No runbook, no pager.
- Runbooks are reviewed after every incident that touched them.

### 3) Prefer automation, but write the manual first

Automation is the end state. The path is:

1. Write the runbook
2. Practice it in staging (game day)
3. Automate it (safe, idempotent, with guardrails)
4. Keep the runbook as the fallback

### 4) Symptom based alerting

We page on symptoms (user impact or imminent user impact):

- SLO burn rate
- elevated 5xx / failed deploys
- inability to create or converge releases
- edge connect failures

We do not page on raw CPU, queue depth, or error logs unless they correlate tightly with user impact.

### 5) Idempotency and eventual consistency are not excuses

Reconciliation makes systems robust, but it also hides failure modes. All controllers and agents must:

- expose convergence SLIs (time to converge, backlog, retries)
- surface stuck states with actionable errors
- support safe retries and resume after restart

### 6) Make failure cheap and contained

- Prefer small blast radius: per-host, per-app, per-env isolation
- Default to IPv6 and avoid centralized NAT bottlenecks
- Treat edge and control plane as separable failure domains
- Design for "degrade gracefully" (read-only mode, cached reads, delayed reconciles)

### 7) Change management is part of the system

- Small changes, canaries, quick rollback
- Explicit migration plans for stateful changes
- Feature flags where rollback is not enough
- Every deploy must have a rollback plan and a "stop the bleeding" plan

### 8) Blameless postmortems, accountable follow-up

We do not blame people. We do assign owners and due dates for fixes. "We learned a lesson" is not a fix.

## Reliability definition of done

A feature is not "done" until it has:

- SLIs emitted (metrics and logs) and a dashboard panel
- an SLO impact statement (does it change existing SLOs?)
- alerts with runbook links (or explicit "no pager" rationale)
- clear failure modes and safe fallback behavior
- capacity considerations (CPU, RAM, storage, IPs, connections)
- rollback procedure (and data migration rollback if needed)

## Operational safety rules

- No production data changes without a written plan and rollback
- No manual failovers without explicit split-brain prevention steps
- No "fix forward" during a major incident unless rollback is impossible
- Freeze non-essential deploys during Sev0/Sev1 incidents
- Prefer actions that reduce blast radius first: cordon, drain, rate limit, shed load

## Ownership and escalation

- Every service has a primary owner team and a secondary.
- Ownership includes: dashboards, alerts, runbooks, and oncall readiness.
- Oncall can mitigate. Owners must fix root causes.

## Required artifacts

This ops module is required reading:

- `docs/ops/01-slos-slis.md`
- `docs/ops/03-monitoring-and-oncall.md`
- `docs/ops/04-incident-response.md`
- Runbooks under `docs/ops/runbooks/`
