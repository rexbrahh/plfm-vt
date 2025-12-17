# Runbook: Control plane down

## Symptoms

- CLI and console cannot create releases, fetch status, or stream events
- API health checks failing or returning 5xx
- Reconciliation backlog grows, desired state is not applied
- Alerts:
  - Control plane API availability SLO burn
  - API 5xx spike
  - Postgres connection errors (often a cause, not a symptom)

## Impact

- Customers cannot deploy or change config
- Existing workloads may continue running on hosts, but drift repair stops
- Secrets updates and scaling actions may be delayed

## Immediate actions (first 5 minutes)

1. Declare incident (Sev0 or Sev1 depending on scope).
2. Freeze non-essential deploys.
3. Establish incident channel, assign IC and scribe.
4. Determine scope:
   - single region vs global
   - API only vs API + Postgres + scheduler

## Triage checklist

### 1) Is this an edge or DNS problem?

- Check edge health and DNS routing to API endpoints.
- Confirm you can reach the API from multiple networks.

If only some networks fail, consider edge partial outage:
- see `edge-partial-outage.md`.

### 2) Is Postgres reachable and healthy?

- Check Postgres primary health, replication lag, connection counts.
- Look for:
  - "too many connections"
  - slow queries or lock contention
  - disk full events

If Postgres is failing or primary is down, switch to:
- `postgres-failover.md`

### 3) Did a deploy break the control plane?

- Identify last deploy timestamp.
- Compare to error onset.
- If a recent deploy correlates, rollback is the fastest mitigation.

Rollback guidelines:

- rollback API first if it is crashing
- rollback scheduler/reconciler if they are retry storming
- keep Postgres unchanged unless it is the failing component

### 4) Is reconciliation causing a thundering herd?

Signs:

- backlog spike
- massive retry rates
- Postgres CPU and connections spike
- host agents timing out

Mitigation:

- rate-limit reconciler
- temporarily pause non-critical controllers
- prioritize "health repair" controllers first

## Mitigation actions

Pick the safest mitigation that restores customer-facing behavior.

### Option A: Roll back the control plane deploy

- Roll back to last known good release.
- Verify:
  - API health endpoint returns OK
  - p95 latency stabilizes
  - error rate returns to baseline

### Option B: Fail over Postgres

If Postgres primary is down or corrupted:

- follow `postgres-failover.md`
- then restart API components to refresh connections

### Option C: Scale up API capacity

If API is overloaded but healthy:

- increase API replicas
- increase connection pool limits cautiously (avoid crushing Postgres)
- enable aggressive caching for reads (if implemented)

### Option D: Enable read-only mode (if supported)

If writes are causing failure but reads can be served:

- disable deploy and mutation endpoints
- keep status and logs endpoints available
- communicate clearly that the platform is read-only temporarily

## Verification

- API SLO burn rate stops
- CLI operations succeed:
  - status, describe, logs/events
  - create release (staging) if safe
- Reconcile backlog begins decreasing
- Postgres replication and connections return to normal

## Escalation

Escalate immediately if:

- suspected data loss or corruption
- split-brain risk in Postgres failover
- security incident suspected

## Follow-ups

- Ensure a postmortem is created.
- Update this runbook with anything that was unclear during the incident.
