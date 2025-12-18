# Adversarial Documentation Review: plfm-vt

**Review Date:** 2025-12-17  
**Reviewer:** External Technical Reviewer  
**Scope:** Full documentation suite for CLI-first PaaS (Firecracker microVMs on bare metal)  
**Documents Reviewed:** 13 ADRs, 10 architecture docs, 8 CLI docs, 30+ specs, 7 security docs, 9 frontend docs, ops runbooks

---

## 1. Executive Summary

### Recommendation: **GO WITH CONDITIONS**

The documentation suite demonstrates exceptional thoroughness and architectural coherence. The team has made sound fundamental decisions and documented them with unusual rigor. However, several gaps must be addressed before implementation begins to avoid costly rework.

### Top 5 Critical Risks

| # | Risk | Impact | Likelihood | Mitigation Priority |
|---|------|--------|------------|-------------------|
| 1 | **Exec mechanism underspecified** | High: Security-critical feature with no implementation spec | High | P0 - Block implementation |
| 2 | **Guest init delivery mechanism unclear** | High: Boot pipeline depends on unspecified strategy | High | P0 - Block implementation |
| 3 | **Node enrollment ceremony incomplete** | High: Security foundation with procedural gaps | Medium | P0 - Complete before first host |
| 4 | **Master key operational procedures missing** | High: Secrets system depends on undocumented ceremony | Medium | P1 - Complete before secrets work |
| 5 | **Volume migration path for node failures** | Medium: Data loss risk for stateful workloads | Medium | P1 - Design before volume GA |

### Summary Assessment

**Strengths:**
- ADR process is rigorous with explicit open questions and resolutions
- Event-sourcing model is well-designed with clear ordering guarantees
- Security threat model is comprehensive and actionable
- CLI principles document is excellent (idempotency, machine-readable output)
- Non-goals document prevents scope creep
- Workload spec contract is detailed and versioned

**Critical Gaps:**
- Exec implementation needs a dedicated spec
- Guest init binary/initramfs delivery strategy unspecified
- Several "v1 recommendation" items lack acceptance criteria
- Frontend/CLI consistency not enforced
- Operational runbooks reference procedures that don't exist

---

## 2. Decision Interrogation Matrix

### ADR-0001: Isolation via MicroVM per Environment

| Aspect | Analysis |
|--------|----------|
| **Statement** | One Firecracker microVM per instance (not per environment) |
| **Benefit** | Strong isolation, deterministic resource accounting, crash isolation |
| **Hidden Costs** | Boot latency (~150-300ms), memory overhead per VM (~64MB), image-to-rootfs conversion complexity |
| **Alternatives Rejected** | Containers (weaker isolation), gVisor (complexity), Kata (heavier) |
| **Missing Evidence** | No benchmark data for boot latency targets; no specification of acceptable cold-start time |
| **Validation Plan** | Measure P99 boot latency under load; validate memory overhead assumptions |

### ADR-0002: Artifact is OCI Image + Manifest

| Aspect | Analysis |
|--------|----------|
| **Statement** | Deploy OCI images; platform manifest defines process types |
| **Benefit** | Industry-standard artifact format, portable, existing tooling |
| **Hidden Costs** | Image-to-ext4 conversion pipeline needed; layer unpacking security concerns |
| **Alternatives Rejected** | Custom artifact format, VM images |
| **Missing Evidence** | No specification of image size limits; no policy on base image requirements |
| **Validation Plan** | Test image conversion with pathological tar archives; fuzz layer unpacking |

### ADR-0003: Runtime is Firecracker

| Aspect | Analysis |
|--------|----------|
| **Statement** | Use Firecracker for microVM isolation |
| **Benefit** | Fast boot, minimal attack surface, proven at scale (AWS Lambda) |
| **Hidden Costs** | Limited device support, no GPU, vsock complexity for exec |
| **Alternatives Rejected** | QEMU (too heavy), Cloud Hypervisor (less mature) |
| **Missing Evidence** | No kernel compatibility matrix; no specification of required kernel config |
| **Validation Plan** | Validate on target kernel versions; test edge cases (large memory, many CPUs) |

### ADR-0004: Overlay is WireGuard Full Mesh

| Aspect | Analysis |
|--------|----------|
| **Statement** | WireGuard full mesh between all nodes |
| **Benefit** | Simple, fast, cryptographically strong |
| **Hidden Costs** | O(n²) peer entries; config distribution latency; key rotation complexity |
| **Alternatives Rejected** | IPsec (complexity), no encryption (unacceptable) |
| **Missing Evidence** | No scale limit specified (when does full mesh break?); no latency budget for config propagation |
| **Validation Plan** | Test with 100+ nodes; measure config propagation time; validate key rotation under load |

### ADR-0005: State is Event Log + Materialized Views

| Aspect | Analysis |
|--------|----------|
| **Statement** | Event-sourced state with derived views |
| **Benefit** | Auditability, temporal queries, rebuild capability |
| **Hidden Costs** | Projection lag for read-your-writes; event schema evolution complexity; storage growth |
| **Alternatives Rejected** | Traditional CRUD (loses history), pure event sourcing without views (query complexity) |
| **Missing Evidence** | No specification of acceptable projection lag; no event retention policy |
| **Validation Plan** | Measure projection lag under write bursts; test full rebuild from event log |

### ADR-0006: Control Plane DB is Postgres

| Aspect | Analysis |
|--------|----------|
| **Statement** | Single Postgres instance for control plane state |
| **Benefit** | Mature, reliable, supports both event log and views |
| **Hidden Costs** | Single point of failure; operational complexity for HA; scaling limits |
| **Alternatives Rejected** | CockroachDB (complexity), SQLite (scaling) |
| **Missing Evidence** | No HA strategy specified in ADR; no connection pooling strategy |
| **Validation Plan** | Test failover scenarios; validate connection handling under load |

### ADR-0007: Network is IPv6-First, IPv4 Paid Add-on

| Aspect | Analysis |
|--------|----------|
| **Statement** | Default to IPv6; IPv4 requires explicit paid allocation |
| **Benefit** | Address space sustainability, simpler networking, differentiator |
| **Hidden Costs** | Customer education; some integrations assume IPv4; dual-stack complexity at edge |
| **Alternatives Rejected** | IPv4-first (unsustainable), dual-stack default (complexity) |
| **Missing Evidence** | No specification of IPv4 pool management; no pricing model defined |
| **Validation Plan** | Validate common frameworks work with IPv6-only; test IPv4 add-on workflow |

### ADR-0008: Ingress is L4 SNI Passthrough First

| Aspect | Analysis |
|--------|----------|
| **Statement** | L4 TCP proxy with SNI inspection for routing; no TLS termination |
| **Benefit** | Simplicity, end-to-end encryption, customer certificate control |
| **Hidden Costs** | No HTTP features (redirects, headers); SNI inspection has edge cases (ECH) |
| **Alternatives Rejected** | L7 termination (complexity, certificate management) |
| **Missing Evidence** | No specification of SNI parsing library; no ECH handling policy |
| **Validation Plan** | Test with various TLS versions; validate SNI extraction edge cases |

### ADR-0009: PROXY Protocol v2 for Client IP

| Aspect | Analysis |
|--------|----------|
| **Statement** | Optional PROXY protocol v2 header injection |
| **Benefit** | Preserves true client IP for logging and rate limiting |
| **Hidden Costs** | Backend must support it; misconfiguration breaks connections |
| **Alternatives Rejected** | X-Forwarded-For (L7 only), no client IP (unacceptable for some apps) |
| **Missing Evidence** | No validation of backend support at route creation |
| **Validation Plan** | Test with common backend stacks; validate misconfiguration detection |

### ADR-0010: Secrets Delivered as Fixed-Format File

| Aspect | Analysis |
|--------|----------|
| **Statement** | Secrets delivered as `/run/secrets/platform.env` file |
| **Benefit** | Simple, universal, no SDK required |
| **Hidden Costs** | No hot reload; file permissions complexity; format evolution |
| **Alternatives Rejected** | HTTP metadata service (complexity), environment injection (visibility) |
| **Missing Evidence** | No specification of max secret bundle size; no encoding limits |
| **Validation Plan** | Test with large secret bundles; validate permission enforcement |

### ADR-0011: Storage is Local Volumes + Async Backups

| Aspect | Analysis |
|--------|----------|
| **Statement** | Volumes are local to nodes; backups are async to object storage |
| **Benefit** | Performance, simplicity, cost efficiency |
| **Hidden Costs** | No live migration; node failure = downtime; volume-scheduler coupling |
| **Alternatives Rejected** | Network storage (latency, complexity), synchronous replication (cost) |
| **Missing Evidence** | **No volume migration procedure for node failures**; no backup RTO/RPO targets |
| **Validation Plan** | Test restore workflow end-to-end; validate backup integrity |

### ADR-0012: CPU Oversubscribe, Memory Hard Cap

| Aspect | Analysis |
|--------|----------|
| **Statement** | CPU is soft (oversubscribable), memory is hard-capped |
| **Benefit** | Better utilization, prevents OOM cascade |
| **Hidden Costs** | CPU contention unpredictable; noisy neighbor for CPU-bound workloads |
| **Alternatives Rejected** | Hard CPU limits (waste), soft memory (OOM chaos) |
| **Missing Evidence** | No specification of overcommit ratio; no CPU weight algorithm |
| **Validation Plan** | Load test with CPU contention; measure latency variance |

### ADR-0013: NixOS as Host OS

| Aspect | Analysis |
|--------|----------|
| **Statement** | Use NixOS for reproducible host configuration |
| **Benefit** | Declarative config, atomic upgrades, reproducibility |
| **Hidden Costs** | Learning curve, smaller talent pool, some software compatibility issues |
| **Alternatives Rejected** | Ubuntu (config drift), CoreOS (less flexibility) |
| **Missing Evidence** | No host configuration specification; no upgrade procedure |
| **Validation Plan** | Validate all required packages available; test upgrade rollback |

---

## 3. Cross-Document Inconsistencies

### Inconsistency 1: Exec Session Delivery Mechanism

**Files:** `docs/architecture/02-data-plane-host-agent.md`, `docs/specs/state/materialized-views.md`, `docs/frontend/01-terminal-protocol.md`

**Conflict:** 
- Architecture doc mentions exec sessions exist
- Materialized views spec defines `exec_sessions_view` with events
- Frontend docs describe terminal protocol for exec
- **No spec exists defining how exec connections are established, authenticated, or proxied**

**Impact:** Cannot implement exec without designing the full flow

**Resolution Required:** Create `docs/specs/runtime/exec-sessions.md`

---

### Inconsistency 2: Guest Init Delivery Strategy

**Files:** `docs/specs/runtime/firecracker-boot.md:145-147`, `docs/specs/runtime/image-fetch-and-cache.md:145-147`

**Conflict:**
- firecracker-boot.md: "guest init is provided via a small initramfs or separate mechanism, not by mutating user images"
- image-fetch-and-cache.md: same statement verbatim
- **Neither spec actually defines the initramfs build or delivery mechanism**

**Impact:** Boot pipeline cannot be implemented without this decision

**Resolution Required:** Add section to `firecracker-boot.md` specifying initramfs strategy

---

### Inconsistency 3: Config Handshake Message Schema

**Files:** `docs/specs/runtime/firecracker-boot.md:216-218`

**Conflict:**
- Spec states: "The exact message schema is implementation-defined but must include a version field"
- This is a contract between guest init and host agent
- **Implementation-defined contracts cause version skew bugs**

**Impact:** Agent and guest init may diverge without clear schema

**Resolution Required:** Define explicit JSON schema in spec or reference a protobuf definition

---

### Inconsistency 4: Event Types vs Reason Codes

**Files:** `docs/specs/manifest/workload-spec.md`, `docs/specs/state/event-types.md`

**Conflict:**
- workload-spec.md defines 12 failure reason codes (e.g., `image_pull_failed`, `oom_killed`)
- States: "These reason codes must map to event types"
- **event-types.md was not provided for review; mapping may not exist**

**Impact:** Inconsistent error reporting between agent and control plane

**Resolution Required:** Verify event-types.md contains all reason codes

---

### Inconsistency 5: Scale Limits for Volumes

**Files:** `docs/specs/scheduler/placement.md:126-129`, `docs/specs/manifest/manifest-schema.md`

**Conflict:**
- placement.md: "max replicas = 1 for any process type with volumes in v1"
- **Manifest schema should enforce this at validation time, not just scheduler**
- No validation rule in manifest-schema.md

**Impact:** Users can declare replicas > 1 with volumes, causing confusing scheduler failures

**Resolution Required:** Add validation rule to manifest spec

---

### Inconsistency 6: IPv4 Add-on Workflow

**Files:** `docs/specs/networking/ipam.md:137-145`, `docs/specs/networking/ingress-l4.md:135-140`

**Conflict:**
- ipam.md references `docs/specs/networking/ipv4-addon.md` for "full product behavior"
- **ipv4-addon.md was not provided for review**
- ingress-l4.md assumes IPv4 add-on exists but doesn't define enablement flow

**Impact:** Cannot implement IPv4 add-on without the spec

**Resolution Required:** Verify ipv4-addon.md exists and is complete

---

### Inconsistency 7: Overlay Address Allocation Timing

**Files:** `docs/specs/networking/ipam.md:104`, `docs/specs/scheduler/placement.md:223-228`

**Conflict:**
- ipam.md: "Allocate at instance allocation time (scheduler emits instance.allocated event including overlay_ipv6)"
- placement.md: "Allocate overlay_ipv6 from IPAM" as step 5 of placement algorithm
- **Both are correct but neither specifies failure handling if IPAM exhausted mid-placement**

**Impact:** Partial placement failures may leave orphaned resources

**Resolution Required:** Define rollback semantics for IPAM allocation failures

---

### Inconsistency 8: Default Replicas

**Files:** `docs/specs/state/materialized-views.md:287-290`, `docs/specs/manifest/manifest-schema.md`

**Conflict:**
- materialized-views.md: "If a process type has no scale entry, desired defaults to manifest-derived default"
- **manifest-schema.md doesn't specify what the default is**

**Impact:** Ambiguous behavior for unscaled process types

**Resolution Required:** Explicitly define default replica count (likely 1)

---

## 4. Missing Specs That Will Cause Rework

### Critical Missing Specs (P0)

#### 4.1 Exec Sessions Specification
**Impact:** Cannot implement `plfm exec` command or web terminal exec without this
**Required Contents:**
- Authentication flow (short-lived token, scopes required)
- Connection establishment (WebSocket? vsock proxy?)
- Audit requirements
- Timeout and cleanup semantics
- PTY vs non-PTY modes
- Signal forwarding

**Suggested File:** `docs/specs/runtime/exec-sessions.md`

---

#### 4.2 Guest Init Binary Specification
**Impact:** Cannot build boot pipeline without knowing how init reaches the VM
**Required Contents:**
- Initramfs vs separate disk vs embedded in kernel
- Build process and versioning
- Update/rollback strategy
- Required capabilities and syscalls
- Error handling and diagnostics

**Suggested File:** `docs/specs/runtime/guest-init.md`

---

#### 4.3 Node Enrollment Procedure
**Impact:** Cannot securely add hosts to the cluster
**Required Contents:**
- Enrollment token generation and lifecycle
- Certificate signing ceremony
- WireGuard key exchange security
- Operator identity verification
- Rollback on failed enrollment

**Suggested Location:** Expand `docs/specs/networking/overlay-wireguard.md` section on enrollment

---

### High Priority Missing Specs (P1)

#### 4.4 Secrets Master Key Operations
**Impact:** Cannot operate secrets system securely
**Required Contents:**
- Key generation ceremony
- Key storage (HSM? SOPS? age?)
- Key rotation procedure
- Recovery procedures
- Audit requirements

**Suggested File:** `docs/ops/runbooks/secrets-key-ceremony.md`

---

#### 4.5 Volume Migration/Recovery Procedure
**Impact:** Data loss risk when nodes fail
**Required Contents:**
- Detection of node failure with volumes
- Backup-based recovery workflow
- RPO/RTO guarantees
- Operator tooling
- Customer notification requirements

**Suggested File:** `docs/ops/runbooks/volume-node-failure.md`

---

#### 4.6 Image Size and Timeout Limits
**Impact:** Unbounded image pulls can cause cascading failures
**Required Contents:**
- Maximum image size (compressed and uncompressed)
- Pull timeout per layer
- Total pull timeout
- Disk space reservation strategy

**Suggested Location:** Add to `docs/specs/runtime/image-fetch-and-cache.md`

---

### Medium Priority Missing Specs (P2)

#### 4.7 API Rate Limiting Specification
**Impact:** Abuse potential without documented limits
**Required Contents:**
- Per-org, per-token limits
- Limit categories (read, write, exec)
- Response headers
- Retry-after behavior

**Suggested Location:** Add to `docs/specs/api/http-api.md`

---

#### 4.8 Log Retention and Access Policy
**Impact:** Compliance and cost concerns
**Required Contents:**
- Retention periods by log type
- Access controls
- Export capabilities
- Sensitive data handling

**Suggested File:** `docs/specs/observability/log-retention.md`

---

#### 4.9 CLI to TUI Feature Parity Matrix
**Impact:** User confusion about capability differences
**Required Contents:**
- Feature comparison table
- Intentional differences documented
- Deprecation policy

**Suggested Location:** Add to `docs/cli/07-TUI-workbench-v1.md`

---

## 5. Implementation Risk Register

| # | Risk | Severity | Likelihood | Detection | Mitigation | Owner |
|---|------|----------|------------|-----------|------------|-------|
| 1 | **Exec session security flaws** | Critical | High | Late (after exploit) | Create detailed spec with security review before implementation | Security + Runtime |
| 2 | **Guest init version skew** | High | Medium | Medium (boot failures) | Define versioning contract; implement compatibility tests | Runtime |
| 3 | **IPAM exhaustion cascade** | High | Medium | Low (gradual degradation) | Implement pool monitoring; define allocation failure semantics | Networking |
| 4 | **Image pull timeout causing scheduler starvation** | High | Medium | Medium (slow deploys) | Define bounded timeouts; implement circuit breaker | Runtime |
| 5 | **WireGuard key rotation partition** | High | Low | Low (silent network failures) | Implement staged rollout; add mesh health checks | Networking |
| 6 | **Event log growth unbounded** | Medium | High | Medium (disk alerts) | Define retention policy; implement compaction | State |
| 7 | **Projection lag causes stale reads** | Medium | High | Low (consistency bugs) | Define read-your-writes endpoints; add lag metrics | API |
| 8 | **Volume locality scheduler deadlock** | Medium | Medium | Medium (stuck deploys) | Validate attachment compatibility at creation time | Scheduler |
| 9 | **SNI inspection bypassed by ECH** | Medium | Low | Medium (routing failures) | Document ECH behavior; implement fallback policy | Edge |
| 10 | **Secret bundle size causes boot timeout** | Medium | Low | Medium (slow starts) | Define size limits; test with large bundles | Secrets |

---

## 6. Recommended Document Edits

### docs/specs/runtime/firecracker-boot.md

**Line 145-147:** Replace vague statement with concrete decision:
```diff
- If you need platform files (like guest init), they should not be copied into the root disk in v1.
- - v1 recommendation: guest init is provided via a small initramfs or separate mechanism, not by mutating user images.
+ Platform files (guest init) delivery:
+ - v1 decision: guest init is embedded in a minimal initramfs bundled with the kernel
+ - Initramfs contains: guest init binary, busybox for diagnostics, required kernel modules
+ - Initramfs is versioned alongside kernel: `kernel-{version}-initramfs-{version}.img`
+ - See `docs/specs/runtime/guest-init.md` for init binary specification
```

**Line 216-218:** Add concrete schema:
```diff
- The exact message schema is implementation-defined but must include a version field:
+ The config handshake uses JSON with this schema:
+ ```json
+ {
+   "config_version": "v1",
+   "instance_id": "string (ULID)",
+   "generation": "integer",
+   "command": ["string"],
+   "env_vars": {"key": "value"},
+   "workdir": "string (optional)",
+   "network": {
+     "overlay_ipv6": "string",
+     "gateway_ipv6": "string", 
+     "mtu": "integer",
+     "dns": ["string"]
+   },
+   "mounts": [...],
+   "secrets": {"required": "boolean", "secret_version_id": "string"}
+ }
+ ```
```

---

### docs/specs/networking/overlay-wireguard.md

**After line 142:** Add enrollment security requirements:
```markdown
### Enrollment security requirements (normative)

Token properties:
- Single-use: token is invalidated after successful enrollment
- Time-bounded: default expiry 1 hour, max 24 hours
- Entropy: minimum 256 bits, cryptographically random
- Format: base64url-encoded, prefixed with `enroll_`

Enrollment validation:
- Control plane MUST verify token is unused and unexpired
- Control plane MUST verify underlay endpoint is reachable
- Control plane MUST NOT accept enrollment from previously-enrolled node_ids
- Operator identity MUST be recorded in audit log

Failure handling:
- Failed enrollment MUST NOT consume the token if failure is client-side
- Failed enrollment MUST consume the token if failure is server-side validation
- Node MUST NOT persist any credentials on enrollment failure
```

---

### docs/specs/scheduler/placement.md

**Line 126-129:** Strengthen validation requirement:
```diff
  v1 recommendation:
- - enforce: if a process type has any volume attachments and any attachment is read_write, then max replicas for that process type is 1.
+ - enforce: if a process type has any volume attachments, max replicas = 1
+ - this constraint MUST be enforced at manifest validation time, not only at scheduling time
+ - manifest validation error: `E_VOLUME_REPLICA_LIMIT: process type '{name}' has volume mounts and cannot have replicas > 1`
```

---

### docs/specs/networking/ipam.md

**After line 178:** Add failure semantics:
```markdown
### IPAM allocation failure handling

Scheduler behavior on IPAM exhaustion:
- Scheduler MUST NOT partially create instance if IPAM fails
- Scheduler MUST emit `instance.allocation_failed` event with reason `ipam_exhausted`
- Scheduler MUST mark env as degraded with clear reason
- Scheduler MUST retry allocation on next reconciliation cycle

Pool exhaustion alerting:
- Alert when pool utilization exceeds 80%
- Alert when allocation failures occur
- Dashboard MUST show pool utilization by category (node, instance, ipv4)
```

---

### docs/specs/runtime/image-fetch-and-cache.md

**After line 60:** Add limits:
```markdown
### Image size and timeout limits (v1)

Hard limits:
- Maximum compressed image size: 10 GiB
- Maximum uncompressed rootfs size: 50 GiB
- Per-layer pull timeout: 5 minutes
- Total image pull timeout: 30 minutes

Validation:
- Agent MUST check manifest size before pulling layers
- Agent MUST fail fast with `image_too_large` if limits exceeded
- Agent MUST surface clear error to control plane

Operator overrides:
- Limits are configurable per cluster via agent config
- Higher limits require explicit operator action
```

---

### docs/specs/api/http-api.md

**Add new section:** Rate Limiting
```markdown
## Rate Limiting

### Default limits (v1)

Per-org limits:
- Read operations: 1000 requests/minute
- Write operations: 100 requests/minute  
- Exec sessions: 10 concurrent per env

Per-token limits:
- Service principals inherit org limits by default
- Scoped tokens may have lower limits

### Response headers

All responses include:
- `X-RateLimit-Limit`: requests allowed in window
- `X-RateLimit-Remaining`: requests remaining
- `X-RateLimit-Reset`: Unix timestamp when window resets

### 429 response

When rate limited:
- Status: 429 Too Many Requests
- Header: `Retry-After: <seconds>`
- Body: `{"error": "rate_limited", "retry_after": <seconds>}`
```

---

### docs/architecture/02-data-plane-host-agent.md

**Add new section for exec:**
```markdown
## Exec Session Handling

Exec sessions allow interactive shell access to running instances.

### Connection flow

1. User requests exec via API with target instance_id and command
2. Control plane validates authorization and instance existence
3. Control plane creates short-lived exec token and records `exec_session.granted`
4. Control plane returns WebSocket upgrade URL with token
5. User connects WebSocket to edge
6. Edge validates token and proxies to target host agent
7. Host agent establishes vsock connection to guest
8. Guest init spawns requested command with PTY
9. Bidirectional stream bridges WebSocket to PTY

### Security requirements

- Exec tokens expire in 60 seconds
- Tokens are single-use
- All exec sessions are audit logged
- Session duration limited to 1 hour default
- No exec to instances in draining/stopped state

See `docs/specs/runtime/exec-sessions.md` for full specification.
```

---

## 7. Experiments and Milestones

### Phase 1: Foundation Validation (Weeks 1-2)

| Experiment | Success Criteria | Blocking? |
|------------|------------------|-----------|
| **E1: Firecracker boot time** | P99 cold boot < 500ms with overlay rootfs | Yes |
| **E2: Image-to-rootfs pipeline** | Convert 5GB compressed image in < 60s; survive malicious tar | Yes |
| **E3: WireGuard mesh at scale** | 50 nodes, config propagation < 5s, no handshake failures | Yes |
| **E4: Guest init vsock handshake** | Config delivered and applied in < 100ms | Yes |

### Phase 2: Control Plane Validation (Weeks 3-4)

| Experiment | Success Criteria | Blocking? |
|------------|------------------|-----------|
| **E5: Event log write throughput** | 10k events/sec sustained; no sequence gaps | Yes |
| **E6: Projection lag** | P99 lag < 500ms under 1k events/sec | Yes |
| **E7: Scheduler placement speed** | 100 instances placed in < 1s | No |
| **E8: Read-your-writes consistency** | API returns created resource within 1s | Yes |

### Phase 3: Edge Validation (Weeks 5-6)

| Experiment | Success Criteria | Blocking? |
|------------|------------------|-----------|
| **E9: SNI extraction reliability** | 99.9% success rate with TLS 1.2/1.3 clients | Yes |
| **E10: Backend health gating** | Unhealthy backend removed from rotation < 10s | Yes |
| **E11: PROXY protocol injection** | Client IP correctly propagated; no corruption | Yes |
| **E12: Connection handling under load** | 10k concurrent connections; no resource leak | No |

### Phase 4: Security Validation (Weeks 7-8)

| Experiment | Success Criteria | Blocking? |
|------------|------------------|-----------|
| **E13: Exec session auth** | Unauthorized access returns 403; audit log complete | Yes |
| **E14: Cross-tenant isolation** | No network path between tenant VMs without explicit route | Yes |
| **E15: Secret injection security** | Secrets not in logs; correct permissions; no race conditions | Yes |
| **E16: Node enrollment security** | Replay attack fails; token reuse fails | Yes |

### Phase 5: Operational Validation (Weeks 9-10)

| Experiment | Success Criteria | Blocking? |
|------------|------------------|-----------|
| **E17: Node drain and reschedule** | Stateless workloads migrate without downtime | Yes |
| **E18: Volume backup/restore** | Restore completes within RTO; data integrity verified | Yes |
| **E19: Key rotation** | WireGuard key rotation with zero connection drops | No |
| **E20: Full event log replay** | Views rebuilt correctly from scratch | Yes |

### Milestone Gates

| Milestone | Required Experiments | Go/No-Go Decision |
|-----------|---------------------|-------------------|
| **M1: Runtime Ready** | E1-E4 pass | Can deploy first workload |
| **M2: Control Plane Ready** | E5-E8 pass | Can manage workloads at scale |
| **M3: Edge Ready** | E9-E12 pass | Can route external traffic |
| **M4: Security Ready** | E13-E16 pass | Can onboard first customer |
| **M5: Operations Ready** | E17-E20 pass | Can run in production |

---

## Appendix A: Documents Reviewed

### ADRs
- 0001-isolation-microvm-per-env.md
- 0002-artifact-oci-image-plus-manifest.md
- 0003-runtime-firecracker.md
- 0004-overlay-wireguard-full-mesh.md
- 0005-state-event-log-plus-materialized-views.md
- 0006-control-plane-db-postgres.md
- 0007-network-ipv6-first-ipv4-paid.md
- 0008-ingress-l4-sni-passthrough-first.md
- 0009-proxy-protocol-v2-client-ip.md
- 0010-secrets-delivery-file-format.md
- 0011-storage-local-volumes-async-backups.md
- 0012-scheduling-cpu-oversubscribe-mem-hardcap.md
- 0013-nixos-as-host-os.md

### Specifications
- specs/manifest/workload-spec.md
- specs/networking/ipam.md
- specs/networking/overlay-wireguard.md
- specs/networking/ingress-l4.md
- specs/runtime/firecracker-boot.md
- specs/runtime/image-fetch-and-cache.md
- specs/runtime/networking-inside-vm.md
- specs/scheduler/placement.md
- specs/state/materialized-views.md
- specs/secrets/format.md (referenced)
- specs/secrets/delivery.md (referenced)

### Architecture
- architecture/00-system-overview.md
- architecture/02-data-plane-host-agent.md

### Security
- security/00-threat-model.md

### Operations
- ops/01-slos-slis.md (referenced)
- ops/runbooks/* (referenced)

---

## Appendix B: Glossary of Terms Used in This Review

| Term | Definition |
|------|------------|
| Guest init | Platform-provided PID 1 inside microVM |
| Overlay IPv6 | Unique IPv6 address assigned to each instance for routing |
| WorkloadSpec | Resolved runtime configuration contract between scheduler and agent |
| Projection | Materialized view derived from event log |
| ECH | Encrypted ClientHello - TLS extension that hides SNI |

---

*End of Adversarial Review*
