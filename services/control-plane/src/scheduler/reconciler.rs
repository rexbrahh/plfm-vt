//! Scheduler reconciler for instance allocation.
//!
//! The reconciler is responsible for:
//! - Reading desired state from env_desired_releases_view and env_scale_view
//! - Computing what instances should exist
//! - Allocating instances to nodes based on capacity
//! - Emitting instance.allocated and instance.desired_state_changed events
//!
//! See: docs/specs/scheduler/reconciliation-loop.md

use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, EnvId, InstanceId, OrgId, ReleaseId, RequestId};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::net::Ipv6Addr;
use tracing::{debug, info, instrument, warn};

use crate::db::{AppendEvent, EventStore};

/// Result type for scheduler operations.
pub type SchedulerResult<T> = Result<T, SchedulerError>;

/// Errors that can occur during scheduling.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("no eligible nodes available")]
    NoEligibleNodes,

    #[error("event store error: {0}")]
    EventStore(String),

    #[error("ipam error: {0}")]
    Ipam(String),
}

/// Desired state for a (env, process_type) group.
#[derive(Debug, Clone)]
pub struct GroupDesiredState {
    pub org_id: OrgId,
    pub app_id: AppId,
    pub env_id: EnvId,
    pub process_type: String,
    pub release_id: ReleaseId,
    pub deploy_id: Option<String>,
    pub desired_replicas: i32,
    pub spec_hash: String,
    pub secrets_version_id: Option<String>,
}

/// Current instance state.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub instance_id: String,
    #[allow(dead_code)]
    pub node_id: String,
    pub desired_state: String,
    pub spec_hash: String,
    #[allow(dead_code)]
    pub release_id: String,
}

/// Node capacity for placement decisions.
#[derive(Debug, Clone)]
pub struct NodeCapacity {
    pub node_id: String,
    pub state: String,
    pub allocatable_memory_bytes: i64,
    pub allocatable_cpu_cores: i32,
    pub available_memory_bytes: i64,
    pub available_cpu_cores: i32,
    pub instance_count: i32,
}

/// The scheduler reconciler.
pub struct SchedulerReconciler {
    pool: PgPool,
}

impl SchedulerReconciler {
    /// Create a new scheduler reconciler.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run a single reconciliation pass for all groups.
    #[instrument(skip(self))]
    pub async fn reconcile_all(&self) -> SchedulerResult<ReconcileStats> {
        let mut stats = ReconcileStats::default();

        // Get all groups that need reconciliation
        let groups = self.get_all_groups().await?;
        debug!(group_count = groups.len(), "Found groups to reconcile");

        for group in groups {
            match self.reconcile_group(&group).await {
                Ok(group_stats) => {
                    stats.groups_processed += 1;
                    stats.instances_allocated += group_stats.instances_allocated;
                    stats.instances_drained += group_stats.instances_drained;
                }
                Err(e) => {
                    warn!(
                        env_id = %group.env_id,
                        process_type = %group.process_type,
                        error = %e,
                        "Failed to reconcile group"
                    );
                    stats.groups_failed += 1;
                }
            }
        }

        info!(
            groups_processed = stats.groups_processed,
            groups_failed = stats.groups_failed,
            instances_allocated = stats.instances_allocated,
            instances_drained = stats.instances_drained,
            "Reconciliation pass complete"
        );

        Ok(stats)
    }

    /// Get all groups that have desired state defined.
    async fn get_all_groups(&self) -> SchedulerResult<Vec<GroupDesiredState>> {
        // Join env_desired_releases_view with env_scale_view to get full group info
        let rows = sqlx::query_as::<_, GroupRow>(
            r#"
            SELECT
                r.org_id,
                r.app_id,
                r.env_id,
                r.process_type,
                r.release_id,
                r.deploy_id,
                COALESCE(s.desired_replicas, 1) as desired_replicas,
                sb.current_version_id as secrets_version_id
            FROM env_desired_releases_view r
            LEFT JOIN env_scale_view s
                ON r.env_id = s.env_id AND r.process_type = s.process_type
            LEFT JOIN secret_bundles_view sb
                ON r.env_id = sb.env_id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut groups = Vec::new();
        for row in rows {
            let release_id: ReleaseId = row.release_id.parse().unwrap_or_else(|_| ReleaseId::new());
            let env_id = row.env_id.parse().unwrap_or_else(|_| EnvId::new());
            let (volume_hash, has_volumes) = self
                .volume_hash_for_group(&env_id, &row.process_type)
                .await?;
            let desired_replicas = if has_volumes && row.desired_replicas > 1 {
                warn!(
                    env_id = %env_id,
                    process_type = %row.process_type,
                    desired_replicas = row.desired_replicas,
                    "Volume-backed process types are limited to 1 replica in v1; clamping"
                );
                1
            } else {
                row.desired_replicas
            };
            let spec_hash = compute_spec_hash(
                &release_id,
                &row.process_type,
                row.secrets_version_id.as_deref(),
                &volume_hash,
            );
            groups.push(GroupDesiredState {
                org_id: row.org_id.parse().unwrap_or_else(|_| OrgId::new()),
                app_id: row.app_id.parse().unwrap_or_else(|_| AppId::new()),
                env_id,
                process_type: row.process_type,
                release_id,
                deploy_id: row.deploy_id,
                desired_replicas,
                spec_hash,
                secrets_version_id: row.secrets_version_id,
            });
        }

        Ok(groups)
    }

    /// Reconcile a single group.
    #[instrument(skip(self), fields(env_id = %group.env_id, process_type = %group.process_type))]
    async fn reconcile_group(&self, group: &GroupDesiredState) -> SchedulerResult<GroupStats> {
        let mut stats = GroupStats::default();

        // Get current instances for this group
        let current_instances = self.get_group_instances(group).await?;

        // Partition instances
        let matching: Vec<_> = current_instances
            .iter()
            .filter(|i| i.desired_state != "stopped" && i.spec_hash == group.spec_hash)
            .collect();
        let old: Vec<_> = current_instances
            .iter()
            .filter(|i| i.desired_state != "stopped" && i.spec_hash != group.spec_hash)
            .collect();
        let running_count = matching.len() + old.len();

        debug!(
            desired = group.desired_replicas,
            matching = matching.len(),
            old = old.len(),
            total_running = running_count,
            "Group instance state"
        );

        // Scale up: need more matching instances
        let matching_count = matching.len() as i32;
        if matching_count < group.desired_replicas {
            let to_create = group.desired_replicas - matching_count;
            for _ in 0..to_create {
                match self.allocate_instance(group).await {
                    Ok(instance_id) => {
                        info!(
                            instance_id = %instance_id,
                            env_id = %group.env_id,
                            process_type = %group.process_type,
                            "Allocated new instance"
                        );
                        stats.instances_allocated += 1;
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to allocate instance");
                        // Don't fail the whole group, continue with what we have
                    }
                }
            }
        }

        // Drain old instances (ones with wrong spec_hash)
        for instance in &old {
            match self.drain_instance(instance).await {
                Ok(_) => {
                    info!(
                        instance_id = %instance.instance_id,
                        "Draining old instance"
                    );
                    stats.instances_drained += 1;
                }
                Err(e) => {
                    warn!(
                        instance_id = %instance.instance_id,
                        error = %e,
                        "Failed to drain instance"
                    );
                }
            }
        }

        // Scale down: too many matching instances
        if matching_count > group.desired_replicas {
            let to_drain = (matching_count - group.desired_replicas) as usize;
            // Drain oldest instances first (by instance_id which is ULID-based)
            let mut to_drain_instances: Vec<_> = matching.iter().collect();
            to_drain_instances.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));

            for instance in to_drain_instances.into_iter().take(to_drain) {
                match self.drain_instance(instance).await {
                    Ok(_) => {
                        info!(
                            instance_id = %instance.instance_id,
                            "Draining excess instance (scale down)"
                        );
                        stats.instances_drained += 1;
                    }
                    Err(e) => {
                        warn!(
                            instance_id = %instance.instance_id,
                            error = %e,
                            "Failed to drain instance"
                        );
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Get current instances for a group.
    async fn get_group_instances(
        &self,
        group: &GroupDesiredState,
    ) -> SchedulerResult<Vec<InstanceState>> {
        let rows = sqlx::query_as::<_, InstanceRow>(
            r#"
            SELECT instance_id, node_id, desired_state, spec_hash, release_id
            FROM instances_desired_view
            WHERE env_id = $1 AND process_type = $2 AND desired_state != 'stopped'
            ORDER BY created_at
            "#,
        )
        .bind(group.env_id.to_string())
        .bind(&group.process_type)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| InstanceState {
                instance_id: r.instance_id,
                node_id: r.node_id,
                desired_state: r.desired_state,
                spec_hash: r.spec_hash,
                release_id: r.release_id,
            })
            .collect())
    }

    /// Allocate a new instance for a group.
    async fn allocate_instance(&self, group: &GroupDesiredState) -> SchedulerResult<InstanceId> {
        let request_id = RequestId::new();
        let instance_id = InstanceId::new();

        // Get release info for resources
        let release_info = self.get_release_info(&group.release_id).await?;
        let required_cpu_cores = release_info.cpu.max(1.0).ceil() as i32;
        let required_memory_bytes = release_info.memory_bytes;

        // Find best node for placement
        let node = self
            .find_best_node(required_memory_bytes, required_cpu_cores)
            .await?;
        debug!(
            node_id = %node.node_id,
            node_state = %node.state,
            allocatable_memory_bytes = node.allocatable_memory_bytes,
            allocatable_cpu_cores = node.allocatable_cpu_cores,
            available_memory_bytes = node.available_memory_bytes,
            available_cpu_cores = node.available_cpu_cores,
            instance_count = node.instance_count,
            required_memory_bytes,
            required_cpu_cores,
            "Selected node for placement"
        );

        // Allocate overlay IPv6 via IPAM
        let overlay_ipv6 = self.allocate_instance_ipv6(&instance_id).await?;

        let resources_snapshot = serde_json::json!({
            "cpu": release_info.cpu,
            "memory_bytes": release_info.memory_bytes,
        });

        // Create instance.allocated event
        let event = AppendEvent {
            aggregate_type: AggregateType::Instance,
            aggregate_id: instance_id.to_string(),
            aggregate_seq: 1,
            event_type: "instance.allocated".to_string(),
            event_version: 1,
            actor_type: ActorType::System,
            actor_id: "scheduler".to_string(),
            org_id: Some(group.org_id),
            request_id: request_id.to_string(),
            idempotency_key: None,
            app_id: Some(group.app_id),
            env_id: Some(group.env_id),
            correlation_id: group.deploy_id.clone(),
            causation_id: None,
            payload: serde_json::json!({
                "instance_id": instance_id.to_string(),
                "node_id": node.node_id,
                "process_type": group.process_type,
                "release_id": group.release_id.to_string(),
                "secrets_version_id": group.secrets_version_id,
                "overlay_ipv6": overlay_ipv6,
                "resources_snapshot": resources_snapshot,
                "spec_hash": group.spec_hash,
                "deploy_id": group.deploy_id,
            }),
            ..Default::default()
        };

        let event_store = EventStore::new(self.pool.clone());
        event_store
            .append(event)
            .await
            .map_err(|e| SchedulerError::EventStore(e.to_string()))?;

        Ok(instance_id)
    }

    /// Drain an instance.
    async fn drain_instance(&self, instance: &InstanceState) -> SchedulerResult<()> {
        if instance.desired_state == "draining" {
            // Already draining
            return Ok(());
        }

        let request_id = RequestId::new();

        let event_store = EventStore::new(self.pool.clone());
        let current_seq = event_store
            .get_latest_aggregate_seq(&AggregateType::Instance, &instance.instance_id)
            .await
            .map_err(|e| SchedulerError::EventStore(e.to_string()))?
            .unwrap_or(0);

        let event = AppendEvent {
            aggregate_type: AggregateType::Instance,
            aggregate_id: instance.instance_id.clone(),
            aggregate_seq: current_seq + 1,
            event_type: "instance.desired_state_changed".to_string(),
            event_version: 1,
            actor_type: ActorType::System,
            actor_id: "scheduler".to_string(),
            org_id: None,
            request_id: request_id.to_string(),
            idempotency_key: None,
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({
                "instance_id": instance.instance_id,
                "desired_state": "draining",
                "drain_grace_seconds": 10,
                "reason": "scheduler_drain",
            }),
            ..Default::default()
        };

        event_store
            .append(event)
            .await
            .map_err(|e| SchedulerError::EventStore(e.to_string()))?;

        Ok(())
    }

    /// Find the best node for placement.
    async fn find_best_node(
        &self,
        required_memory_bytes: i64,
        required_cpu_cores: i32,
    ) -> SchedulerResult<NodeCapacity> {
        // Get all active nodes with their capacity
        let nodes = sqlx::query_as::<_, NodeCapacityRow>(
            r#"
            SELECT
                n.node_id,
                n.state,
                COALESCE((n.allocatable->>'memory_bytes')::BIGINT, 0) as allocatable_memory_bytes,
                COALESCE((n.allocatable->>'cpu_cores')::INT, 0) as allocatable_cpu_cores,
                COALESCE(
                    (n.allocatable->>'available_memory_bytes')::BIGINT,
                    (n.allocatable->>'memory_bytes')::BIGINT,
                    0
                ) as available_memory_bytes,
                COALESCE(
                    (n.allocatable->>'available_cpu_cores')::INT,
                    (n.allocatable->>'cpu_cores')::INT,
                    0
                ) as available_cpu_cores,
                COALESCE((n.allocatable->>'instance_count')::INT, 0) as instance_count
            FROM nodes_view n
            WHERE n.state = 'active'
              AND COALESCE(
                    (n.allocatable->>'available_memory_bytes')::BIGINT,
                    (n.allocatable->>'memory_bytes')::BIGINT,
                    0
                ) >= $1
              AND COALESCE(
                    (n.allocatable->>'available_cpu_cores')::INT,
                    (n.allocatable->>'cpu_cores')::INT,
                    0
                ) >= $2
            ORDER BY
                -- Prefer nodes with more available resources
                COALESCE(
                    (n.allocatable->>'available_memory_bytes')::BIGINT,
                    (n.allocatable->>'memory_bytes')::BIGINT,
                    0
                ) DESC,
                COALESCE(
                    (n.allocatable->>'available_cpu_cores')::INT,
                    (n.allocatable->>'cpu_cores')::INT,
                    0
                ) DESC,
                -- Tie-break by node_id for determinism
                n.node_id ASC
            LIMIT 1
            "#,
        )
        .bind(required_memory_bytes)
        .bind(required_cpu_cores)
        .fetch_optional(&self.pool)
        .await?;

        match nodes {
            Some(row) => Ok(NodeCapacity {
                node_id: row.node_id,
                state: row.state,
                allocatable_memory_bytes: row.allocatable_memory_bytes,
                allocatable_cpu_cores: row.allocatable_cpu_cores,
                available_memory_bytes: row.available_memory_bytes,
                available_cpu_cores: row.available_cpu_cores,
                instance_count: row.instance_count,
            }),
            None => Err(SchedulerError::NoEligibleNodes),
        }
    }

    /// Get release info for resource calculations.
    async fn get_release_info(&self, release_id: &ReleaseId) -> SchedulerResult<ReleaseInfo> {
        let row = sqlx::query_as::<_, ReleaseInfoRow>(
            r#"
            SELECT image_ref, manifest_hash
            FROM releases_view
            WHERE release_id = $1
            "#,
        )
        .bind(release_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(ReleaseInfo {
                image_ref: r.image_ref,
                manifest_hash: r.manifest_hash,
                // Default resources - would come from manifest in full implementation
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024, // 512 MB
            }),
            None => {
                // Default if release not found
                Ok(ReleaseInfo {
                    image_ref: "unknown".to_string(),
                    manifest_hash: "unknown".to_string(),
                    cpu: 1.0,
                    memory_bytes: 512 * 1024 * 1024,
                })
            }
        }
    }
}

/// Statistics from a reconciliation pass.
#[derive(Debug, Default, Clone)]
pub struct ReconcileStats {
    pub groups_processed: i32,
    pub groups_failed: i32,
    pub instances_allocated: i32,
    pub instances_drained: i32,
}

/// Statistics from reconciling a single group.
#[derive(Debug, Default, Clone)]
struct GroupStats {
    instances_allocated: i32,
    instances_drained: i32,
}

/// Release info for resource calculation.
#[derive(Debug, Clone)]
struct ReleaseInfo {
    #[allow(dead_code)]
    image_ref: String,
    #[allow(dead_code)]
    manifest_hash: String,
    cpu: f64,
    memory_bytes: i64,
}

/// Compute a deterministic spec hash for a group.
fn compute_spec_hash(
    release_id: &ReleaseId,
    process_type: &str,
    secrets_version: Option<&str>,
    volume_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(release_id.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(process_type.as_bytes());
    hasher.update(b":");
    hasher.update(secrets_version.unwrap_or("none").as_bytes());
    hasher.update(b":");
    hasher.update(volume_hash.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

impl SchedulerReconciler {
    async fn allocate_instance_ipv6(&self, instance_id: &InstanceId) -> SchedulerResult<String> {
        let prefix = std::env::var("PLFM_INSTANCE_IPV6_PREFIX")
            .or_else(|_| std::env::var("GHOST_INSTANCE_IPV6_PREFIX"))
            .unwrap_or_else(|_| "fd00::".to_string());

        let base: Ipv6Addr = prefix.parse().map_err(|_| {
            SchedulerError::Ipam(format!(
                "invalid instance IPv6 prefix '{}'; expected /64 base address",
                prefix
            ))
        })?;

        let base_u128 = u128::from(base) & (!0u128 << 64);
        let mut attempts = 0;

        loop {
            let suffix: i64 = sqlx::query_scalar("SELECT nextval('ipam_instance_suffix_seq')")
                .fetch_one(&self.pool)
                .await?;

            if suffix < 0 {
                attempts += 1;
                if attempts > 5 {
                    return Err(SchedulerError::Ipam(
                        "failed to allocate IPv6 suffix".to_string(),
                    ));
                }
                continue;
            }

            let suffix_u64 = suffix as u64;
            let addr = Ipv6Addr::from(base_u128 | suffix_u64 as u128);

            let insert = sqlx::query(
                r#"
                INSERT INTO ipam_instances (instance_id, ipv6_suffix, overlay_ipv6)
                VALUES ($1, $2, $3::inet)
                "#,
            )
            .bind(instance_id.to_string())
            .bind(suffix)
            .bind(addr.to_string())
            .execute(&self.pool)
            .await;

            match insert {
                Ok(_) => return Ok(addr.to_string()),
                Err(sqlx::Error::Database(db_err)) => {
                    let constraint = db_err.constraint().unwrap_or_default();
                    if constraint == "ipam_instances_pkey"
                        || constraint == "ipam_instances_ipv6_suffix_key"
                        || constraint == "ipam_instances_overlay_ipv6_key"
                    {
                        attempts += 1;
                        if attempts > 5 {
                            return Err(SchedulerError::Ipam(
                                "ipam allocation retry limit reached".to_string(),
                            ));
                        }
                        continue;
                    }
                    return Err(SchedulerError::Database(sqlx::Error::Database(db_err)));
                }
                Err(e) => return Err(SchedulerError::Database(e)),
            }
        }
    }

    async fn volume_hash_for_group(
        &self,
        env_id: &EnvId,
        process_type: &str,
    ) -> SchedulerResult<(String, bool)> {
        let rows = sqlx::query_as::<_, VolumeAttachmentRow>(
            r#"
            SELECT volume_id, mount_path, read_only
            FROM volume_attachments_view
            WHERE env_id = $1
              AND process_type = $2
              AND NOT is_deleted
            ORDER BY volume_id ASC, mount_path ASC
            "#,
        )
        .bind(env_id.to_string())
        .bind(process_type)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(("none".to_string(), false));
        }

        let mut hasher = Sha256::new();
        for row in rows {
            hasher.update(row.volume_id.as_bytes());
            hasher.update(b":");
            hasher.update(row.mount_path.as_bytes());
            hasher.update(b":");
            hasher.update(if row.read_only { b"ro" } else { b"rw" });
            hasher.update(b";");
        }

        Ok((format!("{:x}", hasher.finalize())[..16].to_string(), true))
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

#[derive(Debug)]
struct GroupRow {
    org_id: String,
    app_id: String,
    env_id: String,
    process_type: String,
    release_id: String,
    deploy_id: Option<String>,
    desired_replicas: i32,
    secrets_version_id: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for GroupRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            release_id: row.try_get("release_id")?,
            deploy_id: row.try_get("deploy_id")?,
            desired_replicas: row.try_get("desired_replicas")?,
            secrets_version_id: row.try_get("secrets_version_id")?,
        })
    }
}

#[derive(Debug)]
struct InstanceRow {
    instance_id: String,
    node_id: String,
    desired_state: String,
    spec_hash: String,
    release_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            node_id: row.try_get("node_id")?,
            desired_state: row.try_get("desired_state")?,
            spec_hash: row.try_get("spec_hash")?,
            release_id: row.try_get("release_id")?,
        })
    }
}

#[derive(Debug)]
struct NodeCapacityRow {
    node_id: String,
    state: String,
    allocatable_memory_bytes: i64,
    allocatable_cpu_cores: i32,
    available_memory_bytes: i64,
    available_cpu_cores: i32,
    instance_count: i32,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for NodeCapacityRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            node_id: row.try_get("node_id")?,
            state: row.try_get("state")?,
            allocatable_memory_bytes: row.try_get("allocatable_memory_bytes")?,
            allocatable_cpu_cores: row.try_get("allocatable_cpu_cores")?,
            available_memory_bytes: row.try_get("available_memory_bytes")?,
            available_cpu_cores: row.try_get("available_cpu_cores")?,
            instance_count: row.try_get("instance_count")?,
        })
    }
}

#[derive(Debug)]
struct ReleaseInfoRow {
    image_ref: String,
    manifest_hash: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ReleaseInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            image_ref: row.try_get("image_ref")?,
            manifest_hash: row.try_get("manifest_hash")?,
        })
    }
}

#[derive(Debug)]
struct VolumeAttachmentRow {
    volume_id: String,
    mount_path: String,
    read_only: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for VolumeAttachmentRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            volume_id: row.try_get("volume_id")?,
            mount_path: row.try_get("mount_path")?,
            read_only: row.try_get("read_only")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_spec_hash_deterministic() {
        let release_id: ReleaseId = "rel_01ABC".parse().unwrap_or_else(|_| ReleaseId::new());
        let hash1 = compute_spec_hash(&release_id, "web", None, "none");
        let hash2 = compute_spec_hash(&release_id, "web", None, "none");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_spec_hash_different_inputs() {
        let release_id: ReleaseId = "rel_01ABC".parse().unwrap_or_else(|_| ReleaseId::new());
        let hash1 = compute_spec_hash(&release_id, "web", None, "none");
        let hash2 = compute_spec_hash(&release_id, "worker", None, "none");
        assert_ne!(hash1, hash2);
    }
}
