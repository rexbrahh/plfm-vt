# Backup and restore runbook

This runbook covers backups and restores for the platform control plane and storage systems.

## Safety rules

- Do not run restore procedures in production unless required for incident mitigation.
- Use a staging environment to test restore steps quarterly.
- Every backup job must have:
  - success signal
  - freshness signal (last successful time)
  - restore test evidence

## What we back up

### 1) Postgres (control plane)

Back up:

- full base backups
- WAL / incremental logs for point-in-time recovery (PITR)
- schema migration history (should also be in git, but back up anyway)

### 2) Snapshot metadata store

If snapshot metadata is separate from Postgres, it must be backed up on the same cadence.

### 3) Volume snapshots

- periodic snapshots per volume
- retention policies
- verified restore of at least one representative volume per week (staging)

### 4) Configuration and secrets metadata

- infrastructure as code repos are the source of truth
- secret encryption keys are backed up and access controlled
- audit logs are retained

## Backup schedules (starting point)

| Asset | Frequency | Retention | Notes |
|---|---:|---:|---|
| Postgres base backup | daily | 30 days | Keep at least 2 full backups |
| Postgres WAL for PITR | continuous | 7 days | Tune based on cost |
| Volume snapshots | daily | 14 days | Critical volumes may need hourly |
| Volume backups (cold) | weekly | 90 days | Optional but recommended |

Adjust per customer tier and cost model.

## Backup verification

Backups that cannot be restored are not backups.

Minimum verification:

- Postgres: restore into staging weekly and run smoke tests
- Volumes: restore one volume weekly and validate checksums or app level checks
- Snapshot store: validate metadata consistency

## Restore procedures

### Restore Postgres into a new cluster (standard path)

Use this for corruption, operator error, or disaster recovery.

1. Freeze control plane writes:
   - disable deploys and config mutations
   - keep reads if possible
2. Provision a new Postgres cluster with equal or larger capacity.
3. Restore latest base backup.
4. Apply WAL to desired recovery point (PITR).
5. Run integrity checks:
   - connection tests
   - migration version
   - critical queries and indexes
6. Switch control plane to new Postgres:
   - update service discovery
   - restart API components in controlled order
7. Resume writes.
8. Monitor:
   - API error rate
   - reconcile backlog
   - Postgres replication (if replicas exist)

### Restore a single volume from snapshot

1. Identify target snapshot:
   - prefer last known good
   - confirm timestamp and retention
2. Stop writes:
   - scale workload to zero or detach volume
   - ensure the filesystem is cleanly unmounted if possible
3. Create a new volume from snapshot (do not overwrite in place).
4. Attach restored volume to a recovery instance.
5. Validate data:
   - filesystem check (offline)
   - application level checks if available
6. Swap volumes:
   - detach old, attach restored
   - redeploy workload
7. Keep old volume for forensic window (time boxed).

### Restore after accidental config deletion

If desired state is missing but workloads still run:

1. Restore control plane state (Postgres) to a point before deletion, into a separate cluster.
2. Extract required rows/configuration.
3. Reapply configuration to the live cluster carefully (manual merge).
4. Validate via reconcile and describe outputs.

This is safer than full rollback of Postgres if only one tenant is affected.

## Monitoring and alerts

Required alerts:

- Postgres backup failed
- Postgres backup stale (no success in expected window)
- WAL archive stale
- Volume snapshot failed
- Volume snapshot stale
- Restore test failed

Each alert must link to the relevant runbook section.

## Operational checklists

### Daily

- verify last Postgres backup success time
- verify snapshot jobs success rate
- verify backup storage utilization

### Weekly

- perform Postgres restore test in staging
- perform one volume restore test in staging
- review backup retention and cost

### Quarterly

- full DR drill for Postgres failover and control plane rebuild
