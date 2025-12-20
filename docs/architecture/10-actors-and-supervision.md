# docs/architecture/10-actors-and-supervision.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-19

## Purpose

This document describes how actors and supervision trees provide the concurrency model for implementing reconciliation loops in both the node agent and control plane.

This is an implementation pattern document. It explains **how** to implement the reconcilers and state convergence described in:
- `docs/architecture/02-data-plane-host-agent.md`
- `docs/architecture/04-state-model-and-reconciliation.md`
- `docs/specs/scheduler/reconciliation-loop.md`

## What actors are (and are not)

### What we mean by "actor"

An actor is a concurrent unit of execution that:
- owns a mailbox (bounded channel) for incoming messages
- processes messages sequentially (no internal concurrency)
- owns mutable state that is not shared with other actors
- communicates with other actors only via message passing
- can spawn child actors and supervise their lifecycle

### What this is not

This is not:
- distributed actors or Erlang-style clustering (actors are per-process)
- a replacement for the event log (actors emit events to the log, they don't replace it)
- mandatory for all code (only for components that need serialized side effects)

## Why actors fit our architecture

Our architecture requires:
- **Idempotent operations**: actors process one message at a time, making idempotency easier to reason about
- **Restart-safe execution**: supervisors restart failed actors with backoff
- **Bounded reconciliation**: actors coalesce message storms into single reconcile passes
- **Observable state machines**: actors emit events for every transition, powering CLI introspection

The core rule: **one actor owns one resource's mutable state and side effects**.

This directly maps to our resource model:
- `instance/<id>` → one actor owns its Firecracker lifecycle
- `volume_attachment/<id>` → one actor owns mount/detach
- `route/<id>` → one actor owns edge routing rules for a hostname/port binding
- `app_env/<id>` → one actor serializes reconciliation for that env

## Node agent supervision tree

The node agent is the primary place where actors prevent races. Anything that touches Firecracker, networking, volumes, secrets files, cgroups, iptables/nft, image cache, or log streaming should be serialized per resource.

### Recommended v1 structure

```
NodeSupervisor
├── ControlPlaneStreamActor
│   └── Owns: event cursor, reconnect backoff, heartbeats
│
├── InstanceSupervisor (dynamic children, keyed by instance_id)
│   └── InstanceActor(instance_id)
│       └── Owns: Firecracker lifecycle, local instance directories
│
├── VolumeSupervisor (dynamic children)
│   └── VolumeAttachmentActor(attachment_id)
│       └── Owns: mount, format checks, snapshot hooks, detach
│
├── OverlaySupervisor
│   └── WireGuardActor
│       └── Owns: peer config, AllowedIPs, MTU
│
├── ImageCacheSupervisor
│   ├── ImagePullActor(image_ref) (dynamic)
│   │   └── Owns: at-most-one pull per digest per node
│   └── ImageGCAgent
│       └── Owns: disk pressure GC, refcounts, pruning
│
└── ObservabilitySupervisor
    ├── LogTailActor (per instance or per node)
    └── MetricsScrapeActor
```

### Minimal v1 starting point

Start with fewer actors and split later:
1. **InstanceActor** (single actor owns full lifecycle)
2. **ImagePullActor** (dedupe pulls, cache)
3. **WireGuardActor** (serialize overlay networking mutations)

Add supervisors, restart policies, and event emission as the system matures.

## Control plane supervision tree

In the control plane, actors are most valuable for:
- preventing concurrent reconciles of the same app env or release
- sequencing state transitions (create release → schedule → instruct nodes → observe readiness)
- coalescing storms (many events, one reconcile)

The core rule: **API writes desired state**, then **reconcile actors** drive it to current state by issuing commands and consuming node observations.

### Recommended v1 structure

```
ControlPlaneSupervisor
├── EventIngestorActor
│   └── Reads event log, feeds internal bus
│
├── ReconcileSupervisor (dynamic children)
│   ├── AppEnvActor(app_env_id)
│   │   └── Owns: desired state for an env, fans out to placement
│   └── ReleaseActor(release_id) (optional)
│       └── Owns: multi-step release state machine
│
├── PlacementSupervisor
│   └── PlacementActor(app_env_id or group_id)
│       └── Runs placement decisions, emits events (not imperative calls)
│
├── QuotaActor(org_id) (optional)
│   └── Owns: quota enforcement per org
│
└── DrainActor(node_id)
    └── Owns: drain-evict-reschedule workflow
```

## Actor execution pattern

Each actor runs the same reconciliation pattern described in `docs/architecture/04-state-model-and-reconciliation.md`:

### 1) Ingest inputs
- desired state updates (control plane) or commands (node agent)
- observed state updates (node agent publishes, control plane consumes)
- periodic tick (for retries, timeouts)

### 2) Compute diff
- compare `desired` vs `current`
- decide minimal next actions

### 3) Apply side effects idempotently
- "ensure X exists" not "create X"
- retries are safe

### 4) Emit events
- every transition and failure becomes an event
- this directly powers `ghostctl events tail`, `ghostctl wait`, and `ghostctl describe`

### Message coalescing

If multiple desired state updates arrive quickly, keep only the latest and run one reconcile. This prevents thrashing under event storms.

Example: 20 scale changes in 100ms → one reconcile pass with the final desired count.

## Restart strategies

Use simple, explicit policies:

### One-for-one restart
If `InstanceActor(A)` crashes, restart only A. Other instances are unaffected.

### Escalation
If a child keeps failing, bubble up to parent which can:
- mark the resource as `degraded`
- quarantine the node (for node agent)
- stop restarting after N attempts
- require a new desired-state bump to retry

### Backoff
Add **exponential backoff with jitter** on:
- actor restarts
- external calls (image pulls, control plane stream reconnects)

### State persistence for recovery

Persist just enough per actor to recover safely:
- last applied `spec_revision`
- in-progress phase (optional)
- durable local facts (paths, created tap name, allocated IP)

These should be stored in the node agent's local state store (see `docs/architecture/02-data-plane-host-agent.md`).

## Message design

Keep messages boring and typed. Avoid complex inheritance hierarchies.

### InstanceActor messages

```
ApplyDesired { spec_revision, spec }
ObservedUpdate { state_snapshot }
Tick
Stop { reason }
Drain { deadline }
```

### WireGuardActor messages

```
ApplyDesiredPeers { config_revision, peers }
Tick
```

### Coalescing rule

For `ApplyDesired` messages, the mailbox should dedupe by keeping only the latest spec_revision. Multiple `ApplyDesired` messages in the mailbox collapse to one.

## Implementation guidance

### Rust (all components)

The platform is implemented in Rust. The actor pattern maps to:
- `tokio::spawn` per actor
- `mpsc::channel` for mailbox (bounded)
- supervisor task that monitors child handles via `JoinHandle`
- `select!` for multiplexing tick + mailbox

Key invariant: **no `Arc<Mutex<_>>` for actor state**. State lives in the actor task only.

```rust
// Actor trait pattern
#[async_trait]
pub trait Actor: Send + 'static {
    type Msg: Send;
    async fn handle(&mut self, msg: Self::Msg) -> Result<(), ActorError>;
}

// Supervisor manages child actors
pub struct Supervisor {
    children: HashMap<String, JoinHandle<()>>,
    restart_policy: RestartPolicy,
}
```

For control plane reconcilers, the same pattern applies with `tokio` tasks and bounded channels. The control plane actors emit events to the event log and consume from materialized views.

## Relationship to existing specs

| Spec | Actor role |
|------|------------|
| `docs/specs/scheduler/reconciliation-loop.md` | PlacementActor implements the per-group reconcile algorithm |
| `docs/specs/scheduler/drain-evict-reschedule.md` | DrainActor implements the drain workflow |
| `docs/specs/manifest/workload-spec.md` | InstanceActor receives WorkloadSpec via ApplyDesired |
| `docs/specs/state/event-types.md` | Actors emit these event types on transitions |
| `docs/specs/runtime/guest-init.md` | InstanceActor manages the Firecracker side of handshake |

## CLI introspection enabled by actors

Because actors emit structured events for every transition and failure:

- `ghostctl events tail --instance <id>` shows the actor's state machine live
- `ghostctl wait --release <id>` waits for specific event predicates
- `ghostctl describe` can show:
  - desired spec revision
  - last observed revision
  - last reconcile error
  - restart count, last crash reason, mailbox depth

This is the "CLI as the product" story, backed by a system that is naturally introspectable.

## Non-goals

This document does not prescribe:
- distributed consensus between actors (not needed in v1)
- actor location transparency or migration
- complex actor frameworks or DSLs
- actors for request/response APIs (use normal async handlers)

## Implementation order

Aligned with `docs/engineering/implementation-plan.md` Milestone 3:

1. **Node agent InstanceActor** (single actor owns full lifecycle)
2. **ImagePullActor** (dedupe pulls, cache, GC later)
3. **WireGuardActor** (serialize overlay networking mutations)
4. Add supervisors, restart policies, and event emission
5. Control plane: **AppEnvActor** to serialize reconciles per env
6. Split actors into child actors only when needed for parallelism

## Next document

- `docs/specs/runtime/agent-actors.md` — detailed actor message schemas and lifecycle events
