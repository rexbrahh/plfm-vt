# Volume Node Failure Recovery Runbook

Last updated: 2025-12-17

## Purpose

This runbook covers recovery procedures when a node hosting local volumes fails.

Since volumes are local to nodes (ADR-0011), node failure may result in volume unavailability. This runbook defines:
- Detection and triage
- Customer communication
- Recovery via backup restoration
- Data loss assessment

## Severity Classification

| Scenario | Severity | RTO Target | Data Loss Risk |
|----------|----------|------------|----------------|
| Node temporarily unreachable | P2 | 15 minutes | None |
| Node failed, disks intact | P1 | 4 hours | None |
| Node failed, disks damaged | P0 | 24 hours | Possible (since last backup) |
| Datacenter-level failure | P0 | 48 hours | Possible |

## Detection

### Automated Alerts

- `node_unreachable`: Node agent heartbeat missing for > 2 minutes
- `node_offline`: Node confirmed offline for > 5 minutes
- `volume_unavailable`: Volume-attached workload cannot start
- `backup_stale`: Volume backup older than retention policy

### Manual Detection

```bash
# Check node status
plfm admin nodes list --state offline

# Check affected volumes
plfm admin volumes list --node <node-id>

# Check affected workloads
plfm admin instances list --node <node-id>
```

## Triage Procedure

### Step 1: Confirm Node State

```bash
# Check node heartbeat history
plfm admin nodes show <node-id> --events

# Attempt direct connectivity (if network access available)
ping <node-underlay-ip>
ssh admin@<node-underlay-ip> "systemctl status plfm-agent"
```

### Step 2: Assess Impact

```bash
# List affected volumes
plfm admin volumes list --node <node-id> --format json | jq '.[] | {id, name, org_id, env_id, size_bytes}'

# List affected customers
plfm admin volumes list --node <node-id> --format json | jq -r '.[] | .org_id' | sort -u

# Check workload status
plfm admin instances list --node <node-id> --format json | jq '.[] | {instance_id, env_id, status}'
```

### Step 3: Classify Scenario

**Scenario A: Node Temporarily Unreachable**
- Network issue or reboot
- Proceed to: [Temporary Outage Recovery](#temporary-outage-recovery)

**Scenario B: Node Failed, Recovery Possible**
- Hardware issue but disks recoverable
- Proceed to: [Disk Recovery](#disk-recovery)

**Scenario C: Node and Disks Lost**
- Complete node loss
- Proceed to: [Backup Restoration](#backup-restoration)

## Temporary Outage Recovery

### Expected Resolution: < 15 minutes

1. **Wait for automatic recovery** (5 minutes)
   - Agent will reconnect automatically
   - Workloads will resume

2. **If not recovered, investigate**:
   ```bash
   # Check datacenter/network status
   # Contact datacenter if needed
   ```

3. **Force node check-in** (if accessible):
   ```bash
   ssh admin@<node-ip> "systemctl restart plfm-agent"
   ```

4. **Verify recovery**:
   ```bash
   plfm admin nodes show <node-id>
   plfm admin instances list --node <node-id> --status running
   ```

## Disk Recovery

### When disks are intact but node needs replacement

1. **Mark node as draining** (prevents new placements):
   ```bash
   plfm admin nodes drain <node-id>
   ```

2. **Migrate stateless workloads**:
   ```bash
   # Automatic: scheduler will reschedule non-volume workloads
   # Monitor progress
   plfm admin instances list --node <node-id>
   ```

3. **For volume-attached workloads**:
   - These cannot migrate automatically
   - Options:
     a. Repair node and reattach disks
     b. Move disks to new node (requires physical access)
     c. Restore from backup (data loss possible)

4. **If moving disks physically**:
   ```bash
   # On new node, after disk attachment
   plfm admin volumes relocate <volume-id> --to-node <new-node-id>
   
   # Verify volume integrity
   plfm admin volumes fsck <volume-id>
   ```

5. **Restart workloads**:
   ```bash
   plfm admin instances restart --env <env-id> --process <process-type>
   ```

## Backup Restoration

### When volume data is lost

**RPO Warning**: Data since last successful backup will be lost.

1. **Identify latest backup**:
   ```bash
   plfm admin snapshots list --volume <volume-id> --status completed
   ```

2. **Communicate with customer** (see [Customer Communication](#customer-communication))

3. **Create new volume from backup**:
   ```bash
   # Create restore job
   plfm admin restore create \
     --snapshot <snapshot-id> \
     --target-node <new-node-id> \
     --new-volume-name "<volume-name>-restored"
   
   # Monitor progress
   plfm admin restore show <restore-id>
   ```

4. **Update volume attachment**:
   ```bash
   # Remove old attachment
   plfm admin volume-attachments delete <old-attachment-id>
   
   # Create new attachment
   plfm admin volume-attachments create \
     --volume <new-volume-id> \
     --env <env-id> \
     --process <process-type> \
     --mount-path <path>
   ```

5. **Redeploy workload**:
   ```bash
   plfm -e <env-id> deploy --force
   ```

6. **Verify data integrity** with customer

## Customer Communication

### Template: Volume Unavailability (P1/P2)

```
Subject: [Action Required] Volume temporarily unavailable for {app_name}

Hello,

We're experiencing temporary unavailability of your volume "{volume_name}" 
attached to {app_name}/{env_name}.

Impact:
- Your {process_type} process cannot start
- Data on the volume is safe

Current Status:
- We are investigating the underlying infrastructure issue
- Estimated resolution: {ETA}

We will update you within {next_update_time}.

No action is required from you at this time.
```

### Template: Data Loss (P0)

```
Subject: [Urgent] Data loss incident for {app_name} - Action may be required

Hello,

We experienced a hardware failure affecting your volume "{volume_name}".

Impact:
- Data written since {last_backup_time} may be lost
- We are restoring from backup dated {backup_time}

Current Status:
- Restoration in progress
- Estimated completion: {ETA}

Action Required:
- Please review your data after restoration
- Contact support if you notice missing data

We sincerely apologize for this incident. A detailed incident report 
will be provided within 72 hours.
```

## Post-Incident

### Verification Checklist

- [ ] All affected workloads running
- [ ] Volume data verified with customer
- [ ] Backup schedule resumed
- [ ] Node removed or repaired
- [ ] Incident documented

### Metrics to Record

- Time to detection
- Time to customer notification
- Time to resolution
- Data loss (if any)
- Number of affected customers

### Follow-up Actions

1. **Root cause analysis** (within 24 hours)
2. **Incident report** (within 72 hours)
3. **Customer compensation** (if warranted)
4. **Process improvements** (if needed)

## Prevention Measures

### Backup Verification

Weekly:
```bash
# Verify backup integrity
plfm admin backups verify --all --sample-rate 10%

# Check backup freshness
plfm admin backups list --older-than 24h
```

### Monitoring

- Alert on backup age > policy threshold
- Alert on backup verification failures
- Dashboard: backup coverage by org

### Customer Guidance

Recommend customers:
- Test backup restoration periodically
- Maintain application-level backups for critical data
- Use appropriate backup retention settings

## CLI Reference

```bash
# Node management
plfm admin nodes list [--state <state>]
plfm admin nodes show <node-id>
plfm admin nodes drain <node-id>
plfm admin nodes decommission <node-id>

# Volume management
plfm admin volumes list [--node <node-id>]
plfm admin volumes show <volume-id>
plfm admin volumes fsck <volume-id>
plfm admin volumes relocate <volume-id> --to-node <node-id>

# Snapshot/backup management
plfm admin snapshots list --volume <volume-id>
plfm admin snapshots create --volume <volume-id>

# Restore management
plfm admin restore create --snapshot <id> --target-node <id>
plfm admin restore show <restore-id>
plfm admin restore list

# Volume attachment
plfm admin volume-attachments list --env <env-id>
plfm admin volume-attachments delete <attachment-id>
plfm admin volume-attachments create --volume <id> --env <id> --process <name> --mount-path <path>
```

## Escalation

- P2: On-call engineer
- P1: On-call engineer + Engineering lead
- P0: On-call + Engineering lead + Customer success + Management

## References

- `docs/ADRs/0011-storage-local-volumes-async-backups.md`
- `docs/specs/storage/backups.md`
- `docs/specs/storage/restore-and-migration.md`
- `docs/specs/scheduler/drain-evict-reschedule.md`
