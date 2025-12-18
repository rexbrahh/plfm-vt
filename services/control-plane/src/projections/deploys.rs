//! Deploys projection handler.
//!
//! Handles deploy.created and deploy.status_changed events, updating the deploys_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for deploys.
pub struct DeploysProjection;

/// Payload for deploy.created event.
#[derive(Debug, Deserialize)]
struct DeployCreatedPayload {
    release_id: String,
    kind: String,
    process_types: Vec<String>,
    status: String,
}

/// Payload for deploy.status_changed event.
#[derive(Debug, Deserialize)]
struct DeployStatusChangedPayload {
    #[allow(dead_code)]
    old_status: String,
    new_status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    failed_reason: Option<String>,
}

#[async_trait]
impl ProjectionHandler for DeploysProjection {
    fn name(&self) -> &'static str {
        "deploys"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["deploy.created", "deploy.status_changed"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "deploy.created" => self.handle_deploy_created(tx, event).await,
            "deploy.status_changed" => self.handle_deploy_status_changed(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl DeploysProjection {
    /// Handle deploy.created event.
    async fn handle_deploy_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: DeployCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("deploy.created event missing org_id".to_string())
        })?;

        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("deploy.created event missing app_id".to_string())
        })?;

        let env_id = event.env_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("deploy.created event missing env_id".to_string())
        })?;

        debug!(
            deploy_id = %event.aggregate_id,
            org_id = %org_id,
            app_id = %app_id,
            env_id = %env_id,
            release_id = %payload.release_id,
            kind = %payload.kind,
            "Inserting deploy into deploys_view"
        );

        sqlx::query(
            r#"
            INSERT INTO deploys_view (
                deploy_id, org_id, app_id, env_id, kind, release_id, process_types,
                status, message, failed_reason, resource_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, NULL, 1, $9, $9)
            ON CONFLICT (deploy_id) DO UPDATE SET
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(app_id)
        .bind(env_id)
        .bind(&payload.kind)
        .bind(&payload.release_id)
        .bind(serde_json::to_value(&payload.process_types).unwrap_or_default())
        .bind(&payload.status)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle deploy.status_changed event.
    async fn handle_deploy_status_changed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: DeployStatusChangedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            deploy_id = %event.aggregate_id,
            new_status = %payload.new_status,
            "Updating deploy status in deploys_view"
        );

        sqlx::query(
            r#"
            UPDATE deploys_view
            SET status = $2,
                message = COALESCE($3, message),
                failed_reason = COALESCE($4, failed_reason),
                resource_version = resource_version + 1,
                updated_at = $5
            WHERE deploy_id = $1
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(&payload.new_status)
        .bind(&payload.message)
        .bind(&payload.failed_reason)
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
    fn test_deploy_created_payload_deserialization() {
        let json = r#"{
            "release_id": "rel_123",
            "kind": "deploy",
            "process_types": ["web", "worker"],
            "status": "queued"
        }"#;
        let payload: DeployCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.release_id, "rel_123");
        assert_eq!(payload.kind, "deploy");
        assert_eq!(payload.process_types, vec!["web", "worker"]);
        assert_eq!(payload.status, "queued");
    }

    #[test]
    fn test_deploy_status_changed_payload_deserialization() {
        let json = r#"{
            "old_status": "queued",
            "new_status": "rolling",
            "message": "Starting deployment"
        }"#;
        let payload: DeployStatusChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.old_status, "queued");
        assert_eq!(payload.new_status, "rolling");
        assert_eq!(payload.message, Some("Starting deployment".to_string()));
        assert_eq!(payload.failed_reason, None);
    }

    #[test]
    fn test_deploys_projection_name() {
        let projection = DeploysProjection;
        assert_eq!(projection.name(), "deploys");
    }

    #[test]
    fn test_deploys_projection_event_types() {
        let projection = DeploysProjection;
        assert!(projection.event_types().contains(&"deploy.created"));
        assert!(projection.event_types().contains(&"deploy.status_changed"));
    }
}
