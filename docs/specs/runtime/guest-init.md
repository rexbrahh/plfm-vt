# Guest Init (v1)

Status: approved  
Owner: Runtime Team  
Last reviewed: 2025-12-17

## Purpose

Define the contract for the platform-provided PID 1 inside each microVM.

Guest init is responsible for turning a Firecracker boot into a running workload instance. It bridges the host agent and the customer workload.

This spec is normative for:
- Guest init binary implementation
- Host agent config handshake
- Boot sequence and diagnostics

Delivery mechanism is specified in `docs/specs/runtime/guest-init-delivery.md`.

## Goals

- Deterministic boot and configuration
- Small trusted computing base inside guest
- Clear versioning and compatibility contract with host agent
- Structured diagnostics for failed boots
- Provide exec service for `plfm exec`

## Non-goals (v1)

- Running arbitrary init systems (systemd) as PID 1
- Mutating customer images to install platform agents
- Hot-reloading configuration without restart
- Running multiple unrelated processes

## Responsibilities (Normative)

Guest init MUST:

1. **Config Handshake**: Perform a config handshake with the host agent over vsock.
2. **Networking**: Configure networking inside the guest according to the contract.
3. **Volumes**: Mount volumes according to the contract.
4. **Secrets**: Materialize secrets to the fixed file format and permissions.
5. **Workload Launch**: Launch the workload process as PID 2+ with correct env, cwd, argv.
6. **Signal Handling**: Forward signals appropriately and reap zombies.
7. **Status Reporting**: Emit structured status back to host agent (ready, unhealthy, exit).
8. **Exec Service**: Provide an exec service endpoint for `plfm exec`.

Guest init MUST NOT:

- Contact the public internet during boot unless explicitly configured by user workload.
- Exfiltrate secrets via logs or diagnostics.
- Modify customer image files.
- Execute untrusted code during boot sequence.

## Versioning and Compatibility

### Version Fields

- `guest_init_version`: semver string, example "1.2.0"
- `guest_init_protocol`: integer, v1 = 1

### Compatibility Rules

- Host agent MUST reject connections from guest init with incompatible protocol versions.
- Host agent MUST surface a clear error: `guest_init_protocol_mismatch`.
- Guest init MUST reject config with unknown required fields.
- Both sides MUST ignore unknown optional fields (forward compatibility).

### Version Negotiation

During hello, guest init declares its protocol version. Host agent either:
- Accepts and sends config (compatible)
- Rejects with error message (incompatible)

## Handshake Protocol (v1)

Transport: vsock
- Host agent listens on vsock port 5161.
- Guest init connects as a client after boot.

Messages are newline-delimited JSON (NDJSON).

### Message Flow

```
Guest                    Host Agent
  |                           |
  |------ hello ------------->|
  |                           |
  |<----- config -------------|
  |                           |
  |------ ack --------------->|
  |                           |
  |------ status (config_applied) -->|
  |                           |
  |  (boot continues...)      |
  |                           |
  |------ status (ready) ---->|
  |                           |
```

### Guest -> Host: hello

```json
{
  "type": "hello",
  "guest_init_version": "1.0.0",
  "guest_init_protocol": 1,
  "instance_id": "01JEXAMPLE",
  "boot_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

Fields:
- `type`: always "hello"
- `guest_init_version`: semver of guest init binary
- `guest_init_protocol`: protocol version (1 for v1)
- `instance_id`: expected instance ID (from kernel cmdline or hardcoded for validation)
- `boot_id`: unique ID for this boot attempt (UUID)

### Host -> Guest: config

```json
{
  "type": "config",
  "config_version": "v1",
  "instance_id": "01JEXAMPLE",
  "generation": 7,
  "workload": {
    "argv": ["./server"],
    "cwd": "/app",
    "env": {
      "PORT": "8080",
      "RUST_LOG": "info"
    },
    "uid": 1000,
    "gid": 1000,
    "stdin": false,
    "tty": false
  },
  "network": {
    "overlay_ipv6": "fd00::1234",
    "gateway_ipv6": "fd00::1",
    "prefix_len": 128,
    "mtu": 1420,
    "dns": ["fd00::53"],
    "hostname": "i-01JEXAMPLE"
  },
  "mounts": [
    {
      "kind": "volume",
      "name": "data",
      "device": "/dev/vdc",
      "mountpoint": "/data",
      "fs_type": "ext4",
      "mode": "rw"
    }
  ],
  "secrets": {
    "required": true,
    "path": "/run/secrets/platform.env",
    "mode": "0400",
    "owner_uid": 0,
    "owner_gid": 0,
    "format": "dotenv",
    "bundle_version_id": "01JSECRET"
  },
  "exec": {
    "vsock_port": 5162,
    "enabled": true
  }
}
```

### Guest -> Host: ack

```json
{
  "type": "ack",
  "config_version": "v1",
  "generation": 7
}
```

Sent after config is received and parsed. Does not indicate config is applied.

### Guest -> Host: status

Status transitions during boot:

```json
{ "type": "status", "state": "config_applied", "timestamp": "2025-12-17T12:00:01Z" }
```

```json
{ "type": "status", "state": "ready", "timestamp": "2025-12-17T12:00:05Z" }
```

On failure:

```json
{
  "type": "status",
  "state": "failed",
  "reason": "mount_failed",
  "detail": "volume data: ext4 mount returned EINVAL",
  "timestamp": "2025-12-17T12:00:02Z"
}
```

### Status States

- `config_applied`: networking, volumes, secrets configured
- `ready`: workload process started, ready for traffic
- `failed`: boot failed (see `reason` and `detail`)
- `exited`: workload process exited (see exit_code in separate message)

### Failure Reasons

Standardized reason codes:
- `config_parse_failed`: could not parse config JSON
- `net_config_failed`: networking configuration failed
- `mount_failed`: volume mount failed
- `secrets_missing`: required secrets not provided
- `secrets_write_failed`: could not write secrets file
- `workload_start_failed`: could not exec workload command
- `workload_crashed`: workload exited immediately (crash loop)

## Networking Inside Guest (Normative)

Guest init MUST:

1. Configure the overlay IPv6 address on eth0 with /128 prefix.
2. Set default route to `gateway_ipv6`.
3. Set MTU to provided value.
4. Configure DNS by writing `/etc/resolv.conf` with provided servers.
5. Set hostname if provided.

If networking configuration fails, guest init MUST report `failed` with reason `net_config_failed`.

Implementation (iproute2 semantics):
```bash
ip link set dev eth0 mtu 1420
ip link set dev eth0 up
ip -6 addr add fd00::1234/128 dev eth0
ip -6 route replace default via fd00::1 dev eth0
```

## Secrets Materialization (Normative)

Guest init MUST write the secrets file atomically:
1. Write to temp path in same directory
2. fsync file
3. Rename to final path

Properties:
- Path: `/run/secrets/platform.env` (fixed in v1)
- Permissions: as specified in config (default 0400)
- Owner: as specified in config (default root:root)
- Format: dotenv (KEY=value, one per line)

If `secrets.required` is true and secrets data is not provided, guest init MUST fail with `secrets_missing`.

Secrets file MUST NOT be logged or included in diagnostics.

## Volume Mounts (Normative)

For each mount in config:

1. Ensure mountpoint directory exists (create if needed).
2. Mount device to mountpoint with specified filesystem.
3. Apply read-only flag if `mode` is "ro".

If any mount fails, guest init MUST fail with `mount_failed` and include the volume name in detail.

Reserved paths that MUST NOT be mount targets:
- `/proc`, `/sys`, `/dev`
- `/run/secrets` (platform-owned)
- `/tmp`, `/run` (tmpfs)

## Workload Launch (Normative)

After networking, volumes, and secrets are configured:

1. Set working directory to `workload.cwd`.
2. Set environment variables from `workload.env`.
3. Drop privileges to `workload.uid`/`workload.gid` if non-zero.
4. Exec `workload.argv[0]` with `workload.argv` as arguments.

Guest init remains PID 1 and:
- Reaps zombie processes
- Forwards SIGTERM, SIGINT, SIGHUP to workload
- Reports workload exit code to host agent

## Exec Service (v1)

Guest init MUST run an exec service on vsock port 5162.

The exec service:
- Accepts exec requests from host agent
- Spawns requested command with provided environment
- Allocates PTY if requested
- Streams stdin/stdout/stderr to host agent
- Reports exit status

Protocol details are in `docs/specs/runtime/exec-sessions.md`.

## Diagnostics

Guest init MUST write a boot log to:
- `/run/platform/guest-init.log` (size capped at 1MB)

Log format: JSON lines with timestamp, level, message.

Host agent MUST be able to fetch this log for `plfm instances diagnose`.

Boot log MUST NOT contain:
- Secret values
- Environment variable values (keys only)
- Full config (only config_version and generation)

## Security Notes

- Guest init is part of the trusted computing base. Keep dependencies minimal.
- Static linking preferred to reduce attack surface.
- No network access during guest init boot sequence (before workload starts).
- Busybox MAY be included for diagnostics if build flag enabled.

## Compliance Tests (Required)

1. Config handshake completes within 5 seconds.
2. Networking is correctly configured (address, route, MTU, DNS).
3. Secrets file exists with correct permissions.
4. Volume mounts succeed and are accessible.
5. Workload process starts with correct env and cwd.
6. Signal forwarding works (SIGTERM to workload).
7. Exit code is correctly reported to host agent.
8. Failed boot produces clear error with reason code.
9. Exec service accepts connections and spawns processes.
10. Boot log is written and contains expected entries.

## Open Questions (v2)

- Structured logging format standardization
- Readiness probe support inside guest
- Resource limit enforcement inside guest (cgroups v2)
- Init script hooks for customer customization
