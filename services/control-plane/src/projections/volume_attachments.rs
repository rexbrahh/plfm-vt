//! Volume attachments projection handler.
//!
//! Handles volume_attachment.created and volume_attachment.deleted events,
//! updating the volume_attachments_view table.

use async_trait::async_trait;
use plfm_events::{VolumeAttachmentCreatedPayload, VolumeAttachmentDeletedPayload};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for volume attachments.
pub struct VolumeAttachmentsProjection;

#[async_trait]
impl ProjectionHandler for VolumeAttachmentsProjection {
    fn name(&self) -> &'static str {
        "volume_attachments"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["volume_attachment.created", "volume_attachment.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "volume_attachment.created" => self.handle_created(tx, event).await,
            "volume_attachment.deleted" => self.handle_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl VolumeAttachmentsProjection {
    async fn handle_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: VolumeAttachmentCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            attachment_id = %payload.attachment_id,
            volume_id = %payload.volume_id,
            env_id = %payload.env_id,
            process_type = %payload.process_type,
            mount_path = %payload.mount_path,
            read_only = payload.read_only,
            "Inserting volume attachment into volume_attachments_view"
        );

        sqlx::query(
            r#"
            INSERT INTO volume_attachments_view (
                attachment_id,
                org_id,
                volume_id,
                app_id,
                env_id,
                process_type,
                mount_path,
                read_only,
                resource_version,
                created_at,
                updated_at,
                is_deleted
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $9, false)
            ON CONFLICT (attachment_id) DO UPDATE SET
                org_id = EXCLUDED.org_id,
                volume_id = EXCLUDED.volume_id,
                app_id = EXCLUDED.app_id,
                env_id = EXCLUDED.env_id,
                process_type = EXCLUDED.process_type,
                mount_path = EXCLUDED.mount_path,
                read_only = EXCLUDED.read_only,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.attachment_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.volume_id.to_string())
        .bind(payload.app_id.to_string())
        .bind(payload.env_id.to_string())
        .bind(&payload.process_type)
        .bind(&payload.mount_path)
        .bind(payload.read_only)
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
        let payload: VolumeAttachmentDeletedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            attachment_id = %payload.attachment_id,
            org_id = %payload.org_id,
            "Soft-deleting volume attachment in volume_attachments_view"
        );

        sqlx::query(
            r#"
            UPDATE volume_attachments_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE attachment_id = $1 AND org_id = $2
            "#,
        )
        .bind(payload.attachment_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
