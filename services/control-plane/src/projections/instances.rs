//! Instances projection handler for scheduler output.
//!
//! Handles instance.allocated, instance.desired_state_changed events,
//! updating the instances_desired_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;
use crate::projections::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for instances.
pub struct InstancesProjection;

/// Payload for instance.allocated event.
#[derive(Debug, Deserialize)]
struct InstanceAllocatedPayload {
    instance_id: String,
    node_id: String,
    process_type: String,
    release_id: String,
    #[serde(default)]
    secrets_version_id: Option<String>,
    overlay_ipv6: String,
    #[serde(default)]
    resources_snapshot: serde_json::Value,
    spec_hash: String,
    #[serde(default)]
    deploy_id: Option<String>,
}

/// Payload for instance.desired_state_changed event.
#[derive(Debug, Deserialize)]
struct InstanceDesiredStateChangedPayload {
    instance_id: String,
    desired_state: String,
    #[serde(default)]
    #[allow(dead_code)]
    drain_grace_seconds: Option<i32>,
    #[serde(default)]
    #[allow(dead_code)]
    reason: Option<String>,
}

/// Payload for instance.status_changed event.
#[derive(Debug, Deserialize)]
struct InstanceStatusChangedPayload {
    instance_id: String,
    #[serde(default)]
    node_id: Option<String>,
    status: String,
    #[serde(default)]
    boot_id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    microvm_id: Option<String>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    reason_code: Option<String>,
    #[serde(default)]
    reason_detail: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    reported_at: Option<String>,
}

#[async_trait]
impl ProjectionHandler for InstancesProjection {
    fn name(&self) -> &'static str {
        "instances"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &[
            "instance.allocated",
            "instance.desired_state_changed",
            "instance.status_changed",
        ]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "instance.allocated" => self.handle_instance_allocated(tx, event).await,
            "instance.desired_state_changed" => {
                self.handle_instance_desired_state_changed(tx, event).await
            }
            "instance.status_changed" => self.handle_instance_status_changed(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl InstancesProjection {
    /// Handle instance.allocated event.
    async fn handle_instance_allocated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: InstanceAllocatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("instance.allocated event missing org_id".to_string())
        })?;

        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("instance.allocated event missing app_id".to_string())
        })?;

        let env_id = event.env_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("instance.allocated event missing env_id".to_string())
        })?;

        debug!(
            instance_id = %payload.instance_id,
            node_id = %payload.node_id,
            env_id = %env_id,
            process_type = %payload.process_type,
            "Inserting instance into instances_desired_view"
        );

        // Ensure resources_snapshot has default value if null
        let resources_snapshot = if payload.resources_snapshot.is_null() {
            serde_json::json!({})
        } else {
            payload.resources_snapshot
        };

        sqlx::query(
            r#"
            INSERT INTO instances_desired_view (
                instance_id, org_id, app_id, env_id, process_type, node_id,
                desired_state, release_id, deploy_id, secrets_version_id, overlay_ipv6,
                resources_snapshot, spec_hash, generation, resource_version,
                created_at, updated_at
            )
            VALUES (
                $1, $2, $3, $4, $5, $6,
                'running', $7, $8, $9, $10::INET,
                $11, $12, 1, 1,
                $13, $13
            )
            ON CONFLICT (instance_id) DO UPDATE SET
                desired_state = 'running',
                node_id = EXCLUDED.node_id,
                release_id = EXCLUDED.release_id,
                deploy_id = EXCLUDED.deploy_id,
                secrets_version_id = EXCLUDED.secrets_version_id,
                resources_snapshot = EXCLUDED.resources_snapshot,
                spec_hash = EXCLUDED.spec_hash,
                resource_version = instances_desired_view.resource_version + 1,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&payload.instance_id)
        .bind(org_id)
        .bind(app_id)
        .bind(env_id.to_string())
        .bind(&payload.process_type)
        .bind(&payload.node_id)
        .bind(&payload.release_id)
        .bind(payload.deploy_id.as_deref())
        .bind(payload.secrets_version_id.as_deref())
        .bind(&payload.overlay_ipv6)
        .bind(&resources_snapshot)
        .bind(&payload.spec_hash)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle instance.desired_state_changed event.
    async fn handle_instance_desired_state_changed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: InstanceDesiredStateChangedPayload =
            serde_json::from_value(event.payload.clone())
                .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            instance_id = %payload.instance_id,
            desired_state = %payload.desired_state,
            "Updating instance desired_state in instances_desired_view"
        );

        sqlx::query(
            r#"
            UPDATE instances_desired_view
            SET desired_state = $2,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE instance_id = $1
            "#,
        )
        .bind(&payload.instance_id)
        .bind(&payload.desired_state)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle instance.status_changed event.
    ///
    /// Updates the instances_status_view table with the current status
    /// as reported by the node-agent.
    async fn handle_instance_status_changed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: InstanceStatusChangedPayload =
            serde_json::from_value(event.payload.clone())
                .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "instance.status_changed event missing org_id".to_string(),
            )
        })?;

        let env_id = event.env_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "instance.status_changed event missing env_id".to_string(),
            )
        })?;

        debug!(
            instance_id = %payload.instance_id,
            status = %payload.status,
            "Updating instance status in instances_status_view"
        );

        let node_id = if let Some(ref nid) = payload.node_id {
            nid.clone()
        } else {
            sqlx::query_scalar("SELECT node_id FROM instances_desired_view WHERE instance_id = $1")
                .bind(&payload.instance_id)
                .fetch_optional(&mut **tx)
                .await?
                .unwrap_or_else(|| "unknown".to_string())
        };

        sqlx::query(
            r#"
            INSERT INTO instances_status_view (
                instance_id, org_id, env_id, node_id, status,
                boot_id, exit_code, reason_code, reason_detail,
                reported_at,
                resource_version, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1, $10)
            ON CONFLICT (instance_id) DO UPDATE SET
                org_id = EXCLUDED.org_id,
                env_id = EXCLUDED.env_id,
                node_id = EXCLUDED.node_id,
                status = EXCLUDED.status,
                boot_id = COALESCE(EXCLUDED.boot_id, instances_status_view.boot_id),
                exit_code = EXCLUDED.exit_code,
                reason_code = EXCLUDED.reason_code,
                reason_detail = EXCLUDED.reason_detail,
                reported_at = EXCLUDED.reported_at,
                resource_version = instances_status_view.resource_version + 1,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&payload.instance_id)
        .bind(org_id)
        .bind(env_id.to_string())
        .bind(&node_id)
        .bind(&payload.status)
        .bind(payload.boot_id.as_deref())
        .bind(payload.exit_code)
        .bind(payload.reason_code.as_deref())
        .bind(payload.reason_detail.as_deref())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_allocated_payload_deserialization() {
        let json = r#"{
            "instance_id": "inst_123",
            "node_id": "node_456",
            "process_type": "web",
            "release_id": "rel_789",
            "overlay_ipv6": "fd00::1",
            "spec_hash": "abc123"
        }"#;
        let payload: InstanceAllocatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.instance_id, "inst_123");
        assert_eq!(payload.node_id, "node_456");
        assert_eq!(payload.process_type, "web");
    }

    #[test]
    fn test_instance_desired_state_changed_payload_deserialization() {
        let json = r#"{
            "instance_id": "inst_123",
            "desired_state": "draining",
            "drain_grace_seconds": 10,
            "reason": "scale_down"
        }"#;
        let payload: InstanceDesiredStateChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.instance_id, "inst_123");
        assert_eq!(payload.desired_state, "draining");
        assert_eq!(payload.drain_grace_seconds, Some(10));
    }

    #[test]
    fn test_instances_projection_name() {
        let projection = InstancesProjection;
        assert_eq!(projection.name(), "instances");
    }

    #[test]
    fn test_instances_projection_event_types() {
        let projection = InstancesProjection;
        let types = projection.event_types();
        assert!(types.contains(&"instance.allocated"));
        assert!(types.contains(&"instance.desired_state_changed"));
        assert!(types.contains(&"instance.status_changed"));
    }

    #[test]
    fn test_instance_status_changed_payload_deserialization() {
        let json = r#"{
            "instance_id": "inst_123",
            "node_id": "node_789",
            "status": "ready",
            "boot_id": "boot_456",
            "reported_at": "2025-12-21T00:00:00Z"
        }"#;
        let payload: InstanceStatusChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.instance_id, "inst_123");
        assert_eq!(payload.status, "ready");
        assert_eq!(payload.boot_id, Some("boot_456".to_string()));
        assert_eq!(payload.node_id, Some("node_789".to_string()));
    }

    #[test]
    fn test_instance_status_changed_payload_with_failure() {
        let json = r#"{
            "instance_id": "inst_123",
            "node_id": "node_789",
            "status": "failed",
            "exit_code": 137,
            "reason_code": "oom_killed",
            "reason_detail": "Out of memory",
            "reported_at": "2025-12-21T00:00:00Z"
        }"#;
        let payload: InstanceStatusChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.instance_id, "inst_123");
        assert_eq!(payload.status, "failed");
        assert_eq!(payload.reason_code, Some("oom_killed".to_string()));
        assert_eq!(payload.reason_detail, Some("Out of memory".to_string()));
        assert_eq!(payload.exit_code, Some(137));
    }
}
