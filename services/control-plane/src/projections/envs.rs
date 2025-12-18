//! Environments projection handler.
//!
//! Handles env.created, env.updated, and env.deleted events, updating the envs_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for environments.
pub struct EnvsProjection;

/// Payload for env.created event.
#[derive(Debug, Deserialize)]
struct EnvCreatedPayload {
    name: String,
}

/// Payload for env.updated event.
#[derive(Debug, Deserialize)]
struct EnvUpdatedPayload {
    #[serde(default)]
    name: Option<String>,
}

#[async_trait]
impl ProjectionHandler for EnvsProjection {
    fn name(&self) -> &'static str {
        "envs"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["env.created", "env.updated", "env.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "env.created" => self.handle_env_created(tx, event).await,
            "env.updated" => self.handle_env_updated(tx, event).await,
            "env.deleted" => self.handle_env_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl EnvsProjection {
    /// Handle env.created event.
    async fn handle_env_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("env.created event missing org_id".to_string())
        })?;

        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("env.created event missing app_id".to_string())
        })?;

        debug!(
            env_id = %event.aggregate_id,
            org_id = %org_id,
            app_id = %app_id,
            name = %payload.name,
            "Inserting env into envs_view"
        );

        sqlx::query(
            r#"
            INSERT INTO envs_view (env_id, org_id, app_id, name, resource_version, created_at, updated_at, is_deleted)
            VALUES ($1, $2, $3, $4, 1, $5, $5, false)
            ON CONFLICT (env_id) DO UPDATE SET
                name = EXCLUDED.name,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(app_id)
        .bind(&payload.name)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle env.updated event.
    async fn handle_env_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            env_id = %event.aggregate_id,
            name = ?payload.name,
            "Updating env in envs_view"
        );

        if let Some(name) = payload.name {
            sqlx::query(
                r#"
                UPDATE envs_view
                SET name = $2,
                    resource_version = resource_version + 1,
                    updated_at = $3
                WHERE env_id = $1 AND NOT is_deleted
                "#,
            )
            .bind(&event.aggregate_id)
            .bind(&name)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE envs_view
                SET resource_version = resource_version + 1,
                    updated_at = $2
                WHERE env_id = $1 AND NOT is_deleted
                "#,
            )
            .bind(&event.aggregate_id)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        }

        Ok(())
    }

    /// Handle env.deleted event.
    async fn handle_env_deleted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        debug!(
            env_id = %event.aggregate_id,
            "Soft-deleting env in envs_view"
        );

        sqlx::query(
            r#"
            UPDATE envs_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE env_id = $1
            "#,
        )
        .bind(&event.aggregate_id)
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
    fn test_env_created_payload_deserialization() {
        let json = r#"{"name": "production"}"#;
        let payload: EnvCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, "production");
    }

    #[test]
    fn test_env_updated_payload_deserialization() {
        let json = r#"{"name": "staging"}"#;
        let payload: EnvUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, Some("staging".to_string()));
    }

    #[test]
    fn test_env_updated_payload_empty() {
        let json = r#"{}"#;
        let payload: EnvUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, None);
    }

    #[test]
    fn test_envs_projection_name() {
        let projection = EnvsProjection;
        assert_eq!(projection.name(), "envs");
    }

    #[test]
    fn test_envs_projection_event_types() {
        let projection = EnvsProjection;
        assert!(projection.event_types().contains(&"env.created"));
        assert!(projection.event_types().contains(&"env.updated"));
        assert!(projection.event_types().contains(&"env.deleted"));
    }
}
