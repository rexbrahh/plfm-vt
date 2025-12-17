# docs/adr/0011-storage-local-volumes-async-backups.md

## Title

Persistent storage uses local volumes with asynchronous backups

## Status

Locked

## Context

We need a storage model for workloads that:

* supports stateful services and persistent data
* is simple to implement and operate on bare metal early
* avoids building a distributed storage system before product proof
* aligns with multi-node scheduling and failure domains
* makes backup and restore explicit and testable

We also want to avoid the default assumption that storage is shared and strongly consistent across nodes.

## Decision

1. **Persistent storage is provided as local volumes attached to a host.**

* a volume physically resides on a specific node
* a workload instance scheduled elsewhere cannot attach the same volume without a migration operation

2. **Backups are asynchronous and explicit.**

* volumes are backed up out-of-band to a remote backup store
* backups do not provide synchronous replication or zero data loss guarantees by default

3. **The control plane tracks volume ownership, attachment, and backup metadata** as first-class state.

* attach and detach are controlled operations
* restore is a controlled operation that creates a new volume from a snapshot/backup

4. **Scheduling must respect volume locality.**

* workloads that require a volume are scheduled to the node where the volume resides
* if that node is unavailable, the workload is unavailable until restore or migration occurs

## Rationale

* Building or operating distributed storage is a major complexity multiplier.
* Local volumes are simple, fast, and match bare metal economics.
* Async backups provide a practical durability story without forcing replication into v1.
* This aligns with an early-stage platform where operational simplicity matters more than perfect availability for stateful workloads.

## Consequences

### Positive

* Simple implementation and predictable performance
* Clear failure model and clear operational responsibility
* Backup/restore can be validated and automated without complex storage dependencies

### Negative

* Node failure can take stateful workloads down until recovery actions occur
* Volume migration across nodes is non-trivial and may be slow
* Not suitable for workloads requiring synchronous multi-node consistency without higher-level replication in the app

## Alternatives considered

1. **Networked shared storage by default (NFS, Ceph, etc)**
   Rejected for v1 due to operational complexity, performance variability, and failure modes.

2. **Synchronous replication (multi-node mirrored volumes)**
   Rejected for v1 due to complexity and the need to design consensus and storage coordination.

3. **Stateless only (no persistent volumes)**
   Rejected because it limits the platform too severely and blocks many real workloads.

## Invariants to enforce

* A volume is attached to at most one microVM at a time unless we explicitly support shared read-only mounts (not in v1).
* Volume attachment and detachment are recorded as events with audit metadata.
* Scheduler must not violate locality constraints.
* Backup operations must be observable and have integrity verification.
* Restore must be idempotent and must not overwrite an existing volume in place.

## What this explicitly does NOT mean

* We are not providing a distributed filesystem in v1.
* We are not guaranteeing zero downtime failover for stateful workloads.
* We are not guaranteeing zero data loss on node failure between backup intervals.
* We are not implementing multi-writer shared volumes in v1.

## Open questions

* Backup backend choice (object storage, remote server, etc) and encryption model.
* Snapshot mechanism (filesystem snapshots vs block-level snapshots) on the host.
* How we expose backup policies to users (retention, frequency) in the manifest or separate config.

Proceed to **ADR 0012** when ready.
