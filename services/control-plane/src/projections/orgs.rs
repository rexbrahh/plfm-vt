//! Organizations projection handler.
//!
//! Handles org.created and org.updated events, updating the orgs_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for organizations.
pub struct OrgsProjection;

/// Payload for org.created event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OrgCreatedPayload {
    org_id: String,
    name: String,
}

/// Payload for org.updated event.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OrgUpdatedPayload {
    org_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    billing_email: Option<String>,
}

#[async_trait]
impl ProjectionHandler for OrgsProjection {
    fn name(&self) -> &'static str {
        "orgs"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["org.created", "org.updated"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "org.created" => self.handle_org_created(tx, event).await,
            "org.updated" => self.handle_org_updated(tx, event).await,
            _ => {
                // Unknown event type for this handler - should not happen
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl OrgsProjection {
    /// Handle org.created event.
    async fn handle_org_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: OrgCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            org_id = %event.aggregate_id,
            name = %payload.name,
            "Inserting org into orgs_view"
        );

        sqlx::query(
            r#"
            INSERT INTO orgs_view (org_id, name, resource_version, created_at, updated_at)
            VALUES ($1, $2, 1, $3, $3)
            ON CONFLICT (org_id) DO UPDATE SET
                name = EXCLUDED.name,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(&payload.name)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Handle org.updated event.
    async fn handle_org_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: OrgUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            org_id = %event.aggregate_id,
            name = ?payload.name,
            "Updating org in orgs_view"
        );

        // Update fields that are present in the payload
        if let Some(name) = payload.name {
            sqlx::query(
                r#"
                UPDATE orgs_view
                SET name = $2,
                    resource_version = resource_version + 1,
                    updated_at = $3
                WHERE org_id = $1
                "#,
            )
            .bind(&event.aggregate_id)
            .bind(&name)
            .bind(event.occurred_at)
            .execute(&mut **tx)
            .await?;
        } else {
            // Just bump the version/timestamp
            sqlx::query(
                r#"
                UPDATE orgs_view
                SET resource_version = resource_version + 1,
                    updated_at = $2
                WHERE org_id = $1
                "#,
            )
            .bind(&event.aggregate_id)
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
    fn test_org_created_payload_deserialization() {
        let json = r#"{"org_id": "org_test", "name": "Test Org"}"#;
        let payload: OrgCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.org_id, "org_test");
        assert_eq!(payload.name, "Test Org");
    }

    #[test]
    fn test_org_updated_payload_deserialization() {
        let json = r#"{"org_id": "org_test", "name": "Updated Org"}"#;
        let payload: OrgUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.org_id, "org_test");
        assert_eq!(payload.name, Some("Updated Org".to_string()));
    }

    #[test]
    fn test_org_updated_payload_empty() {
        let json = r#"{"org_id": "org_test"}"#;
        let payload: OrgUpdatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.org_id, "org_test");
        assert_eq!(payload.name, None);
    }

    #[test]
    fn test_orgs_projection_name() {
        let projection = OrgsProjection;
        assert_eq!(projection.name(), "orgs");
    }

    #[test]
    fn test_orgs_projection_event_types() {
        let projection = OrgsProjection;
        assert!(projection.event_types().contains(&"org.created"));
        assert!(projection.event_types().contains(&"org.updated"));
    }
}
