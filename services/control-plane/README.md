# Control Plane

The control plane is the central coordination service for the plfm-vt platform.

## Responsibilities

- **API Server**: Exposes the REST API for all platform operations (organizations, projects, apps, environments, releases, workloads)
- **Authentication & Authorization**: Validates tokens, enforces scopes, manages tenant isolation
- **Reconciliation Loop**: Drives desired state → current state convergence
- **Scheduler**: Assigns workloads to nodes based on resource availability and constraints
- **Event Log**: Writes all state mutations to the append-only event log
- **Secret Management**: Encrypts and stores secret bundles (never decrypts; node-agent does)

## Interfaces

### Owns
- REST API (see `api/openapi/openapi.yaml`)
- Event log schema (see `api/schemas/event-envelope.json`)
- Manifest validation (see `api/schemas/manifest.json`)

### Consumes
- PostgreSQL for state storage
- Node-agent heartbeat/status updates
- Ingress registration events

## Directory Structure

```
services/control-plane/
├── Cargo.toml        # Crate manifest
├── README.md
├── config/           # Default and example configuration
│   └── example.toml
├── migrations/       # SQL migrations
├── src/
│   ├── main.rs       # Binary entrypoint
│   ├── lib.rs        # Library root
│   ├── config.rs     # Configuration loading
│   ├── state.rs      # Application state
│   ├── api/          # HTTP handlers
│   │   └── v1/       # v1 API endpoints
│   ├── db/           # Database access
│   │   ├── event_store.rs
│   │   ├── idempotency.rs
│   │   └── projections.rs
│   ├── projections/  # Materialized view workers
│   └── scheduler/    # Workload placement logic
└── tests/            # Integration tests
    └── core_loop.rs
```

## Running Locally

```bash
# Start dependencies (postgres, etc.)
just dev-up

# Run control-plane
just build-control-plane
./target/release/control-plane --config config/dev.toml

# Or with hot reload
cargo watch -x 'run -p control-plane'
```

## Testing

```bash
# Unit tests
cargo test -p control-plane

# Integration tests (requires dev stack)
cargo test -p control-plane --test '*'
```

## Configuration

Environment variables:
- `GHOST_DB_URL` - PostgreSQL connection string
- `GHOST_LISTEN_ADDR` - HTTP listen address (default: `127.0.0.1:8080`)
- `GHOST_LOG_LEVEL` - Log level (default: `info`)
- `GHOST_SECRETS_KEY` - Path to secrets encryption key

See `config/example.toml` for full configuration options.

## Related Documentation

- [Architecture: Control Plane](../../docs/architecture/01-control-plane.md)
- [API Spec](../../docs/specs/api/)
- [State Model](../../docs/specs/state/)
- [Scheduler Spec](../../docs/specs/scheduler/)
