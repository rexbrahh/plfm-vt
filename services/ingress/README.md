# Ingress

The ingress service handles external traffic routing into the platform.

## Responsibilities

- **L4 Load Balancing**: SNI-based routing without TLS termination
- **IPv6-First**: Native IPv6 by default; dedicated IPv4 as paid add-on
- **Proxy Protocol v2**: Preserve client IP through to workloads
- **Endpoint Registration**: Receive and apply endpoint mappings from control plane
- **Health Checks**: L4 TCP health checks to backend workloads
- **Connection Draining**: Graceful connection handling during deployments

## Interfaces

### Owns
- L4 ingress data path
- SNI routing table
- Proxy protocol injection

### Consumes
- Control plane endpoint updates (desired routing state)
- Node agent instance health status

## Directory Structure

```
services/ingress/
├── Cargo.toml        # Crate manifest
├── README.md
├── config/           # Default and example configuration
│   └── example.toml
├── src/
│   ├── main.rs       # Binary entrypoint
│   ├── lib.rs        # Library root
│   ├── config.rs     # Configuration loading
│   ├── sync.rs       # Control plane state synchronization
│   └── proxy/        # Proxy and routing implementation
│       ├── mod.rs
│       ├── router.rs # SNI-based routing logic
│       └── health.rs # Backend health checking
└── tests/            # Integration tests
```

## Running Locally

```bash
just dev-up
just build-ingress
./target/release/ingress --config config/dev.toml

# Test with a sample request
curl -v --resolve myapp.example.com:443:127.0.0.1 https://myapp.example.com/
```

## Testing

```bash
# Unit tests
cargo test -p ingress

# Integration tests
cargo test -p ingress --test '*'
```

## Configuration

Environment variables:
- `GHOST_CONTROL_PLANE_URL` - Control plane API URL for endpoint sync
- `GHOST_LISTEN_ADDR_IPV6` - IPv6 listen address (default: `[::]:443`)
- `GHOST_LISTEN_ADDR_IPV4` - IPv4 listen address (optional, for dedicated IPv4)
- `GHOST_HEALTH_CHECK_INTERVAL` - Backend health check interval (default: `5s`)
- `GHOST_LOG_LEVEL` - Log level (default: `info`)

See `config/example.toml` for full configuration options.

## Traffic Flow

```
Client → [DNS] → Ingress (L4, no TLS termination)
                    │
                    ├─ SNI: app1.example.com → Node A, Instance 1
                    ├─ SNI: app2.example.com → Node B, Instance 2
                    └─ SNI: app1.example.com → Node A, Instance 3 (replica)
```

Key behaviors:
1. TLS is **not** terminated at ingress (passthrough)
2. Proxy Protocol v2 header is prepended for client IP preservation
3. Workloads must handle TLS termination themselves
4. Health checks are L4 TCP only (no TLS handshake)

## Related Documentation

- [Architecture: Edge / Ingress / Egress](../../docs/architecture/03-edge-ingress-egress.md)
- [Ingress L4 Spec](../../docs/specs/networking/ingress-l4.md)
- [Proxy Protocol Spec](../../docs/specs/networking/proxy-protocol-v2.md)
- [ADR-0008: Ingress L4 SNI Passthrough First](../../docs/ADRs/0008-ingress-l4-sni-passthrough-first.md)
- [ADR-0009: Proxy Protocol v2 for Client IP](../../docs/ADRs/0009-proxy-protocol-v2-client-ip.md)
