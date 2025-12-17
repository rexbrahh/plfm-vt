# Runbook: Volume corruption

This runbook covers suspected corruption or failure of persistent volumes.

## Symptoms

- Application reports data corruption, checksum failures, or missing files
- Kernel logs show IO errors, ext4/xfs errors, or device resets
- Volume attach or mount starts failing
- Alerts:
  - volume IO error rate
  - filesystem errors detected
  - snapshot failures for a volume

## Impact

- Potential data loss or data integrity risk
- Downtime for workloads using the volume
- Restore operations may be required

## Safety rules

- Do not run filesystem repair on a mounted, writable filesystem.
- Preserve evidence: snapshot or copy data before destructive actions.
- Prefer restoring into a new volume over in-place repair.

## Immediate actions (stop the bleeding)

1. Identify affected volume ID, workload, and host.
2. Reduce writes immediately:
   - scale workload to zero, or
   - switch application to read-only mode, or
   - detach the volume
3. If possible, take a snapshot of the current state for forensic purposes.

## Triage checklist

### 1) Is it a filesystem issue or underlying device issue?

Check host logs:

- IO errors, timeouts, NVMe resets
- SMART warnings (if available)
- repeated attach failures

If underlying device is failing, treat host as degraded:
- cordon host and consider drain.

### 2) Is corruption localized to a recent deploy?

- Compare incident start time to deployment time.
- Some apps corrupt their own data due to bugs.
- If correlated, roll back application first.

### 3) Can we restore from snapshots?

- Identify last known good snapshot.
- Confirm retention and integrity.
- If snapshots are missing or failing, escalate severity.

## Mitigation options

### Option A: Restore from snapshot (preferred)

1. Create a new volume from last known good snapshot.
2. Attach to a recovery instance.
3. Validate data integrity.
4. Swap the restored volume into the workload.
5. Keep the corrupted volume detached for forensic window.

### Option B: Offline filesystem repair (use only if restore is impossible)

1. Ensure volume is detached and not mounted.
2. Attach to a recovery instance.
3. Run filesystem repair tool appropriate to filesystem.
4. Validate application data.
5. If repair succeeds, reattach to workload with caution.

### Option C: Partial data extraction

If restore is not possible and repair is risky:

1. Mount read-only.
2. Copy out critical data.
3. Recreate volume and rehydrate data.

## Verification

- Workload starts and passes health checks
- Application-level integrity checks pass
- Snapshot jobs for the volume succeed again
- No new IO errors in host logs

## Escalation

Escalate immediately if:

- multiple volumes show corruption (systemic storage issue)
- corruption affects snapshot store or backup system
- suspected data breach or unauthorized access

## Follow-ups

- Ensure snapshots and backup alerts were functioning.
- Add integrity checks if possible (checksums, app-level verification).
- If device failure, replace hardware and review host admission tests.
