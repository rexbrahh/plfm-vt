# Runbook: WireGuard partition

The overlay network connects hosts and components that require private connectivity. Partitions can cause partial outages that look like random app failures.

## Symptoms

- Cross-host traffic fails for a subset of hosts
- Some workloads cannot reach dependencies on other hosts
- Edge can accept connections but cannot reach targets behind overlay
- Alerts:
  - Overlay peer handshake stale
  - Increased packet loss / RTT between hosts
  - Elevated upstream connect failures from edge to host targets

## Impact

- Partial app outages
- Deployments may hang (health checks fail)
- Increased latency and retries

## Immediate actions

1. Determine scope:
   - which region
   - which hosts or subnets
   - edge to host impact vs host to host only
2. Reduce blast radius:
   - avoid scheduling new instances on affected hosts
   - drain severely impacted hosts if needed

## Triage checklist

### 1) Confirm it is overlay related

- Compare failures for traffic that stays on the same host vs cross-host.
- Check overlay health metrics and peer handshake ages.

### 2) Check time and MTU

Common gotchas:

- clock skew can cause handshake confusion
- MTU mismatch can cause silent blackholes (fragmentation)

Verify:

- NTP sync on hosts
- overlay MTU settings consistent across fleet

### 3) Inspect WireGuard status on affected hosts

On an affected host:

- `wg show` and inspect:
  - latest handshake time
  - transfer counters
  - endpoint IP/port

Look for:

- handshakes not updating
- transfer stuck at 0
- endpoint changed unexpectedly

### 4) Check underlying network reachability

- Is UDP port reachable between hosts?
- Any firewall change or provider ACL issue?
- If many hosts in one rack fail together, suspect switch or provider.

## Mitigation actions

### Option A: Restart the WireGuard interface on affected hosts

- Restart the interface or the agent that manages it.
- Confirm handshakes resume.

### Option B: Reconcile and re-push peer config

If config drift is suspected:

- trigger agent reconcile for networking
- ensure keys and endpoints match expected inventory

### Option C: Isolate and drain affected hosts

If partition is not quickly fixable:

- cordon affected hosts
- drain workloads to healthy hosts
- prioritize customer facing endpoints first

### Option D: Fail over edge to healthy targets

If edge cannot reach some hosts:

- remove those targets from rotation
- route traffic only to reachable hosts

## Verification

- handshake ages return to normal distribution
- RTT and packet loss return to baseline
- edge upstream connect success returns to baseline
- customer endpoints stabilize

## Escalation

Escalate if:

- suspected provider network issue
- partition affects a large fraction of the fleet
- repeated partitions indicate systemic key rotation or MTU bugs

## Follow-ups

- Add synthetic tests for cross-host connectivity.
- If MTU related, bake MTU checks into host admission tests.
- If key rotation related, add safer rotation and rollback.
