//! Volumes projection handler.
//!
//! Handles volume.created and volume.deleted events, updating the volumes_view table.

use async_trait::async_trait;
use plfm_events::{VolumeCreatedPayload, VolumeDeletedPayload};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for volumes.
pub struct VolumesProjection;

#[async_trait]
impl ProjectionHandler for VolumesProjection {
    fn name(&self) -> &'static str {
        "volumes"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["volume.created", "volume.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "volume.created" => self.handle_created(tx, event).await,
            "volume.deleted" => self.handle_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl VolumesProjection {
    async fn handle_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: VolumeCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            volume_id = %payload.volume_id,
            org_id = %payload.org_id,
            size_bytes = payload.size_bytes,
            filesystem = %payload.filesystem,
            backup_enabled = payload.backup_enabled,
            "Inserting volume into volumes_view"
        );

        sqlx::query(
            r#"
            INSERT INTO volumes_view (
                volume_id,
                org_id,
                name,
                size_bytes,
                filesystem,
                backup_enabled,
                resource_version,
                created_at,
                updated_at,
                is_deleted
            )
            VALUES ($1, $2, $3, $4, $5, $6, 1, $7, $7, false)
            ON CONFLICT (volume_id) DO UPDATE SET
                name = EXCLUDED.name,
                size_bytes = EXCLUDED.size_bytes,
                filesystem = EXCLUDED.filesystem,
                backup_enabled = EXCLUDED.backup_enabled,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.volume_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.name.as_deref())
        .bind(payload.size_bytes)
        .bind(&payload.filesystem)
        .bind(payload.backup_enabled)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_deleted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: VolumeDeletedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            volume_id = %payload.volume_id,
            org_id = %payload.org_id,
            "Soft-deleting volume in volumes_view"
        );

        sqlx::query(
            r#"
            UPDATE volumes_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE volume_id = $1 AND org_id = $2
            "#,
        )
        .bind(payload.volume_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
