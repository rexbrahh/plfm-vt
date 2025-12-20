# docs/specs/runtime/agent-actors.md

Status: draft  
Owner: TBD  
Last reviewed: 2025-12-19

## Purpose

Define the actor message schemas, lifecycle events, and operational contracts for node agent actors.

This spec complements:
- `docs/architecture/10-actors-and-supervision.md` (supervision tree design)
- `docs/specs/state/event-types.md` (control-plane event log payloads)
- `docs/specs/manifest/workload-spec.md` (WorkloadSpec contract)

Note: the "events" referenced in this document are **node-agent local signals**
(structured logs/metrics and optional local event streams). They are not
necessarily persisted into the control-plane event log unless explicitly
specified in `docs/specs/state/event-types.md`.

## Scope

This spec defines:
- message types for each node agent actor
- lifecycle events emitted by actors
- mailbox sizing and coalescing rules
- recovery and state persistence requirements
- observable metrics

This spec does not define:
- control plane actor messages (future spec)
- exact Rust type definitions (implementation detail)
- network protocol for control plane stream (see API specs)

## InstanceActor

Owns the full lifecycle of a single microVM instance.

### Identity

Actor key: `instance/<instance_id>`

One actor per allocated instance on this node.

### Messages

#### ApplyDesired

Received when desired state for this instance changes.

```
ApplyDesired {
  spec_revision: u64,
  spec: WorkloadSpec,
  desired_state: DesiredState,  // running | draining | stopped
}
```

Coalescing: keep only the message with highest `spec_revision` in mailbox.

#### ObservedTick

Periodic self-check to detect drift between expected and actual state.

```
ObservedTick {
  tick_id: u64,
}
```

Coalescing: keep only the latest tick_id.

#### ExecRequest

Request to establish an exec session into the running instance.

```
ExecRequest {
  session_id: string,
  command: string[],
  grant_token: string,
  reply_to: channel,
}
```

No coalescing. Each exec request is distinct.

#### Stop

Forceful stop request (not graceful drain).

```
Stop {
  reason: StopReason,  // node_shutdown | admin_kill | eviction
}
```

### State machine

```
               ApplyDesired(running)
                      │
                      ▼
    ┌─────────────────────────────────┐
    │           preparing             │
    │  (fetch image, create dirs)     │
    └─────────────────────────────────┘
                      │
                      ▼
    ┌─────────────────────────────────┐
    │            booting              │
    │  (start Firecracker, handshake) │
    └─────────────────────────────────┘
                      │
                      ▼
    ┌─────────────────────────────────┐
    │             ready               │◄────── steady state
    │  (serving, health checks pass)  │
    └─────────────────────────────────┘
                      │
          ApplyDesired(draining)
                      │
                      ▼
    ┌─────────────────────────────────┐
    │            draining             │
    │  (removed from LB, grace period)│
    └─────────────────────────────────┘
                      │
          drain complete or timeout
                      │
                      ▼
    ┌─────────────────────────────────┐
    │            stopped              │
    │  (VM terminated, cleanup done)  │
    └─────────────────────────────────┘
                      │
                      ▼
    ┌─────────────────────────────────┐
    │       garbage_collected         │
    │  (actor stops, dirs removed)    │
    └─────────────────────────────────┘
```

### Events emitted

| Transition | Event type | Notes |
|------------|------------|-------|
| allocated → preparing | `instance.status_changed` | status=preparing |
| preparing → booting | `instance.status_changed` | status=booting |
| booting → ready | `instance.status_changed` | status=ready, includes boot_duration_ms |
| ready → draining | `instance.status_changed` | status=draining |
| draining → stopped | `instance.status_changed` | status=stopped, includes stop_reason |
| any → failed | `instance.status_changed` | status=failed, includes error details |
| crash detected | `instance.crashed` | includes crash_reason, exit_code if available |
| exec started | `instance.exec_started` | includes session_id |
| exec ended | `instance.exec_ended` | includes session_id, exit_code |

### Persisted state

Stored in node agent local DB (survives agent restart):

```
instance_actor_state {
  instance_id: string,
  last_applied_spec_revision: u64,
  current_phase: Phase,
  firecracker_socket_path: string,
  tap_device_name: string,
  root_disk_path: string,
  scratch_disk_path: string,
  overlay_ip: string,
  created_at: timestamp,
  last_health_check_at: timestamp,
}
```

### Recovery on agent restart

1. Load persisted state from local DB
2. Scan Firecracker socket to check if VM is still running
3. If VM running and phase was `ready`: resume health checks
4. If VM not running and phase was not `stopped`: emit `instance.crashed`, restart per policy
5. If VM not running and phase was `stopped`: proceed to garbage collection

## ImagePullActor

Ensures at-most-one concurrent pull per image digest per node.

### Identity

Actor key: `image/<digest>`

Dynamic actor spawned on first pull request, stopped after TTL of no references.

### Messages

#### EnsurePulled

Request to ensure an image is available locally.

```
EnsurePulled {
  image_ref: string,        // registry/repo:tag or @digest
  expected_digest: string,  // sha256:...
  reply_to: channel,        // for completion notification
}
```

Coalescing: dedupe by digest. Multiple requesters get same reply.

#### ReleaseRef

Decrement reference count for an image.

```
ReleaseRef {
  instance_id: string,
}
```

#### GCCheck

Periodic check if image can be garbage collected.

```
GCCheck {
  tick_id: u64,
}
```

### Events emitted

| Event | Event type | Notes |
|-------|------------|-------|
| Pull started | `image.pull_started` | includes image_ref, digest |
| Pull completed | `image.pull_completed` | includes digest, size_bytes, duration_ms |
| Pull failed | `image.pull_failed` | includes digest, error |
| Image evicted | `image.evicted` | includes digest, reason (gc, disk_pressure) |

### Persisted state

```
image_cache_entry {
  digest: string,
  root_disk_path: string,
  size_bytes: u64,
  pulled_at: timestamp,
  last_used_at: timestamp,
  ref_count: u32,
}
```

## WireGuardActor

Owns the node's overlay (WireGuard) configuration.

### Identity

Actor key: `wireguard/<node_id>`

### Messages

#### ApplyMeshConfig

Apply the desired peer set and overlay settings for this node.

```
ApplyMeshConfig {
  config_revision: u64,
  peers: WireGuardPeer[],
  mtu: u32,
}
```

Coalescing: keep only the message with the highest config_revision.

#### Tick

Periodic self-check (optional) to detect drift and refresh keepalives.

```
Tick {
  tick_id: u64,
}
```

### Events emitted

| Event | Event type | Notes |
|-------|------------|-------|
| Config applied | `wireguard.config_applied` | includes config_revision |
| Config failed | `wireguard.config_failed` | includes error |
| Peer unreachable | `wireguard.peer_unreachable` | includes peer node_id (best effort) |

### Persisted state

```
wireguard_state {
  last_applied_config_revision: u64,
  last_applied_at: timestamp,
}
```

## VolumeAttachmentActor

Owns mount/detach lifecycle for a volume attachment.

### Identity

Actor key: `volume_attachment/<attachment_id>`

### Messages

#### EnsureAttached

Ensure volume is attached and mounted for target instance.

```
EnsureAttached {
  spec_revision: u64,
  volume_id: string,
  instance_id: string,
  mount_path: string,
  read_only: bool,
}
```

#### Detach

Detach volume from instance (graceful).

```
Detach {
  reason: DetachReason,  // instance_stopping | migration | admin
}
```

#### SnapshotRequest

Trigger a snapshot of the attached volume.

```
SnapshotRequest {
  snapshot_id: string,
  reply_to: channel,
}
```

### Events emitted

| Event | Event type | Notes |
|-------|------------|-------|
| Attached | `volume_attachment.attached` | includes volume_id, instance_id |
| Detached | `volume_attachment.detached` | includes reason |
| Attach failed | `volume_attachment.attach_failed` | includes error |
| Snapshot started | `volume_attachment.snapshot_started` | includes snapshot_id |
| Snapshot completed | `volume_attachment.snapshot_completed` | includes snapshot_id, size_bytes |

## ControlPlaneStreamActor

Maintains the long-lived connection to control plane.

### Identity

Actor key: `control_plane_stream` (singleton per node)

### Messages

#### Connect

Initial connection or reconnection request.

```
Connect {
  force: bool,  // force reconnect even if connected
}
```

#### SendHeartbeat

Trigger a heartbeat send.

```
SendHeartbeat {
  tick_id: u64,
}
```

#### StreamEvent

Internal: received event from stream.

```
StreamEvent {
  event: EventEnvelope,
}
```

### State machine

```
    disconnected ──Connect──► connecting
         ▲                         │
         │                         ▼
    backoff_wait ◄──failure── connected
         │                         │
         └───────timer────────────►│
```

### Events emitted

| Event | Event type | Notes |
|-------|------------|-------|
| Connected | `node.control_plane_connected` | includes cursor position |
| Disconnected | `node.control_plane_disconnected` | includes reason |
| Reconnecting | `node.control_plane_reconnecting` | includes attempt, backoff_ms |

### Persisted state

```
stream_actor_state {
  last_event_cursor: u64,
  last_connected_at: timestamp,
  consecutive_failures: u32,
}
```

## Mailbox configuration

### Sizing

| Actor | Mailbox capacity | Rationale |
|-------|------------------|-----------|
| InstanceActor | 16 | Low volume, coalesced |
| ImagePullActor | 64 | Many concurrent requesters |
| WireGuardActor | 32 | Peer set updates, keepalive refresh |
| VolumeAttachmentActor | 8 | Very low volume |
| ControlPlaneStreamActor | 256 | High event throughput |

### Backpressure

If mailbox is full:
- **InstanceActor**: block sender (critical path)
- **ImagePullActor**: block sender (prevents runaway pulls)
- **WireGuardActor**: drop oldest non-critical message, log warning
- **ControlPlaneStreamActor**: drop oldest event, increment dropped counter

## Observable metrics

Each actor exposes:

| Metric | Type | Labels |
|--------|------|--------|
| `actor_mailbox_depth` | gauge | actor_type, actor_id |
| `actor_messages_processed_total` | counter | actor_type, actor_id, message_type |
| `actor_message_processing_duration_seconds` | histogram | actor_type, message_type |
| `actor_restarts_total` | counter | actor_type, actor_id |
| `actor_last_restart_timestamp` | gauge | actor_type, actor_id |
| `actor_state` | gauge (enum) | actor_type, actor_id |

## Error handling

### Transient errors

Retry with exponential backoff:
- Network timeouts
- Firecracker API temporary failures
- Image registry rate limits

### Permanent errors

Emit error event and transition to failed state:
- Invalid spec (schema violation)
- Missing required volume
- Authentication failure

### Crash handling

Supervisor catches panics/crashes and:
1. Emits `actor.crashed` event with stack trace (redacted)
2. Increments restart counter
3. Applies backoff: `min(base * 2^attempts, max_backoff)` with jitter
4. Restarts actor with recovered state
5. After max restarts, marks resource as `degraded` and stops retrying

Default backoff parameters:
- base: 100ms
- max_backoff: 30s
- max_restarts: 5 (within 5 minutes)
- jitter: 0-25%

## Testing contracts

Each actor must support:

1. **Deterministic message injection**: test harness can send messages in controlled order
2. **State inspection**: test can read current actor state without side effects
3. **Side effect mocking**: all external calls (Firecracker, network, filesystem) behind interfaces
4. **Event capture**: test can capture all emitted events

Example test pattern:

```
given: InstanceActor in state=ready
when: ApplyDesired(desired_state=draining) received
then:
  - state transitions to draining
  - emits instance.status_changed(status=draining)
  - starts drain timer
```
