# Node Enrollment (v1)

Status: approved  
Owner: Infrastructure Team  
Last reviewed: 2025-12-17

## Purpose

Define the security ceremony for adding a new host to the cluster.

This spec ensures:
- Control plane can authenticate the host agent (mTLS)
- Overlay (WireGuard) keys are registered and distributed safely
- Enrollment is auditable and hard to replay
- Compromised hosts can be revoked

## Goals

- Secure node identity establishment
- Auditable enrollment process
- Revocable node membership
- Simple operator workflow

## Non-goals (v1)

- Automated node provisioning (cloud API integration)
- Node re-enrollment without operator action
- Multi-region enrollment coordination

## Enrollment Token

### Properties (Normative)

- **Single-use**: Token is invalidated after successful enrollment.
- **Time-bounded**: Default expiry 1 hour, maximum 24 hours.
- **High entropy**: Minimum 256 bits, cryptographically random.
- **Scoped**: Bound to cluster ID (prevents cross-cluster confusion).
- **Audited**: Token creation recorded with operator identity.

### Token Format

```
enroll_<base64url-encoded-random-bytes>
```

Example:
```
enroll_dGhpcyBpcyBhIHRlc3QgdG9rZW4gd2l0aCBlbm91Z2ggZW50cm9weQ
```

Length: `enroll_` prefix + 43 characters (256 bits base64url encoded)

### Token Storage (Control Plane)

```json
{
  "token_id": "01JTOKEN",
  "token_hash": "sha256:abc123...",
  "cluster_id": "01JCLUSTER",
  "created_by": "user:alice@example.com",
  "created_at": "2025-12-17T12:00:00Z",
  "expires_at": "2025-12-17T13:00:00Z",
  "used": false,
  "used_at": null,
  "used_by_host_id": null
}
```

Token value is never stored; only hash is persisted.

## Enrollment Flow (Normative)

### Step 1: Operator Creates Token

```bash
plfm hosts enroll-token create --expires 1h
```

Output:
```
Enrollment token created.
Token: enroll_dGhpcyBpcyBhIHRlc3QgdG9rZW4gd2l0aCBlbm91Z2ggZW50cm9weQ
Expires: 2025-12-17T13:00:00Z

Use this token within 1 hour to enroll a new host.
This token can only be used once.
```

Control plane:
- Generates cryptographically random token
- Stores token_hash, not token value
- Records operator identity in audit log
- Emits `enrollment_token.created` event

### Step 2: Host Agent Starts in Unenrolled Mode

Agent starts with:
- No mTLS certificate
- No WireGuard configuration
- Enrollment token provided via:
  - Environment variable: `PLFM_ENROLLMENT_TOKEN`
  - Config file: `/etc/plfm-agent/enrollment-token`
  - Command line: `plfm-agent enroll --token <token>`

Agent generates locally:
- WireGuard keypair (stored in `/etc/plfm-agent/wireguard/`)
- mTLS keypair and CSR (stored in `/etc/plfm-agent/certs/`)

### Step 3: Agent Calls Enroll Endpoint

`POST /v1/hosts/enroll`

Request (over TLS, but without client cert):
```json
{
  "enrollment_token": "enroll_...",
  "underlay_endpoint": {
    "ipv6": "2001:db8::1",
    "ipv4": "203.0.113.1",
    "port": 51820
  },
  "hardware_facts": {
    "cpu_cores": 32,
    "memory_bytes": 137438953472,
    "disk_bytes": 1099511627776,
    "arch": "amd64"
  },
  "wireguard_public_key": "base64-encoded-public-key",
  "mtls_csr": "-----BEGIN CERTIFICATE REQUEST-----\n..."
}
```

### Step 4: Control Plane Validates

Validation checks (all must pass):
1. Token hash matches a valid, unused, unexpired token.
2. Token not previously used.
3. WireGuard public key not already enrolled.
4. CSR format is valid.
5. Underlay endpoint appears reachable (best-effort ping/connect).

On validation failure:
- Return error with reason
- If server-side validation failure, consume token
- If client-side failure (bad format), do not consume token

### Step 5: Control Plane Issues Credentials

Response:
```json
{
  "host_id": "01JHOST",
  "mtls_cert": "-----BEGIN CERTIFICATE-----\n...",
  "mtls_ca_chain": "-----BEGIN CERTIFICATE-----\n...",
  "overlay_config": {
    "host_overlay_ipv6": "fd00:plfm::1234",
    "listen_port": 51820,
    "mtu": 1420,
    "peers": [
      {
        "public_key": "base64...",
        "endpoint": "[2001:db8::2]:51820",
        "allowed_ips": ["fd00:plfm::5678/128"]
      }
    ]
  },
  "cluster_config": {
    "control_plane_endpoints": ["https://api.plfm.example.com"],
    "agent_version_policy": "auto"
  }
}
```

Control plane also:
- Marks token as used
- Creates host record with state `enrolled`
- Allocates overlay IPv6 from IPAM
- Emits `host.enrolled` event

### Step 6: Agent Persists and Applies Config

Agent:
1. Writes mTLS cert and key to disk (restricted permissions)
2. Applies WireGuard configuration
3. Brings up `wg0` interface
4. Starts normal operation with mTLS identity

If persistence fails:
- Agent reports failure
- Control plane can revoke enrollment
- Operator must re-enroll

### Step 7: Agent Confirms Enrollment

Agent calls (now with mTLS):
```
POST /v1/hosts/{host_id}/confirm-enrollment
```

Control plane:
- Verifies mTLS cert matches enrolled host
- Updates host state to `active`
- Distributes peer config to other nodes

## Failure Handling (Normative)

### Token Validation Failures

| Failure | Token Consumed? | Agent Action |
|---------|-----------------|--------------|
| Token expired | No | Retry with new token |
| Token already used | No | Retry with new token |
| Token invalid format | No | Fix token input |
| WireGuard key already enrolled | Yes | Investigate duplicate |
| CSR invalid | No | Regenerate CSR |
| Underlay unreachable | No | Check network |

### Partial Enrollment

If enrollment succeeds but agent fails to persist:
- Host remains in `enrolled` state (not `active`)
- Operator can revoke: `plfm hosts revoke <host_id>`
- Agent can retry enrollment with new token after revocation

### Stuck Enrollments

Detect: Host in `enrolled` state for > 10 minutes without confirmation.

Action:
- Alert operator
- Operator can force revoke or investigate

## Revocation

### Immediate Revocation

```bash
plfm hosts revoke <host_id> --reason "compromised"
```

Control plane:
1. Sets host state to `disabled`
2. Removes host from peer sets
3. Revokes mTLS certificate (adds to CRL or short-lived certs)
4. Emits `host.revoked` event

### Graceful Decommission

```bash
plfm hosts drain <host_id>
plfm hosts decommission <host_id>
```

Flow:
1. Drain: Stop new placements, migrate workloads
2. Decommission: Remove from cluster, release resources

## Audit Requirements

### Token Creation Event

```json
{
  "event_type": "enrollment_token.created",
  "token_id": "01JTOKEN",
  "created_by": "user:alice@example.com",
  "expires_at": "2025-12-17T13:00:00Z",
  "timestamp": "2025-12-17T12:00:00Z"
}
```

### Enrollment Event

```json
{
  "event_type": "host.enrolled",
  "host_id": "01JHOST",
  "token_id": "01JTOKEN",
  "enrolled_by": "system:enrollment-service",
  "underlay_endpoint": "2001:db8::1:51820",
  "wireguard_pubkey_fingerprint": "SHA256:abc123...",
  "mtls_cert_fingerprint": "SHA256:def456...",
  "timestamp": "2025-12-17T12:05:00Z"
}
```

### Revocation Event

```json
{
  "event_type": "host.revoked",
  "host_id": "01JHOST",
  "revoked_by": "user:alice@example.com",
  "reason": "compromised",
  "timestamp": "2025-12-17T15:00:00Z"
}
```

## Security Considerations

- Enrollment tokens are high-value secrets; treat like passwords.
- Tokens should be transmitted out-of-band (not in version control).
- mTLS certificates should be short-lived (90 days) with automated renewal.
- WireGuard private keys never leave the host.
- Operator identity for token creation should require MFA.

## CLI Commands

```bash
# Create enrollment token
plfm hosts enroll-token create [--expires <duration>]

# List pending tokens
plfm hosts enroll-token list

# Revoke unused token
plfm hosts enroll-token revoke <token_id>

# List hosts
plfm hosts list

# Show host details
plfm hosts show <host_id>

# Drain host
plfm hosts drain <host_id>

# Decommission host
plfm hosts decommission <host_id>

# Emergency revoke
plfm hosts revoke <host_id> --reason <reason>
```

## Compliance Tests (Required)

1. Valid token enrollment succeeds.
2. Expired token is rejected.
3. Reused token is rejected.
4. Invalid CSR is rejected without consuming token.
5. Duplicate WireGuard key is rejected.
6. Revoked host is removed from peer sets within 60 seconds.
7. Audit log contains all required fields.
8. mTLS communication works after enrollment.
9. Partial enrollment can be recovered.
10. Token creation requires authenticated operator.

## Open Questions (v2)

- Automated provisioning integration (Terraform, cloud-init)
- Certificate rotation automation
- Geo-distributed enrollment (regional control planes)
- Hardware attestation (TPM-based identity)
