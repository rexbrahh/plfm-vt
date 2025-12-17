# Disaster recovery

Disaster recovery (DR) is the set of practices that let us recover from catastrophic failures: region loss, unrecoverable database failure, or widespread data corruption.

DR is not "hope". DR is a rehearsed plan with verified backups.

## DR goals

- Restore service within target RTO
- Restore data within target RPO
- Minimize human error via documented, practiced steps

## RTO and RPO targets (starting point)

These targets should be revised based on customer expectations and product maturity.

| Component | RTO | RPO | Notes |
|---|---:|---:|---|
| Control plane API (minimal) | 2 hours | 15 minutes | Requires Postgres restore or failover |
| App runtime in existing region | 1 hour | 0 | Apps continue if hosts are up, even if API is degraded |
| Full region rebuild | 24 hours | 1 hour | Cold-standby assumptions |
| Volume restore (single volume) | 4 hours | 24 hours | Depends on snapshot frequency |

## Critical assets

### Control plane state (Postgres)

Contains:

- orgs, projects, apps, envs
- releases, desired state specs
- inventory: hosts, endpoints, volumes
- events and audit logs (if stored)

### Secrets

- secrets are delivered to instances via a fixed file format
- encryption keys (KMS, HSM, or equivalent) are the true root of trust
- losing keys means losing secrets, even if data exists

### Storage and snapshots

- volume snapshots and backup artifacts
- snapshot metadata and indexes

### Edge configuration

- endpoint mappings
- IPv4 allocation state (if dedicated IPv4 add-on exists)
- anycast or DNS routing config

## DR scenarios and playbooks

### Scenario A: Control plane down, data plane still running

Goal: restore API and reconciliation without disrupting running workloads.

Actions:

1. Declare incident (Sev0 or Sev1 depending on impact).
2. Verify that hosts are still running workloads.
3. Restore control plane services:
   - prioritize API read-only mode if possible
   - then scheduler and reconciler
4. Validate Postgres health or run failover:
   - see `docs/ops/runbooks/postgres-failover.md`
5. Once API is stable, resume reconciler gradually to avoid retry storms.
6. Confirm time-to-converge SLI returns to normal.

### Scenario B: Postgres unrecoverable corruption

Goal: restore from backups with acceptable RPO.

1. Freeze mutations:
   - disable deploys and config writes
   - keep read endpoints if possible
2. Restore Postgres into a new cluster from last good backup.
3. Validate integrity:
   - schema migrations match expected version
   - basic reads work
4. Switch control plane to new Postgres (service discovery update).
5. Resume writes in controlled fashion.
6. Reconcile and repair any drift.

### Scenario C: Full region outage

Goal: fail over to alternate region (if available) or rebuild.

If multi-region exists:

1. Shift edge traffic to healthy region.
2. Promote standby control plane in target region.
3. Reconcile desired state into available hosts.
4. Verify endpoint availability.

If single-region (v1):

1. Declare disaster.
2. Restore control plane in new facility or provider if needed.
3. Bring up a minimal host fleet.
4. Restore critical customer volumes based on snapshots.
5. Communicate realistic recovery timelines and partial restores.

### Scenario D: Snapshot store loss

Goal: maintain operational service but accept reduced restore ability.

1. Confirm blast radius and whether recent backups exist elsewhere.
2. Increase snapshot frequency only if store is healthy (avoid writing into a failing system).
3. Prioritize restoring the snapshot store from its own backups.
4. Run a restore drill for at least one volume to verify.

## DR preparation requirements

- Backups are automated and monitored (success and freshness alerts).
- Restore procedures are written and tested:
  - see `docs/ops/07-backup-restore-runbook.md`
- Quarterly DR drills:
  - simulate Postgres primary loss
  - simulate control plane outage with data plane still running
  - simulate restoring a volume from snapshot

## DR communications

Disasters require proactive communication:

- status page updates with clear scope
- internal updates with decision logs
- after recovery, publish a postmortem and action plan
