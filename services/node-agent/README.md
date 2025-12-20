# Node Agent

The node agent runs on each bare-metal host and manages workload lifecycle.

## Responsibilities

- **VM Lifecycle**: Boot, monitor, and terminate Firecracker microVMs
- **Image Cache**: Fetch, verify, and cache OCI images (rootfs extraction)
- **Secrets Materialization**: Decrypt secret bundles and materialize to workload-accessible files
- **Volume Management**: Attach, mount, and manage local volumes with async backup
- **Network Setup**: Configure tap devices, WireGuard mesh, and overlay networking
- **Health Reporting**: Send heartbeats and instance status to control plane
- **Exec Sessions**: Proxy WebSocket exec connections into VMs via vsock

## Interfaces

### Owns
- vsock guest communication protocol (port 5161)
- Local volume lifecycle
- Image cache on disk

### Consumes
- Control plane API (desired state)
- Guest init handshake protocol
- WireGuard mesh configuration from control plane

## Directory Structure

```
services/node-agent/
├── Cargo.toml        # Crate manifest
├── README.md
├── config/           # Default and example configuration
│   └── example.toml
├── src/
│   ├── main.rs       # Binary entrypoint
│   ├── lib.rs        # Library root
│   ├── config.rs     # Configuration loading
│   ├── runtime.rs    # Runtime trait and selection
│   ├── vsock.rs      # Guest communication protocol
│   ├── exec.rs       # Exec session handling
│   ├── actors/       # Actor-based reconciliation
│   │   ├── mod.rs
│   │   ├── supervisor.rs
│   │   ├── instance.rs
│   │   └── image_pull.rs
│   ├── firecracker/  # Firecracker runtime implementation
│   ├── image/        # OCI image fetching and caching
│   ├── network/      # Tap device and overlay setup
│   └── state/        # Local state persistence
└── tests/            # Integration tests
```

## Running Locally

**Note**: Full runtime testing requires Linux with KVM support.

```bash
# On Linux with KVM
just dev-up
just build-node-agent
sudo ./target/release/node-agent --config config/dev.toml

# Check KVM availability
ls -l /dev/kvm
kvm-ok  # if available
```

On macOS, use the remote dev cluster for runtime testing.

## Testing

```bash
# Unit tests (work on any platform)
cargo test -p node-agent --lib

# Integration tests (require KVM)
cargo test -p node-agent --test '*'
```

## Configuration

Environment variables:
- `GHOST_CONTROL_PLANE_URL` - Control plane API URL
- `GHOST_NODE_ID` - Unique node identifier (auto-generated if not set)
- `GHOST_DATA_DIR` - Local data directory (default: `/var/lib/ghost`)
- `GHOST_IMAGE_CACHE_DIR` - Image cache location
- `GHOST_SECRETS_KEY` - Path to node-local secrets decryption key
- `GHOST_WIREGUARD_PRIVATE_KEY` - WireGuard private key path

See `config/example.toml` for full configuration options.

## Related Documentation

- [Architecture: Data Plane / Host Agent](../../docs/architecture/02-data-plane-host-agent.md)
- [Runtime Specs](../../docs/specs/runtime/)
- [Guest Init Protocol](../../docs/specs/runtime/guest-init.md)
- [Image Fetch and Cache](../../docs/specs/runtime/image-fetch-and-cache.md)
- [Volume Specs](../../docs/specs/storage/)
