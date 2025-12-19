//! Snapshots projection handler.
//!
//! Handles snapshot.created and snapshot.status_changed events, updating snapshots_view.

use async_trait::async_trait;
use plfm_events::{JobStatus, SnapshotCreatedPayload, SnapshotStatusChangedPayload};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for snapshots.
pub struct SnapshotsProjection;

#[async_trait]
impl ProjectionHandler for SnapshotsProjection {
    fn name(&self) -> &'static str {
        "snapshots"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["snapshot.created", "snapshot.status_changed"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "snapshot.created" => self.handle_created(tx, event).await,
            "snapshot.status_changed" => self.handle_status_changed(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

fn job_status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
    }
}

impl SnapshotsProjection {
    async fn handle_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: SnapshotCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let status = job_status_label(payload.status);

        debug!(
            snapshot_id = %payload.snapshot_id,
            volume_id = %payload.volume_id,
            org_id = %payload.org_id,
            status = %status,
            "Inserting snapshot into snapshots_view"
        );

        sqlx::query(
            r#"
            INSERT INTO snapshots_view (
                snapshot_id,
                org_id,
                volume_id,
                status,
                size_bytes,
                note,
                failed_reason,
                resource_version,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, NULL, $5, NULL, 1, $6, $6)
            ON CONFLICT (snapshot_id) DO UPDATE SET
                status = EXCLUDED.status,
                note = EXCLUDED.note,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.snapshot_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.volume_id.to_string())
        .bind(status)
        .bind(payload.note.as_deref())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_status_changed(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: SnapshotStatusChangedPayload =
            serde_json::from_value(event.payload.clone())
                .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let status = job_status_label(payload.status);

        debug!(
            snapshot_id = %payload.snapshot_id,
            volume_id = %payload.volume_id,
            org_id = %payload.org_id,
            status = %status,
            "Updating snapshot in snapshots_view"
        );

        sqlx::query(
            r#"
            UPDATE snapshots_view
            SET status = $4,
                size_bytes = $5,
                failed_reason = $6,
                resource_version = resource_version + 1,
                updated_at = $7
            WHERE snapshot_id = $1 AND org_id = $2 AND volume_id = $3
            "#,
        )
        .bind(payload.snapshot_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.volume_id.to_string())
        .bind(status)
        .bind(payload.size_bytes)
        .bind(payload.failed_reason.as_deref())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
