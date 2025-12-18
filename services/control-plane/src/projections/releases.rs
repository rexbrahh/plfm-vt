//! Releases projection handler.
//!
//! Handles release.created events, updating the releases_view table.
//! Releases are immutable - once created, they cannot be updated or deleted.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for releases.
pub struct ReleasesProjection;

/// Payload for release.created event.
#[derive(Debug, Deserialize)]
struct ReleaseCreatedPayload {
    image_ref: String,
    image_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
}

#[async_trait]
impl ProjectionHandler for ReleasesProjection {
    fn name(&self) -> &'static str {
        "releases"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["release.created"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "release.created" => self.handle_release_created(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl ReleasesProjection {
    /// Handle release.created event.
    async fn handle_release_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ReleaseCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("release.created event missing org_id".to_string())
        })?;

        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload("release.created event missing app_id".to_string())
        })?;

        debug!(
            release_id = %event.aggregate_id,
            org_id = %org_id,
            app_id = %app_id,
            image_ref = %payload.image_ref,
            "Inserting release into releases_view"
        );

        sqlx::query(
            r#"
            INSERT INTO releases_view (
                release_id, org_id, app_id, image_ref, index_or_manifest_digest,
                resolved_digests, manifest_schema_version, manifest_hash,
                resource_version, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9)
            ON CONFLICT (release_id) DO NOTHING
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(app_id)
        .bind(&payload.image_ref)
        .bind(&payload.image_digest)
        .bind(serde_json::json!({})) // resolved_digests
        .bind(payload.manifest_schema_version)
        .bind(&payload.manifest_hash)
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
    fn test_release_created_payload_deserialization() {
        let json = r#"{
            "image_ref": "registry.example.com/app:v1.0",
            "image_digest": "sha256:abc123",
            "manifest_schema_version": 1,
            "manifest_hash": "def456"
        }"#;
        let payload: ReleaseCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.image_ref, "registry.example.com/app:v1.0");
        assert_eq!(payload.image_digest, "sha256:abc123");
        assert_eq!(payload.manifest_schema_version, 1);
        assert_eq!(payload.manifest_hash, "def456");
    }

    #[test]
    fn test_releases_projection_name() {
        let projection = ReleasesProjection;
        assert_eq!(projection.name(), "releases");
    }

    #[test]
    fn test_releases_projection_event_types() {
        let projection = ReleasesProjection;
        assert!(projection.event_types().contains(&"release.created"));
    }
}
