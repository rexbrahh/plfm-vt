# Runbook: Host degraded

## Symptoms

- Elevated workload errors for instances placed on a specific host
- Host agent heartbeats missing or flapping
- High CPU steal, OOM kills, disk IO errors, or packet loss on one host
- Alerts:
  - Host health degraded
  - Elevated microVM start failures on host
  - Per-host edge connect or app error spike

## Impact

- Instances on the host may crash, hang, or become unreachable
- Rescheduling may increase load on remaining hosts (watch headroom)

## Safety note

Avoid "fixing in place" while the host is serving traffic. Prioritize reducing blast radius.

## Immediate actions

1. Identify the host ID and region.
2. Cordon the host (prevent new placements).
3. Drain workloads from the host if customer impact is ongoing.

## Cordon and drain procedure

### 1) Cordon

- Mark host as unschedulable in the scheduler inventory.
- Verify no new instances are placed.

### 2) Drain

- Evict or migrate instances to healthy hosts.
- For stateful workloads:
  - detach volumes safely
  - prefer controlled shutdown if possible

If drain increases region utilization beyond safe thresholds, pause and add capacity or perform partial drain.

## Triage checklist (after blast radius reduction)

### CPU saturation or steal

- Check host CPU usage and steal time.
- Check noisy neighbor patterns (one instance consuming CPU).
- Mitigate:
  - enforce per-instance CPU limits
  - move offending instances
  - if host wide, reboot may be required

### Memory pressure

- Look for OOM kills in kernel logs.
- Check host reserved memory configuration.
- Mitigate:
  - drain further
  - restart host services if leaked memory
  - reboot if kernel memory fragmentation is suspected

### Disk issues

- Check dmesg for IO errors and filesystem warnings.
- Check disk utilization and inode exhaustion.
- Mitigate:
  - stop scheduling to the host
  - if disk is failing, remove host from service and replace hardware
  - avoid deleting data blindly during incident

### Network issues

- Check packet loss to edge and to overlay peers.
- Verify WireGuard health (see `wireguard-partition.md` if it is peer related).
- Mitigate:
  - move workloads off host
  - restart networking services
  - consider provider network issue if multiple hosts affected

## Recovery actions

After root cause is understood:

- Restart host-agent and Firecracker supervisor if they are wedged.
- Reboot host if kernel or hardware state is suspect.
- If hardware is faulty:
  - keep host cordoned
  - schedule replacement
  - decommission after workloads are gone

## Verification

- Host heartbeats stable for 30 minutes
- No elevated error rate for workloads previously on host
- Firecracker start success returns to baseline
- Network and disk metrics return to normal

## Escalation

Escalate if:

- disk corruption suspected
- repeated degradations on same host within 7 days
- multiple hosts in a rack degrade together (possible switch or provider issue)

## Follow-ups

- Open a ticket to investigate root cause and prevent recurrence.
- Update capacity plan if drain reduced headroom below target.
