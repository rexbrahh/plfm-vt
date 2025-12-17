# Runbook: Firecracker failure

Firecracker failures can be isolated (one instance) or systemic (host or fleet level). Treat systemic failures as high severity.

## Symptoms

- Instances fail to start or stop
- Elevated crash loop rate on new instances
- Firecracker supervisor errors (jailer, /dev/kvm, socket failures)
- Alerts:
  - microVM start failure rate spike
  - host agent reports Firecracker unhealthy

## Impact

- Deployments fail or hang
- Instances crash, causing downtime
- In worst case, entire host becomes unable to launch microVMs

## Immediate actions

1. Determine scope:
   - one instance
   - one host
   - multiple hosts (possibly new image or config regression)
2. If host-level:
   - cordon host
   - drain workloads if customer impact is ongoing
   - see `host-degraded.md`

## Triage checklist

### 1) Instance-specific failure

- Check instance event logs and Firecracker logs for that instance.
- Common causes:
  - bad kernel/initrd image
  - invalid rootfs
  - missing volume device
  - insufficient resources

Mitigation:

- restart instance on a different host
- roll back to last working release if new release introduced failure

### 2) Host-wide Firecracker failure

Check:

- `/dev/kvm` access and permissions
- file descriptor limits
- disk full on host
- CPU and memory exhaustion
- kernel logs for KVM or virtio errors

Mitigation:

- restart Firecracker supervisor and host-agent
- reboot host if kernel subsystem is wedged
- keep host cordoned until stable

### 3) Fleet-wide regression

If many hosts fail at once:

- suspect a new Firecracker build, kernel build, or config change
- roll back the change immediately
- pause deploys that create new instances
- consider pinning to last known good base image

## Mitigation actions

### Option A: Roll back Firecracker or base image

- Roll back to last known good version.
- Verify new instance launches succeed.

### Option B: Disable optional features temporarily

If snapshot restore, hugepages, or other optimizations are implicated:

- disable them to restore baseline functionality
- document the temporary mitigation

### Option C: Quarantine affected hosts

- cordon and drain
- keep for forensic analysis

## Verification

- microVM start success rate returns to baseline
- deployment time-to-converge returns to baseline
- host agent health stable for 30 minutes

## Escalation

Escalate if:

- repeated host-wide failures
- suspected kernel bug
- data corruption in guest disks suspected

## Follow-ups

- Add canary testing for Firecracker and base images.
- Improve admission tests on hosts for KVM, FD limits, disk space.
- Update runbooks with concrete error signatures as we learn them.
