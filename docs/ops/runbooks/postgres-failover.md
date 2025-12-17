# Runbook: Postgres failover

This runbook covers controlled failover of the control plane Postgres primary.

Exact commands depend on your HA solution (managed service, Patroni, etc). The steps and safety constraints do not change.

## Symptoms

- Control plane API returns 5xx with DB connection errors
- Postgres primary is down or unreachable
- Replication lag alerts or replica promotion events
- Alerts:
  - Postgres primary down
  - Postgres replication lag high
  - API DB error rate spike

## Impact

- Control plane may be unavailable
- Deploys and config mutations fail
- Reconciliation may stall

## Safety rules (avoid split-brain)

- Confirm only one node is writable.
- If the old primary is partially alive, isolate it before promoting a replica.
- Disable any automatic failover if it conflicts with manual steps.

## Immediate actions

1. Declare incident if control plane is impacted.
2. Identify HA topology:
   - primary
   - replicas
   - service discovery mechanism (DNS, VIP, proxy)
3. Check replica health and replication lag.

## Decision points

### If primary is unhealthy but reachable

- Prefer a controlled switchover if possible.
- If primary is corrupted or cannot commit, proceed to failover.

### If primary is down

- Proceed to failover if at least one replica is healthy.

### If all replicas are stale or down

- Do not promote blindly.
- Consider restore from backups:
  - see `docs/ops/07-backup-restore-runbook.md`

## Failover procedure (generic)

### 1) Isolate old primary

- Remove it from service discovery (or block writes).
- Ensure clients cannot still connect and write to it.

### 2) Promote the best replica

Criteria:

- lowest replication lag
- healthy disk and CPU
- located in the same region as control plane if possible

Promote:

- use your HA tool to promote to primary
- wait for it to accept writes

### 3) Update service discovery

- Point the `postgres` endpoint to the new primary.
- Restart or reload control plane services so they reconnect.

### 4) Verify

- Run a write transaction (a safe, small write).
- Verify API health endpoints.
- Monitor:
  - error rate
  - latency
  - connection counts
  - replication status for remaining replicas

### 5) Rebuild the old primary as a replica

- Ensure it is fully wiped of old timeline state.
- Re-seed from new primary.
- Re-add to HA cluster.

## Post-failover actions

- Review whether failover was caused by:
  - capacity exhaustion
  - disk full
  - bad deploy or migration
  - network partition
- If caused by migration, review migration safety for future changes.

## Verification checklist

- Only one writable primary exists
- API availability returns to normal
- Reconcile backlog begins decreasing
- Replication resumes to at least one replica

## Escalation

Escalate if:

- split-brain suspected
- data corruption suspected
- failover repeats within 24 hours
