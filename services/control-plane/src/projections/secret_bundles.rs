//! Secret bundle projection handler.
//!
//! Handles secret_bundle.created and secret_bundle.version_set events,
//! updating the secret_bundles_view table.

use async_trait::async_trait;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for secret bundle metadata.
pub struct SecretBundlesProjection;

#[derive(Debug, Deserialize)]
struct SecretBundleCreatedPayload {
    #[allow(dead_code)]
    bundle_id: String,
    #[serde(default)]
    format: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SecretBundleVersionSetPayload {
    #[allow(dead_code)]
    bundle_id: String,
    version_id: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    data_hash: Option<String>,
}

#[async_trait]
impl ProjectionHandler for SecretBundlesProjection {
    fn name(&self) -> &'static str {
        "secret_bundles"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["secret_bundle.created", "secret_bundle.version_set"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "secret_bundle.created" => self.handle_created(tx, event).await,
            "secret_bundle.version_set" => self.handle_version_set(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl SecretBundlesProjection {
    async fn handle_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: SecretBundleCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.created event missing org_id".to_string(),
            )
        })?;
        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.created event missing app_id".to_string(),
            )
        })?;
        let env_id = event.env_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.created event missing env_id".to_string(),
            )
        })?;

        let format = payload
            .format
            .unwrap_or_else(|| "platform_env_v1".to_string());

        debug!(
            bundle_id = %event.aggregate_id,
            org_id = %org_id,
            app_id = %app_id,
            env_id = %env_id,
            format = %format,
            "Inserting secret bundle into secret_bundles_view"
        );

        sqlx::query(
            r#"
            INSERT INTO secret_bundles_view (
                bundle_id,
                org_id,
                app_id,
                env_id,
                format,
                current_version_id,
                current_data_hash,
                resource_version,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, NULL, NULL, 1, $6, $6)
            ON CONFLICT (bundle_id) DO UPDATE SET
                org_id = EXCLUDED.org_id,
                app_id = EXCLUDED.app_id,
                env_id = EXCLUDED.env_id,
                format = EXCLUDED.format,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(app_id)
        .bind(env_id)
        .bind(&format)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_version_set(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: SecretBundleVersionSetPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let org_id = event.org_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.version_set event missing org_id".to_string(),
            )
        })?;
        let app_id = event.app_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.version_set event missing app_id".to_string(),
            )
        })?;
        let env_id = event.env_id.as_ref().ok_or_else(|| {
            ProjectionError::InvalidPayload(
                "secret_bundle.version_set event missing env_id".to_string(),
            )
        })?;

        let format = payload
            .format
            .unwrap_or_else(|| "platform_env_v1".to_string());

        debug!(
            bundle_id = %event.aggregate_id,
            version_id = %payload.version_id,
            "Updating secret bundle current version"
        );

        sqlx::query(
            r#"
            INSERT INTO secret_bundles_view (
                bundle_id,
                org_id,
                app_id,
                env_id,
                format,
                current_version_id,
                current_data_hash,
                resource_version,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8, $8)
            ON CONFLICT (bundle_id) DO UPDATE SET
                current_version_id = EXCLUDED.current_version_id,
                current_data_hash = EXCLUDED.current_data_hash,
                format = EXCLUDED.format,
                resource_version = secret_bundles_view.resource_version + 1,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(&event.aggregate_id)
        .bind(org_id)
        .bind(app_id)
        .bind(env_id)
        .bind(&format)
        .bind(&payload.version_id)
        .bind(payload.data_hash.as_deref())
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
    fn created_payload_minimal_deserialization() {
        let json = r#"{"bundle_id":"sb_01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#;
        let payload: SecretBundleCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.bundle_id, "sb_01ARZ3NDEKTSV4RRFFQ69G5FAV");
    }

    #[test]
    fn version_set_payload_deserialization() {
        let json = r#"{
            "bundle_id":"sb_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "version_id":"sv_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "format":"platform_env_v1",
            "data_hash":"deadbeef"
        }"#;
        let payload: SecretBundleVersionSetPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.version_id, "sv_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(payload.data_hash.as_deref(), Some("deadbeef"));
    }
}
