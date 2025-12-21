use async_trait::async_trait;
use plfm_events::{EnvIpv4AddonDisabledPayload, EnvIpv4AddonEnabledPayload};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

pub struct EnvNetworkingProjection;

#[async_trait]
impl ProjectionHandler for EnvNetworkingProjection {
    fn name(&self) -> &'static str {
        "env_networking"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["env.ipv4_addon_enabled", "env.ipv4_addon_disabled"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "env.ipv4_addon_enabled" => self.handle_ipv4_enabled(tx, event).await,
            "env.ipv4_addon_disabled" => self.handle_ipv4_disabled(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl EnvNetworkingProjection {
    async fn handle_ipv4_enabled(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvIpv4AddonEnabledPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            env_id = %payload.env_id,
            ipv4_address = %payload.ipv4_address,
            "Enabling IPv4 add-on for environment"
        );

        sqlx::query(
            r#"
            INSERT INTO env_networking_view (
                env_id, org_id, app_id, ipv4_enabled, ipv4_address, ipv4_allocation_id,
                resource_version, updated_at
            )
            VALUES ($1, $2, $3, true, $4::INET, $5, 1, $6)
            ON CONFLICT (env_id) DO UPDATE SET
                ipv4_enabled = true,
                ipv4_address = $4::INET,
                ipv4_allocation_id = $5,
                resource_version = env_networking_view.resource_version + 1,
                updated_at = $6
            "#,
        )
        .bind(payload.env_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.app_id.to_string())
        .bind(&payload.ipv4_address)
        .bind(&payload.allocation_id)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_ipv4_disabled(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: EnvIpv4AddonDisabledPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(
            env_id = %payload.env_id,
            "Disabling IPv4 add-on for environment"
        );

        sqlx::query(
            r#"
            UPDATE env_networking_view
            SET ipv4_enabled = false,
                ipv4_address = NULL,
                ipv4_allocation_id = NULL,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE env_id = $1
            "#,
        )
        .bind(payload.env_id.to_string())
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
    fn ipv4_enabled_payload_roundtrip() {
        let json = r#"{
            "env_id": "env_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "org_id": "org_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "app_id": "app_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "allocation_id": "ipv4_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "ipv4_address": "203.0.113.10"
        }"#;

        let payload: EnvIpv4AddonEnabledPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.ipv4_address, "203.0.113.10");
    }
}
