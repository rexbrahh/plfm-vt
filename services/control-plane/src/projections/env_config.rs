//! Environment configuration projection handler.
//!
//! Handles env.desired_release_set and env.scale_set events,
//! updating the env_desired_releases_view and env_scale_view tables.
//!
//! These views are critical inputs for the scheduler.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for environment configuration.
pub struct EnvConfigProjection;

/// Payload for env.desired_release_set event.
#[derive(Debug, Deserialize)]
struct EnvDesiredReleaseSetPayload {
    env_id: String,
    org_id: String,
    app_id: String,
    process_type: String,
    release_id: String,
    #[serde(default)]
    deploy_id: Option<String>,
}

/// Payload for env.scale_set event.
#[derive(Debug, Deserialize)]
struct EnvScaleSetPayload {
    env_id: String,
    org_id: String,
    app_id: String,
    scales: Vec<ScaleEntry>,
}

/// Individual scale entry.
#[derive(Debug, Deserialize)]
struct ScaleEntry {
    process_type: String,
    desired: i32,
}

#[async_trait]
impl ProjectionHandler for EnvConfigProjection {
    fn name(&self) -> &'static str {
        "env_config"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["env.desired_release_set", "env.scale_set"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "env.desired_release_set" => self.handle_desired_release_set(tx, event).await,
            "env.scale_set" => self.handle_scale_set(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl EnvConfigProjection {
    /// Handle env.desired_release_set event.
    ///
    /// Updates env_desired_releases_view with the new desired release for a process type.
    async fn handle_desired_release_set(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvDesiredReleaseSetPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            env_id = %payload.env_id,
            process_type = %payload.process_type,
            release_id = %payload.release_id,
            "Setting desired release for process type"
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
        .bind(&payload.env_id)
        .bind(&payload.process_type)
        .bind(&payload.org_id)
        .bind(&payload.app_id)
        .bind(&payload.release_id)
        .bind(payload.deploy_id.as_deref())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle env.scale_set event.
    ///
    /// Updates env_scale_view with the new desired replica counts.
    async fn handle_scale_set(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvScaleSetPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            env_id = %payload.env_id,
            scale_count = payload.scales.len(),
            "Setting scale for environment"
        );

        let current_version: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(resource_version) FROM env_scale_view WHERE env_id = $1",
        )
        .bind(&payload.env_id)
        .fetch_one(&mut **tx)
        .await?;

        let next_version: i32 = current_version.unwrap_or(0).saturating_add(1) as i32;

        let process_types: Vec<String> = payload
            .scales
            .iter()
            .map(|s| s.process_type.clone())
            .collect();

        if process_types.is_empty() {
            sqlx::query("DELETE FROM env_scale_view WHERE env_id = $1")
                .bind(&payload.env_id)
                .execute(&mut **tx)
                .await?;
            return Ok(());
        }

        sqlx::query(
            r#"
            DELETE FROM env_scale_view
            WHERE env_id = $1 AND process_type != ALL($2)
            "#,
        )
        .bind(&payload.env_id)
        .bind(&process_types)
        .execute(&mut **tx)
        .await?;

        for scale in &payload.scales {
            sqlx::query(
                r#"
                INSERT INTO env_scale_view (
                    env_id, process_type, org_id, app_id, desired_replicas,
                    resource_version, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (env_id, process_type) DO UPDATE SET
                    org_id = EXCLUDED.org_id,
                    app_id = EXCLUDED.app_id,
                    desired_replicas = EXCLUDED.desired_replicas,
                    resource_version = EXCLUDED.resource_version,
                    updated_at = EXCLUDED.updated_at
                "#,
            )
            .bind(&payload.env_id)
            .bind(&scale.process_type)
            .bind(&payload.org_id)
            .bind(&payload.app_id)
            .bind(scale.desired)
            .bind(next_version)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_desired_release_set_payload_deserialization() {
        let json = r#"{
            "env_id": "env_123",
            "org_id": "org_456",
            "app_id": "app_789",
            "process_type": "web",
            "release_id": "rel_abc",
            "deploy_id": "dep_xyz"
        }"#;
        let payload: EnvDesiredReleaseSetPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.env_id, "env_123");
        assert_eq!(payload.process_type, "web");
        assert_eq!(payload.release_id, "rel_abc");
        assert_eq!(payload.deploy_id, Some("dep_xyz".to_string()));
    }

    #[test]
    fn test_env_scale_set_payload_deserialization() {
        let json = r#"{
            "env_id": "env_123",
            "org_id": "org_456",
            "app_id": "app_789",
            "scales": [
                {"process_type": "web", "desired": 3},
                {"process_type": "worker", "desired": 2}
            ]
        }"#;
        let payload: EnvScaleSetPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.env_id, "env_123");
        assert_eq!(payload.scales.len(), 2);
        assert_eq!(payload.scales[0].process_type, "web");
        assert_eq!(payload.scales[0].desired, 3);
    }

    #[test]
    fn test_env_config_projection_name() {
        let projection = EnvConfigProjection;
        assert_eq!(projection.name(), "env_config");
    }

    #[test]
    fn test_env_config_projection_event_types() {
        let projection = EnvConfigProjection;
        let types = projection.event_types();
        assert!(types.contains(&"env.desired_release_set"));
        assert!(types.contains(&"env.scale_set"));
    }
}
