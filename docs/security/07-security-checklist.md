# Security checklist

Status: draft
Owner: TBD
Last reviewed: 2025-12-20

This document provides actionable security checklists for deployment, operations, and incident response. Use these checklists to verify security posture at each stage.

Related docs:
- Threat model: `00-threat-model.md`
- Feature review checklist: `../architecture/08-security-architecture.md`

## Pre-deployment checklist

Before deploying a new control plane or node agent version:

### Authentication and authorization
- [ ] All API mutations require authentication
- [ ] Default deny policy enforced (explicit scopes required)
- [ ] Sensitive operations require elevated roles
- [ ] Resource access uses immutable IDs, not user-provided names
- [ ] Token expiration and refresh logic tested

### Tenant isolation
- [ ] MicroVM boundary enforced for all workloads
- [ ] Network policy defaults to deny lateral tenant traffic
- [ ] Volume ownership verified before mount
- [ ] Quota enforcement active for all resource dimensions
- [ ] Org and env boundaries enforced at API layer

### Secrets
- [ ] Secrets encrypted at rest (envelope encryption with KMS)
- [ ] No secret values in logs, errors, or crash dumps
- [ ] Secret delivery via control plane reconciliation only
- [ ] Secrets never returned after creation (metadata only)
- [ ] Rotation mechanism tested

### Supply chain
- [ ] Images deployed by digest (not mutable tags)
- [ ] Signature verification enabled (if configured)
- [ ] Dependency versions locked
- [ ] SBOM generated for release artifacts
- [ ] CLI binaries signed

### Audit
- [ ] All security-relevant actions logged
- [ ] Audit events include actor, action, resource, timestamp
- [ ] Request IDs propagated for correlation
- [ ] Audit log tamper evidence enabled
- [ ] Retention policy configured

## Runtime operations checklist

Periodic verification during normal operations:

### Control plane
- [ ] API rate limits active and thresholds appropriate
- [ ] Idempotency records cleaned up on schedule
- [ ] Projection checkpoints advancing (no stuck projections)
- [ ] Database connections pooled and bounded
- [ ] Health endpoints returning expected status

### Host agents
- [ ] Node enrollment ceremony completed with mutual auth
- [ ] Heartbeats received within expected interval
- [ ] Seccomp policy applied to VMM processes
- [ ] cgroup v2 limits enforced (CPU fair, memory hard)
- [ ] WireGuard overlay membership matches expected nodes

### Networking
- [ ] Ingress routes owned by correct tenants
- [ ] IPv4 allocations within quota limits
- [ ] Proxy Protocol v2 only from trusted sources
- [ ] No stale route bindings
- [ ] Egress abuse detection thresholds configured

### Observability
- [ ] Workload logs retained per policy (7 days default)
- [ ] Platform logs retained per policy (14 days default)
- [ ] No secrets in log output (spot check)
- [ ] Metrics cardinality bounded
- [ ] Alert thresholds calibrated

## Vulnerability response checklist

When a vulnerability is discovered:

### Triage
- [ ] Severity assessed (critical, high, medium, low)
- [ ] Affected components identified
- [ ] Exploitation status determined (theoretical vs in-the-wild)
- [ ] Customer impact scope estimated
- [ ] Patch timeline set per severity (critical: 24-72h, high: 7d)

### Remediation
- [ ] Patch developed or obtained
- [ ] Patch tested in staging environment
- [ ] Rollout plan approved
- [ ] Rollback plan documented
- [ ] Communication prepared (if customer-facing)

### Post-incident
- [ ] Patch deployed to all affected systems
- [ ] Verification that vulnerability no longer exploitable
- [ ] Customer advisory published (if applicable)
- [ ] Incident retrospective scheduled
- [ ] Monitoring added for regression detection

## Incident response checklist

When a security incident is detected:

### Containment
- [ ] Affected tokens revoked
- [ ] Affected org or env disabled if necessary
- [ ] Affected node quarantined if compromised
- [ ] Affected routes removed or rebound
- [ ] Credentials rotated for impacted systems

### Investigation
- [ ] Audit logs preserved (legal hold if needed)
- [ ] Event log snapshot taken
- [ ] Affected timeframe identified
- [ ] Attack vector determined
- [ ] Blast radius assessed

### Recovery
- [ ] Systems restored from known-good state
- [ ] Projections rebuilt if integrity uncertain
- [ ] Volumes restored from backups if needed
- [ ] Affected customers notified
- [ ] Monitoring increased for similar activity

### Follow-up
- [ ] Root cause analysis completed
- [ ] Preventive measures identified
- [ ] Controls updated or added
- [ ] Runbooks updated
- [ ] Team debrief conducted

## New feature security review

For every new feature, answer these questions (from `08-security-architecture.md`):

1. Does it expand a trust boundary or add a new one?
2. Does it introduce new secret handling paths?
3. Does it introduce a new externally reachable port or protocol?
4. Does it create cross-tenant shared state or shared execution?
5. Can it be abused to cause resource exhaustion?
6. Is it auditable? Can we attribute actions to an actor?
7. Does it preserve locked ADR decisions, or require a new ADR?

If any answer is "yes" or "unclear", escalate for security review before merge.
