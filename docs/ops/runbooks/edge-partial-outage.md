# Runbook: Edge partial outage

## Symptoms

- Elevated TCP connect failures or resets for a subset of clients
- Impact limited to one region or one edge node group
- Synthetic probes show failures from certain networks
- Alerts:
  - Edge connect success SLO burn (region scoped)
  - Elevated resets/timeouts on edge nodes

## Impact

- Customers cannot reach endpoints reliably
- Deployments may still work, but apps appear down to users

## Immediate actions

1. Declare incident if SLO burn is significant.
2. Identify affected region, endpoints, and edge nodes.
3. Reduce blast radius:
   - remove unhealthy edge nodes from rotation
   - shift traffic to healthy nodes if possible

## Triage checklist

### 1) Is this a single edge node?

- Compare per-edge-node error rates.
- If one node is clearly worse:
  - drain it from service
  - restart the edge process
  - if still bad, keep out of rotation and investigate host health

### 2) Is this upstream network or provider issue?

Signs:

- multiple edge nodes in same facility failing
- packet loss to upstream peers
- BGP or routing anomalies (if applicable)

Mitigation:

- shift traffic to alternate facility/region if available
- communicate partial outage and workarounds

### 3) Is this an endpoint config regression?

- Check recent endpoint changes:
  - port/protocol changes
  - proxy protocol settings
  - IPv4 add-on provisioning events
- Roll back recent edge config changes if correlated.

### 4) Is the data plane unreachable from the edge?

- If edge can accept connects but upstream to hosts fails:
  - check overlay health (WireGuard)
  - check host fleet health
  - check service discovery from edge to host targets

## Mitigation actions

### Option A: Remove failing edge node(s) from rotation

- Take node out of service.
- Verify connect success improves.
- Keep node isolated until root cause found.

### Option B: Fail over traffic (if multi-region)

- Move DNS or routing to healthy region.
- Ensure customers are informed about any increased latency.

### Option C: Roll back recent edge deploy/config

- Roll back edge software or config changes that correlate with incident start.
- Verify connect success and reset rate.

### Option D: Rate limit and shed load

If the edge is overloaded:

- apply connection limits per IP
- enable SYN flood protections if needed
- ensure protections do not block legitimate customer traffic

## Verification

- TCP connect success returns to baseline
- reset and timeout rates normalize
- synthetic probes succeed from diverse networks

## Escalation

Escalate if:

- suspected provider outage
- suspected DDoS
- anycast or routing changes are required

## Follow-ups

- Add or adjust synthetic probing to detect this earlier.
- Ensure edge nodes have per-node dashboards and alerts.
