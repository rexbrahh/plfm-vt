//! Exec sessions projection handler.
//!
//! Handles exec_session.granted, exec_session.connected, exec_session.ended events,
//! updating the exec_sessions_view table.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use plfm_events::{
    ExecSessionConnectedPayload, ExecSessionEndedPayload, ExecSessionGrantedPayload,
};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for exec sessions.
pub struct ExecSessionsProjection;

#[async_trait]
impl ProjectionHandler for ExecSessionsProjection {
    fn name(&self) -> &'static str {
        "exec_sessions"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &[
            "exec_session.granted",
            "exec_session.connected",
            "exec_session.ended",
        ]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "exec_session.granted" => self.handle_granted(tx, event).await,
            "exec_session.connected" => self.handle_connected(tx, event).await,
            "exec_session.ended" => self.handle_ended(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, ProjectionError> {
    let dt = DateTime::parse_from_rfc3339(s)
        .map_err(|e| ProjectionError::InvalidPayload(format!("invalid timestamp: {e}")))?;
    Ok(dt.with_timezone(&Utc))
}

impl ExecSessionsProjection {
    async fn handle_granted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ExecSessionGrantedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let expires_at = parse_rfc3339(&payload.expires_at)?;
        let requested_command = serde_json::to_value(&payload.requested_command)
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            exec_session_id = %payload.exec_session_id,
            instance_id = %payload.instance_id,
            org_id = %payload.org_id,
            env_id = %payload.env_id,
            tty = payload.tty,
            "Inserting exec session into exec_sessions_view"
        );

        sqlx::query(
            r#"
            INSERT INTO exec_sessions_view (
                exec_session_id,
                org_id,
                env_id,
                instance_id,
                requested_command,
                tty,
                status,
                expires_at,
                resource_version,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'granted', $7, 1, $8, $8)
            ON CONFLICT (exec_session_id) DO UPDATE SET
                org_id = EXCLUDED.org_id,
                env_id = EXCLUDED.env_id,
                instance_id = EXCLUDED.instance_id,
                requested_command = EXCLUDED.requested_command,
                tty = EXCLUDED.tty,
                status = EXCLUDED.status,
                expires_at = EXCLUDED.expires_at,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.exec_session_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.env_id.to_string())
        .bind(payload.instance_id.to_string())
        .bind(&requested_command)
        .bind(payload.tty)
        .bind(expires_at)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_connected(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ExecSessionConnectedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let connected_at = parse_rfc3339(&payload.connected_at)?;

        debug!(
            exec_session_id = %payload.exec_session_id,
            instance_id = %payload.instance_id,
            org_id = %payload.org_id,
            "Updating exec session to connected"
        );

        sqlx::query(
            r#"
            UPDATE exec_sessions_view
            SET status = 'connected',
                connected_at = $2,
                resource_version = resource_version + 1,
                updated_at = $3
            WHERE exec_session_id = $1 AND org_id = $4
            "#,
        )
        .bind(payload.exec_session_id.to_string())
        .bind(connected_at)
        .bind(event.occurred_at)
        .bind(payload.org_id.to_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_ended(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: ExecSessionEndedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let ended_at = parse_rfc3339(&payload.ended_at)?;

        debug!(
            exec_session_id = %payload.exec_session_id,
            instance_id = %payload.instance_id,
            org_id = %payload.org_id,
            "Updating exec session to ended"
        );

        sqlx::query(
            r#"
            UPDATE exec_sessions_view
            SET status = 'ended',
                ended_at = $2,
                exit_code = $3,
                end_reason = $4,
                resource_version = resource_version + 1,
                updated_at = $5
            WHERE exec_session_id = $1 AND org_id = $6
            "#,
        )
        .bind(payload.exec_session_id.to_string())
        .bind(ended_at)
        .bind(payload.exit_code)
        .bind(payload.end_reason.as_deref())
        .bind(event.occurred_at)
        .bind(payload.org_id.to_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
