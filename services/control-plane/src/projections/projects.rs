//! Projects projection handler.
//!
//! Handles project.created, project.updated, and project.deleted events,
//! updating the projects_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for projects.
pub struct ProjectsProjection;

/// Payload for project.created event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ProjectCreatedPayload {
    project_id: String,
    org_id: String,
    name: String,
}

/// Payload for project.updated event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ProjectUpdatedPayload {
    project_id: String,
    org_id: String,
    #[serde(default)]
    name: Option<String>,
}

#[async_trait]
impl ProjectionHandler for ProjectsProjection {
    fn name(&self) -> &'static str {
        "projects"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["project.created", "project.updated", "project.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "project.created" => self.handle_project_created(tx, event).await,
            "project.updated" => self.handle_project_updated(tx, event).await,
            "project.deleted" => self.handle_project_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl ProjectsProjection {
    async fn handle_project_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ProjectCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("project.created event missing org_id".to_string())
        })?;

        debug!(
            project_id = %event.aggregate_id,
            org_id = %org_id,
            name = %payload.name,
            "Inserting project into projects_view"
        );

        sqlx::query(
            r#"
            INSERT INTO projects_view (
                project_id,
                org_id,
                name,
                resource_version,
                created_at,
                updated_at,
                is_deleted
            )
            VALUES ($1, $2, $3, 1, $4, $4, false)
            ON CONFLICT (project_id) DO UPDATE SET
                name = EXCLUDED.name,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id.to_string())
        .bind(&payload.name)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_project_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ProjectUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            project_id = %event.aggregate_id,
            name = ?payload.name,
            "Updating project in projects_view"
        );

        sqlx::query(
            r#"
            UPDATE projects_view
            SET name = COALESCE($2, name),
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE project_id = $1
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(payload.name)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_project_deleted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        debug!(
            project_id = %event.aggregate_id,
            "Soft-deleting project in projects_view"
        );

        sqlx::query(
            r#"
            UPDATE projects_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE project_id = $1
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
    fn test_projects_projection_name() {
        let proj = ProjectsProjection;
        assert_eq!(proj.name(), "projects");
    }

    #[test]
    fn test_projects_projection_event_types() {
        let proj = ProjectsProjection;
        assert!(proj.event_types().contains(&"project.created"));
        assert!(proj.event_types().contains(&"project.updated"));
        assert!(proj.event_types().contains(&"project.deleted"));
    }

    #[test]
    fn test_project_created_payload_deserialization() {
        let json = r#"{"project_id": "project_test", "org_id": "org_test", "name": "my-project"}"#;
        let payload: ProjectCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.project_id, "project_test");
        assert_eq!(payload.org_id, "org_test");
        assert_eq!(payload.name, "my-project");
    }
}
