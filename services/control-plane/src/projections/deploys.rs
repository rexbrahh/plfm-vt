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
#[allow(dead_code)]
struct DeployCreatedPayload {
    deploy_id: String,
    org_id: String,
    app_id: String,
    env_id: String,
    kind: String,
    release_id: String,
    process_types: Vec<String>,
    strategy: String,
    initiated_at: String,
}

/// Payload for deploy.status_changed event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DeployStatusChangedPayload {
    deploy_id: String,
    org_id: String,
    env_id: String,
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    failed_reason: Option<String>,
    updated_at: String,
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
    ///
    /// This updates both deploys_view and env_desired_releases_view.
    /// The latter is critical for the scheduler to know what release to run.
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

        // 1. Insert into deploys_view
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
        .bind("queued")
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        // 2. Update env_desired_releases_view for each process type
        // This is what the scheduler reads to know what to run
        for process_type in &payload.process_types {
            debug!(
                env_id = %env_id,
                process_type = %process_type,
                release_id = %payload.release_id,
                deploy_id = %event.aggregate_id,
                "Setting desired release for process type in env_desired_releases_view"
            );

            sqlx::query(
                r#"
                INSERT INTO env_desired_releases_view (
                    env_id, process_type, org_id, app_id, release_id, deploy_id,
                    resource_version, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, 1, $7)
                ON CONFLICT (env_id, process_type) DO UPDATE SET
                    release_id = EXCLUDED.release_id,
                    deploy_id = EXCLUDED.deploy_id,
                    resource_version = env_desired_releases_view.resource_version + 1,
                    updated_at = EXCLUDED.updated_at
                "#,
            )
            .bind(env_id)
            .bind(process_type)
            .bind(org_id)
            .bind(app_id)
            .bind(&payload.release_id)
            .bind(&event.aggregate_id)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        }

        // 3. If this is the first deploy, also set default scale of 1 for each process type
        // This ensures the scheduler allocates at least one instance
        for process_type in &payload.process_types {
            // Only insert if not already set (don't override user-set scale)
            sqlx::query(
                r#"
                INSERT INTO env_scale_view (
                    env_id, process_type, org_id, app_id, desired_replicas,
                    resource_version, updated_at
                )
                VALUES ($1, $2, $3, $4, 1, 1, $5)
                ON CONFLICT (env_id, process_type) DO NOTHING
                "#,
            )
            .bind(env_id)
            .bind(process_type)
            .bind(org_id)
            .bind(app_id)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        }

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
            status = %payload.status,
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
        .bind(&payload.status)
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
            "deploy_id": "dep_123",
            "org_id": "org_123",
            "app_id": "app_123",
            "env_id": "env_123",
            "release_id": "rel_123",
            "kind": "deploy",
            "process_types": ["web", "worker"],
            "strategy": "rolling",
            "initiated_at": "2025-01-01T00:00:00Z"
        }"#;
        let payload: DeployCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.deploy_id, "dep_123");
        assert_eq!(payload.org_id, "org_123");
        assert_eq!(payload.app_id, "app_123");
        assert_eq!(payload.env_id, "env_123");
        assert_eq!(payload.release_id, "rel_123");
        assert_eq!(payload.kind, "deploy");
        assert_eq!(payload.process_types, vec!["web", "worker"]);
        assert_eq!(payload.strategy, "rolling");
        assert_eq!(payload.initiated_at, "2025-01-01T00:00:00Z");
    }

    #[test]
    fn test_deploy_status_changed_payload_deserialization() {
        let json = r#"{
            "deploy_id": "dep_123",
            "org_id": "org_123",
            "env_id": "env_123",
            "status": "rolling",
            "message": "Starting deployment",
            "updated_at": "2025-01-01T00:00:10Z"
        }"#;
        let payload: DeployStatusChangedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.deploy_id, "dep_123");
        assert_eq!(payload.org_id, "org_123");
        assert_eq!(payload.env_id, "env_123");
        assert_eq!(payload.status, "rolling");
        assert_eq!(payload.message, Some("Starting deployment".to_string()));
        assert_eq!(payload.failed_reason, None);
        assert_eq!(payload.updated_at, "2025-01-01T00:00:10Z");
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
