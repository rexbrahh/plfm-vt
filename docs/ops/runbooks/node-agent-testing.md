# Node Agent Testing Runbook

Status: reviewed
Owner: Team Runtime
Last reviewed: 2025-12-25

## Overview

The node-agent manages VM lifecycle on bare-metal hosts. Testing requires different strategies depending on the environment:

- **macOS/CI**: Mock runtime (no KVM)
- **Linux with KVM**: Full Firecracker runtime

## Test Layers

### Unit Tests (Any Platform)

Run on any platform without special requirements:

```bash
cargo test -p plfm-node-agent --lib
```

Tests include:
- Status transition logic (`instance::tests`)
- Reconciler configuration
- Image parsing and caching logic
- Network tap configuration
- Actor framework behavior

### Integration Tests (Any Platform)

Uses `MockRuntime` to simulate VM lifecycle without actual Firecracker:

```bash
cargo test -p plfm-node-agent --test reconciliation
cargo test -p plfm-node-agent --test m3_status_reporting
```

Key scenarios:
- Supervisor lifecycle management
- Instance scaling up/down
- Status transition reporting (only on state changes)
- Failure reason code propagation

### macOS Development

macOS lacks KVM support. Use the wrapper script for libiconv compatibility:

```bash
./scripts/dev/with-macos-libiconv.sh cargo test -p plfm-node-agent
./scripts/dev/with-macos-libiconv.sh cargo build -p plfm-node-agent
```

All tests pass on macOS using the mock runtime.

## Linux Smoke Test (Requires KVM)

### Prerequisites

1. Linux host with KVM support:
```bash
ls -l /dev/kvm
# If available: crw-rw---- 1 root kvm 10, 232 ...
```

2. Firecracker binary in PATH or configured location

3. Root disk image (extracted from OCI image)

### Running with Real Runtime

```bash
# Build release binary
cargo build -p plfm-node-agent --release

# Run with real Firecracker (requires root or kvm group)
sudo ./target/release/plfm-node-agent --config config/dev.toml
```

### Smoke Test Checklist

1. **VM Boot**: Instance transitions Booting → Ready
2. **Health Check**: Periodic health checks pass
3. **Status Reporting**: Status reported only on transitions
4. **Graceful Stop**: Instance transitions Ready → Draining → Stopped
5. **Failure Handling**: Failed VMs report appropriate `reason_code`

### Expected Failure Reason Codes

When instances fail, the `reason_code` field should be one of:

| Code | Trigger |
|------|---------|
| `image_pull_failed` | OCI image fetch error |
| `rootfs_build_failed` | Root disk extraction error |
| `firecracker_start_failed` | VM boot failure |
| `network_setup_failed` | Tap device or overlay error |
| `volume_attach_failed` | Volume mount error |
| `secrets_missing` | Required secrets not configured |
| `secrets_injection_failed` | Secrets fetch/decrypt error |
| `healthcheck_failed` | Health check failure after Ready |
| `oom_killed` | Memory limit exceeded |
| `crash_loop_backoff` | Repeated crash restarts |
| `terminated_by_operator` | Manual kill |
| `node_draining` | Node entering drain state |

## CI Configuration

CI runs mock-gated tests only (no KVM):

```yaml
# .github/workflows/rust.yml
- name: Test node-agent
  run: cargo test -p plfm-node-agent
```

Linux KVM tests require a dedicated runner with `/dev/kvm` access.

## Troubleshooting

### Mock Runtime Tests Fail

1. Check for compilation errors: `cargo check -p plfm-node-agent`
2. Run with verbose output: `cargo test -p plfm-node-agent -- --nocapture`

### Firecracker Won't Start

1. Verify KVM access: `ls -l /dev/kvm`
2. Check group membership: `groups | grep kvm`
3. Verify Firecracker binary: `firecracker --version`

### Status Not Reported

Status is only reported on transitions. If status doesn't change between reconcile ticks, no report is sent. This is intentional to reduce control-plane load.

## Related Documentation

- [Architecture: Data Plane / Host Agent](../../architecture/02-data-plane-host-agent.md)
- [Runtime Specs](../../specs/runtime/)
- [Testing Strategy](../../engineering/testing-strategy.md)
