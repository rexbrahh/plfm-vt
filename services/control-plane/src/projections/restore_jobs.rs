//! Restore jobs projection handler.
//!
//! Handles restore_job.created and restore_job.status_changed events, updating restore_jobs_view.

use async_trait::async_trait;
use plfm_events::{JobStatus, RestoreJobCreatedPayload, RestoreJobStatusChangedPayload};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for restore jobs.
pub struct RestoreJobsProjection;

#[async_trait]
impl ProjectionHandler for RestoreJobsProjection {
    fn name(&self) -> &'static str {
        "restore_jobs"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["restore_job.created", "restore_job.status_changed"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "restore_job.created" => self.handle_created(tx, event).await,
            "restore_job.status_changed" => self.handle_status_changed(tx, event).await,
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

impl RestoreJobsProjection {
    async fn handle_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: RestoreJobCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let status = job_status_label(payload.status);

        debug!(
            restore_id = %payload.restore_id,
            snapshot_id = %payload.snapshot_id,
            source_volume_id = %payload.source_volume_id,
            org_id = %payload.org_id,
            status = %status,
            "Inserting restore job into restore_jobs_view"
        );

        sqlx::query(
            r#"
            INSERT INTO restore_jobs_view (
                restore_id,
                org_id,
                snapshot_id,
                source_volume_id,
                status,
                new_volume_id,
                failed_reason,
                resource_version,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, NULL, NULL, 1, $6, $6)
            ON CONFLICT (restore_id) DO UPDATE SET
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.restore_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.snapshot_id.to_string())
        .bind(payload.source_volume_id.to_string())
        .bind(status)
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
        let payload: RestoreJobStatusChangedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let status = job_status_label(payload.status);

        debug!(
            restore_id = %payload.restore_id,
            org_id = %payload.org_id,
            status = %status,
            "Updating restore job in restore_jobs_view"
        );

        sqlx::query(
            r#"
            UPDATE restore_jobs_view
            SET status = $3,
                new_volume_id = $4,
                failed_reason = $5,
                resource_version = resource_version + 1,
                updated_at = $6
            WHERE restore_id = $1 AND org_id = $2
            "#,
        )
        .bind(payload.restore_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(status)
        .bind(payload.new_volume_id.as_ref().map(|id| id.to_string()))
        .bind(payload.failed_reason.as_deref())
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
