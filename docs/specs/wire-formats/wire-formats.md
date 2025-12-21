Below is a concrete spec you can drop into the repo as-is.

---

# Wire Formats and API Contracts: Protobuf and JSON Policy (v1)

**File:** `docs/specs/wire-formats/wire-formats.md`
**Status:** Draft
**Owner:** Platform team

## 1. Purpose

This document defines where we use **Protocol Buffers** and where we use **JSON (and JSON variants)** across the platform, plus the concrete implementation rules needed to make the choice safe: schema ownership, versioning, compatibility, event envelopes, error modeling, and migration.

The intended outcome is:

* Strong, evolvable contracts between machines (node agents, schedulers, controllers).
* Human friendly interfaces at the edges (CLI, web console, debugging).
* One consistent approach to streaming logs and events.
* No accidental lock-in to “whatever shape our structs happen to serialize into today”.

## 2. Goals

1. **Protobuf as the dominant machine contract** for node-to-node and service-to-service communication.
2. **JSON at human interfaces** (CLI output, web console, debugging endpoints), including streaming variants.
3. A **single event envelope** format that supports long-lived storage and replay.
4. **Backwards and forwards compatibility** rules that are enforced in CI.
5. A clean **migration strategy** from existing JSON internal messages to protobuf without breaking ops.

## 3. Non-goals

* Designing the full external public REST API for third parties. (CLI is the primary surface in v1.)
* Solving full multi-region replication semantics here. (Only the encoding and contract layer.)

## 4. Definitions

* **Control Plane:** API and services that create desired state, run scheduling, reconciliation, and event materialization.
* **Host Agent:** daemon on each bare-metal host that boots and manages microVMs, networking, volumes, secrets delivery.
* **Event Log:** append-only stream of platform events used for debugging, auditing, and materialized views.
* **Human Surface:** CLI, console UI, and any debugging endpoints intended for people.

## 5. Format selection policy

### 5.1 Encoding matrix

| Surface                                                      | Encoding                              | Why                                                            |
| ------------------------------------------------------------ | ------------------------------------- | -------------------------------------------------------------- |
| Service-to-service RPC inside control plane                  | **gRPC + Protobuf**                   | Strong contracts, schema evolution, streaming, lower ambiguity |
| Host agent control RPC (control plane ↔ agent)               | **gRPC + Protobuf**                   | Same as above, plus reduces drift and patchy JSON semantics    |
| High-rate watch streams (events, heartbeats, status updates) | **Protobuf streaming**                | Efficient incremental updates, stable types                    |
| Event log storage payload                                    | **Protobuf inside a stable envelope** | Replayable, compact, versioned; decodable later                |
| CLI output for humans                                        | **Text**                              | Ergonomics                                                     |
| CLI output for scripting (`--json`)                          | **JSON derived from protobuf**        | Stable machine output, easy piping                             |
| Web console API                                              | **JSON** (backed by protobuf models)  | Browser friendly, debuggable                                   |
| Logs stream to humans                                        | **NDJSON** or **text**                | Easy tail, simple tooling                                      |
| Config manifests                                             | **TOML or YAML** (not protobuf)       | Humans author these                                            |
| Debug endpoints (introspection)                              | **JSON**                              | Maximum accessibility                                          |
| One-off ad hoc internal experiments                          | Not allowed by default                | Must pick from this policy                                     |

### 5.2 Default rule

* If both parties are machines and the message is part of platform correctness: **Protobuf**.
* If a human reads it directly, or it is primarily for ad hoc tooling: **JSON or text**.

### 5.3 Exceptions

* If a component cannot reasonably speak gRPC (rare), it may use Protobuf payloads over HTTP/1.1 with `Content-Type: application/protobuf`. This must be documented per interface.

## 6. Transport standards

### 6.1 Internal RPC

* **Protocol:** gRPC over HTTP/2
* **Encoding:** Protobuf (proto3)
* **Security:** mTLS (service identity and node identity)
* **AuthZ:** enforced at the receiving service using identity from mTLS plus request context (org, project, env)
* **Compression:** allowed (gzip), optional zstd if supported consistently; compression must be negotiated and measured

### 6.2 Streaming semantics

Use gRPC streams for:

* event tailing from control plane to clients that can handle it (agents, internal tools)
* heartbeat and status channels (agent → control plane)
* long-running operations progress reporting (release creation, drain/evict, restore)

For human-facing log tail in CLI, expose a JSON streaming facade (NDJSON) even if internally it is a protobuf stream.

## 7. Protobuf as the source of truth

### 7.1 Proto repository layout

```
proto/
  vt/
    common/v1/
      ids.proto
      time.proto
      errors.proto
      events.proto
    controlplane/v1/
      app.proto
      env.proto
      release.proto
      workload.proto
      endpoint.proto
      volume.proto
      secrets.proto
      events_api.proto
    agent/v1/
      agent.proto
      runtime.proto
      networking.proto
      storage.proto
      secrets_delivery.proto
```

Rules:

* Packages include explicit version segments: `vt.controlplane.v1`.
* Do not mix multiple versions in a single file.

### 7.2 Language generation

* Use `buf` for linting and breaking-change detection.
* Generate:

  * Rust: `prost` + `tonic`
  * Go: `protoc-gen-go` + `protoc-gen-go-grpc`
* Web console uses JSON endpoints backed by server-side mapping. (No requirement for protobuf in the browser in v1.)

## 8. Compatibility rules (mandatory)

These rules apply to all `.proto` in the repo.

### 8.1 Allowed changes within a version (v1)

* Add new fields with new field numbers.
* Add new enum values.
* Add new RPC methods.
* Mark fields or methods as deprecated.

### 8.2 Breaking changes (require v2)

* Remove a field or reuse its field number.
* Change a field type.
* Change semantics in a way that old clients interpret differently.
* Renumber fields.
* Rename packages in a way that changes type URLs used in event envelopes.

### 8.3 Field numbering and reservations

* Never reuse a field number once shipped.
* When removing a field, reserve the field number and name:

```proto
reserved 12;
reserved "old_field_name";
```

### 8.4 Enums

* First enum value must be `*_UNSPECIFIED = 0`.
* Never repurpose an enum value.

### 8.5 Presence and patch semantics

Proto3 defaults are not enough for patch operations. Use one of:

* `optional` fields for presence, or
* `google.protobuf.*Value` wrappers, or
* explicit `oneof` when tri-state matters, or
* `google.protobuf.FieldMask` for patch endpoints

Patch endpoints must be defined explicitly, not inferred from “missing JSON keys”.

## 9. JSON policy at the edges

### 9.1 JSON for CLI and console

Even if the backend uses protobuf, humans want JSON and text.

* CLI supports:

  * `--json` for machine-readable output
  * default human formatting for interactive use
* Web console endpoints return JSON objects designed for UI usage.

**Important constraint:** CLI `--json` output must be stable and versioned. It must not depend on incidental struct field order.

### 9.2 JSON variants

We standardize these variants:

1. **NDJSON** for streaming (`application/x-ndjson`)

   * Used for: `logs tail`, `events tail` in CLI
   * One JSON object per line
   * Each object includes a stable `type` and `ts`

2. **Problem Details JSON** for errors (`application/problem+json`)

   * Used for any JSON HTTP endpoints (console, debug endpoints, optional REST)
   * Includes `type`, `title`, `status`, `detail`, `instance`, plus platform fields like `request_id`

3. **JSON Merge Patch** (`application/merge-patch+json`) only for human-facing HTTP patch endpoints

   * Internally converted into protobuf FieldMask + value object
   * Avoid JSON Patch unless absolutely needed

## 10. Unified error model

### 10.1 Internal (gRPC)

Use `google.rpc.Status` semantics:

* gRPC code indicates broad class (InvalidArgument, NotFound, FailedPrecondition, Unavailable, PermissionDenied)
* Error details include typed metadata (resource ids, retry hints)

Define a common error proto:

```proto
syntax = "proto3";

package vt.common.v1;

message ErrorDetail {
  string request_id = 1;
  string resource_type = 2;
  string resource_id = 3;
  bool retryable = 4;
  uint32 retry_after_seconds = 5;
  string human_hint = 6;
  map<string,string> tags = 7;
}
```

### 10.2 External (JSON)

Map internal errors to `application/problem+json`. Include:

* `request_id`
* `code` (stable string, not just HTTP status)
* `retryable`
* `retry_after_seconds`

CLI exit codes are handled by the CLI policy, but the transport must provide enough structured info to implement consistent exit codes.

## 11. Event log encoding (critical)

The event log must remain decodable years later. Do not store “whatever struct we had at the time” as JSON without a schema identifier.

### 11.1 Event envelope

All events are stored and streamed using a stable envelope. Payload is protobuf bytes with an explicit type URL and schema version.

```proto
syntax = "proto3";

package vt.common.v1;

import "google/protobuf/timestamp.proto";

message EventEnvelope {
  string event_id = 1;                       // stable UUID
  uint64 sequence = 2;                       // per-stream monotonically increasing
  google.protobuf.Timestamp observed_at = 3;

  // Multi-tenant routing context
  string org_id = 10;
  string project_id = 11;
  string app_id = 12;
  string env_id = 13;

  // Aggregate routing
  string aggregate_type = 20;                // "app", "env", "workload", "endpoint", ...
  string aggregate_id = 21;

  // Event typing
  string event_type = 30;                    // stable string, ex: "workload.instance.started"
  uint32 schema_version = 31;                // schema version for this event_type
  string payload_type_url = 32;              // fully qualified protobuf message type URL
  bytes payload = 33;                        // protobuf-encoded message

  // Tracing and metadata
  string traceparent = 40;
  map<string,string> tags = 41;
}
```

### 11.2 Payload rules

* `event_type` is stable and human-readable.
* `payload_type_url` must remain stable across time. Do not rename packages casually.
* `schema_version` increments only when the event meaning changes materially.
* Additive payload changes that preserve meaning can keep the same `schema_version`.

### 11.3 Human streaming of events

For `vt events tail --json`, output NDJSON objects like:

```json
{"ts":"2025-12-20T22:10:05Z","seq":182771,"type":"workload.instance.started","app_id":"...","env_id":"...","payload":{...}}
```

Where `payload` is protobuf JSON mapping of the payload message type.

## 12. Concrete “where protobuf lives” list for this platform

### 12.1 Control plane internal RPC (protobuf)

* Scheduler placement decisions and constraints evaluation inputs and outputs
* Reconciliation loop desired-state fetch and diff application
* Workload lifecycle commands to agent (start, stop, replace, migrate)
* Networking programming (L4 endpoints, IP assignment, overlay membership updates)
* Volume attach, detach, snapshot, restore commands
* Secret bundle metadata distribution and delivery coordination
* Host heartbeats, host inventory, capacity reporting
* Event ingestion from agents and controllers into the event log

### 12.2 Agent RPC (protobuf)

* Boot request: OCI image reference, manifest digest, resource limits, env and secret bundle refs
* Runtime status: VM state, exit reasons, health checks, resource usage samples
* Networking status: assigned IPs, endpoint bindings, packet counters
* Storage status: volume mounts, usage, snapshot state
* Logs channel: structured log records (protobuf) into control plane, then rendered as text or NDJSON to humans

### 12.3 JSON and text surfaces

* CLI display output (human text)
* CLI `--json` output (JSON derived from protobuf)
* Web console APIs (JSON)
* Debug endpoints (JSON), including:

  * `describe`, `events`, `logs`, `health`, `metrics pointers`
* Manifests (TOML or YAML)

## 13. Implementation plan

### Phase 0: Repo and tooling

1. Add `proto/` tree and `buf.yaml`, `buf.gen.yaml`.
2. Add CI checks:

   * buf lint
   * buf breaking (against main branch)
3. Generate Rust and Go stubs and commit generated outputs or generate at build time (pick one and be consistent).

### Phase 1: Internal contracts first

1. Define protos for:

   * ids, timestamps, common error detail
   * event envelope
   * agent heartbeat and workload lifecycle
2. Implement gRPC control plane ↔ agent with mTLS.

### Phase 2: Event log stabilization

1. Emit all platform events as `EventEnvelope`.
2. Store raw envelopes in the event store (append-only).
3. Build materialized views off envelopes, not off ad hoc JSON.

### Phase 3: Human facades

1. CLI outputs:

   * human formatting remains unchanged
   * `--json` uses protobuf JSON mapping consistently
2. Implement NDJSON streams for:

   * logs tail
   * events tail

### Phase 4: Migration and cleanup

* For any legacy JSON internal message:

  * introduce protobuf equivalent
  * run dual publishing for one release window if needed
  * remove legacy JSON once all nodes upgrade past a minimum version

## 14. Operational requirements

### 14.1 Version negotiation

* Agents must report:

  * agent build version
  * supported protobuf API versions (ex: `agent.v1`)
* Control plane must reject incompatible agents with a clear error event and remediation hint.

### 14.2 Observability and debugging

Required tools:

* `vt events tail` renders envelopes
* `vt debug decode-event <event_id>` decodes payload bytes using a local registry of known types
* `vt debug grpc-call` for internal operators (admin only)

## 15. Security considerations

* Protobuf does not remove the need for validation.

  * All inbound messages must be validated exactly like JSON would be.
* mTLS identities are not authorization.

  * AuthZ still checks org, project, env access per request.
* Event payloads must avoid secrets.

  * Secret material must never enter the event log, only references and digests.

## 16. Open questions (to resolve, but not blockers for v1)

1. Do we want a public REST API in v1, or do we treat CLI and console as the only supported clients?
2. Do we want zstd compression on event envelopes at rest, or leave compression to storage layer?
3. Should logs ingestion be raw text or structured protobuf log records? (Recommendation: structured ingestion, human rendering at the edge.)

---
