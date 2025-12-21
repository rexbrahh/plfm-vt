//! Nodes projection handler.
//!
//! Handles node.enrolled, node.state_changed, and node.capacity_updated events,
//! updating the nodes_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for nodes.
pub struct NodesProjection;

/// Payload for node.enrolled event.
#[derive(Debug, Deserialize)]
struct NodeEnrolledPayload {
    node_id: String,
    hostname: String,
    region: String,
    wireguard_public_key: String,
    agent_mtls_subject: String,
    public_ipv6: String,
    #[serde(default)]
    public_ipv4: Option<String>,
    #[serde(default)]
    overlay_ipv6: Option<String>,
    cpu_cores: i32,
    memory_bytes: i64,
    #[serde(default)]
    mtu: Option<i32>,
    #[serde(default)]
    labels: serde_json::Value,
    #[serde(default)]
    allocatable: serde_json::Value,
}

/// Payload for node.state_changed event.
#[derive(Debug, Deserialize)]
struct NodeStateChangedPayload {
    node_id: String,
    #[allow(dead_code)]
    old_state: String,
    new_state: String,
    #[serde(default)]
    #[allow(dead_code)]
    reason: Option<String>,
}

/// Payload for node.capacity_updated event.
#[derive(Debug, Deserialize)]
struct NodeCapacityUpdatedPayload {
    node_id: String,
    available_cpu_cores: i32,
    available_memory_bytes: i64,
    instance_count: i32,
}

#[async_trait]
impl ProjectionHandler for NodesProjection {
    fn name(&self) -> &'static str {
        "nodes"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &[
            "node.enrolled",
            "node.state_changed",
            "node.capacity_updated",
        ]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "node.enrolled" => self.handle_node_enrolled(tx, event).await,
            "node.state_changed" => self.handle_node_state_changed(tx, event).await,
            "node.capacity_updated" => self.handle_node_capacity_updated(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl NodesProjection {
    /// Handle node.enrolled event.
    async fn handle_node_enrolled(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: NodeEnrolledPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            node_id = %payload.node_id,
            hostname = %payload.hostname,
            region = %payload.region,
            "Inserting node into nodes_view"
        );

        // Ensure labels and allocatable have default values if null
        let labels = if payload.labels.is_null() {
            serde_json::json!({
                "hostname": payload.hostname,
                "region": payload.region,
            })
        } else {
            payload.labels
        };

        let allocatable = if payload.allocatable.is_null() {
            serde_json::json!({
                "cpu_cores": payload.cpu_cores,
                "memory_bytes": payload.memory_bytes,
            })
        } else {
            payload.allocatable
        };

        sqlx::query(
            r#"
            INSERT INTO nodes_view (
                node_id, state, wireguard_public_key, agent_mtls_subject,
                public_ipv6, public_ipv4, overlay_ipv6, labels, allocatable, mtu,
                resource_version, created_at, updated_at
            )
            VALUES (
                $1, 'active', $2, $3,
                $4::INET, $5::INET, $6::INET, $7, $8, $9,
                1, $10, $10
            )
            ON CONFLICT (node_id) DO UPDATE SET
                state = 'active',
                wireguard_public_key = EXCLUDED.wireguard_public_key,
                agent_mtls_subject = EXCLUDED.agent_mtls_subject,
                public_ipv6 = EXCLUDED.public_ipv6,
                public_ipv4 = EXCLUDED.public_ipv4,
                overlay_ipv6 = EXCLUDED.overlay_ipv6,
                labels = EXCLUDED.labels,
                allocatable = EXCLUDED.allocatable,
                mtu = EXCLUDED.mtu,
                resource_version = nodes_view.resource_version + 1,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&payload.node_id)
        .bind(&payload.wireguard_public_key)
        .bind(&payload.agent_mtls_subject)
        .bind(&payload.public_ipv6)
        .bind(payload.public_ipv4.as_deref())
        .bind(payload.overlay_ipv6.as_deref())
        .bind(&labels)
        .bind(&allocatable)
        .bind(payload.mtu)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle node.state_changed event.
    async fn handle_node_state_changed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: NodeStateChangedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            node_id = %payload.node_id,
            new_state = %payload.new_state,
            "Updating node state in nodes_view"
        );

        sqlx::query(
            r#"
            UPDATE nodes_view
            SET state = $2,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE node_id = $1
            "#,
        )
        .bind(&payload.node_id)
        .bind(&payload.new_state)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle node.capacity_updated event.
    ///
    /// Updates the allocatable field with current available resources.
    async fn handle_node_capacity_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: NodeCapacityUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            node_id = %payload.node_id,
            available_cpu = %payload.available_cpu_cores,
            available_memory = %payload.available_memory_bytes,
            instance_count = %payload.instance_count,
            "Updating node capacity in nodes_view"
        );

        // Update allocatable with current available resources
        let allocatable = serde_json::json!({
            "available_cpu_cores": payload.available_cpu_cores,
            "available_memory_bytes": payload.available_memory_bytes,
            "instance_count": payload.instance_count,
        });

        sqlx::query(
            r#"
            UPDATE nodes_view
            SET allocatable = allocatable || $2::jsonb,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE node_id = $1
            "#,
        )
        .bind(&payload.node_id)
        .bind(&allocatable)
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
    fn test_node_enrolled_payload_deserialization() {
        let json = r#"{
            "node_id": "node_123",
            "hostname": "node-1",
            "region": "us-west-2",
            "wireguard_public_key": "dGVzdGtleQ==",
            "agent_mtls_subject": "CN=node-1",
            "public_ipv6": "2001:db8::1",
            "cpu_cores": 8,
            "memory_bytes": 17179869184
        }"#;
        let payload: NodeEnrolledPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.node_id, "node_123");
        assert_eq!(payload.hostname, "node-1");
        assert_eq!(payload.cpu_cores, 8);
        assert!(payload.public_ipv4.is_none());
    }

    #[test]
    fn test_node_enrolled_payload_with_optionals() {
        let json = r#"{
            "node_id": "node_123",
            "hostname": "node-1",
            "region": "us-west-2",
            "wireguard_public_key": "dGVzdGtleQ==",
            "agent_mtls_subject": "CN=node-1",
            "public_ipv6": "2001:db8::1",
            "public_ipv4": "10.0.0.1",
            "cpu_cores": 8,
            "memory_bytes": 17179869184,
            "mtu": 1500,
            "labels": {"zone": "a"}
        }"#;
        let payload: NodeEnrolledPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.public_ipv4, Some("10.0.0.1".to_string()));
        assert_eq!(payload.mtu, Some(1500));
    }

    #[test]
    fn test_node_state_changed_payload_deserialization() {
        let json = r#"{
            "node_id": "node_123",
            "old_state": "active",
            "new_state": "draining",
            "reason": "maintenance"
        }"#;
        let payload: NodeStateChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.node_id, "node_123");
        assert_eq!(payload.new_state, "draining");
    }

    #[test]
    fn test_node_capacity_updated_payload_deserialization() {
        let json = r#"{
            "node_id": "node_123",
            "available_cpu_cores": 6,
            "available_memory_bytes": 12884901888,
            "instance_count": 4
        }"#;
        let payload: NodeCapacityUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.node_id, "node_123");
        assert_eq!(payload.available_cpu_cores, 6);
        assert_eq!(payload.instance_count, 4);
    }

    #[test]
    fn test_nodes_projection_name() {
        let projection = NodesProjection;
        assert_eq!(projection.name(), "nodes");
    }

    #[test]
    fn test_nodes_projection_event_types() {
        let projection = NodesProjection;
        let types = projection.event_types();
        assert!(types.contains(&"node.enrolled"));
        assert!(types.contains(&"node.state_changed"));
        assert!(types.contains(&"node.capacity_updated"));
    }
}
