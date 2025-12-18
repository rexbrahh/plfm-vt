//! Applications projection handler.
//!
//! Handles app.created, app.updated, and app.deleted events, updating the apps_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for applications.
pub struct AppsProjection;

/// Payload for app.created event.
#[derive(Debug, Deserialize)]
struct AppCreatedPayload {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Payload for app.updated event.
#[derive(Debug, Deserialize)]
struct AppUpdatedPayload {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl ProjectionHandler for AppsProjection {
    fn name(&self) -> &'static str {
        "apps"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["app.created", "app.updated", "app.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "app.created" => self.handle_app_created(tx, event).await,
            "app.updated" => self.handle_app_updated(tx, event).await,
            "app.deleted" => self.handle_app_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl AppsProjection {
    /// Handle app.created event.
    async fn handle_app_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: AppCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("app.created event missing org_id".to_string())
        })?;

        debug!(
            app_id = %event.aggregate_id,
            org_id = %org_id,
            name = %payload.name,
            "Inserting app into apps_view"
        );

        sqlx::query(
            r#"
            INSERT INTO apps_view (app_id, org_id, name, description, resource_version, created_at, updated_at, is_deleted)
            VALUES ($1, $2, $3, $4, 1, $5, $5, false)
            ON CONFLICT (app_id) DO UPDATE SET
                name = EXCLUDED.name,
                description = EXCLUDED.description,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(&payload.name)
        .bind(&payload.description)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle app.updated event.
    async fn handle_app_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: AppUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            app_id = %event.aggregate_id,
            name = ?payload.name,
            "Updating app in apps_view"
        );

        // Build dynamic update based on what's in the payload
        match (&payload.name, &payload.description) {
            (Some(name), Some(desc)) => {
                sqlx::query(
                    r#"
                    UPDATE apps_view
                    SET name = $2,
                        description = $3,
                        resource_version = resource_version + 1,
                        updated_at = $4
                    WHERE app_id = $1 AND NOT is_deleted
                    "#,
                )
                .bind(&event.aggregate_id)
                .bind(name)
                .bind(desc)
                .bind(event.occurred_at)
                .execute(&mut **tx)
                .await?;
            }
            (Some(name), None) => {
                sqlx::query(
                    r#"
                    UPDATE apps_view
                    SET name = $2,
                        resource_version = resource_version + 1,
                        updated_at = $3
                    WHERE app_id = $1 AND NOT is_deleted
                    "#,
                )
                .bind(&event.aggregate_id)
                .bind(name)
                .bind(event.occurred_at)
                .execute(&mut **tx)
                .await?;
            }
            (None, Some(desc)) => {
                sqlx::query(
                    r#"
                    UPDATE apps_view
                    SET description = $2,
                        resource_version = resource_version + 1,
                        updated_at = $3
                    WHERE app_id = $1 AND NOT is_deleted
                    "#,
                )
                .bind(&event.aggregate_id)
                .bind(desc)
                .bind(event.occurred_at)
                .execute(&mut **tx)
                .await?;
            }
            (None, None) => {
                sqlx::query(
                    r#"
                    UPDATE apps_view
                    SET resource_version = resource_version + 1,
                        updated_at = $2
                    WHERE app_id = $1 AND NOT is_deleted
                    "#,
                )
                .bind(&event.aggregate_id)
                .bind(event.occurred_at)
                .execute(&mut **tx)
                .await?;
            }
        }

        Ok(())
    }

    /// Handle app.deleted event.
    async fn handle_app_deleted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        debug!(
            app_id = %event.aggregate_id,
            "Soft-deleting app in apps_view"
        );

        sqlx::query(
            r#"
            UPDATE apps_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE app_id = $1
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
    fn test_app_created_payload_deserialization() {
        let json = r#"{"name": "my-app", "description": "A test app"}"#;
        let payload: AppCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, "my-app");
        assert_eq!(payload.description, Some("A test app".to_string()));
    }

    #[test]
    fn test_app_created_payload_without_description() {
        let json = r#"{"name": "my-app"}"#;
        let payload: AppCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, "my-app");
        assert_eq!(payload.description, None);
    }

    #[test]
    fn test_app_updated_payload_deserialization() {
        let json = r#"{"name": "updated-app"}"#;
        let payload: AppUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.name, Some("updated-app".to_string()));
        assert_eq!(payload.description, None);
    }

    #[test]
    fn test_apps_projection_name() {
        let projection = AppsProjection;
        assert_eq!(projection.name(), "apps");
    }

    #[test]
    fn test_apps_projection_event_types() {
        let projection = AppsProjection;
        assert!(projection.event_types().contains(&"app.created"));
        assert!(projection.event_types().contains(&"app.updated"));
        assert!(projection.event_types().contains(&"app.deleted"));
    }
}
