//! Routes projection handler.
//!
//! Handles route.created, route.updated, and route.deleted events,
//! updating the routes_view table.

use async_trait::async_trait;
use plfm_events::{
    RouteCreatedPayload, RouteDeletedPayload, RouteProtocolHint, RouteProxyProtocol,
    RouteUpdatedPayload,
};
use tracing::{debug, instrument};

use crate::db::EventRow;

use super::{ProjectionError, ProjectionHandler, ProjectionResult};

/// Projection handler for routes.
pub struct RoutesProjection;

#[async_trait]
impl ProjectionHandler for RoutesProjection {
    fn name(&self) -> &'static str {
        "routes"
    }

    fn event_types(&self) -> &'static [&'static str] {
        &["route.created", "route.updated", "route.deleted"]
    }

    #[instrument(skip(self, tx, event), fields(event_id = event.event_id, event_type = %event.event_type))]
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        match event.event_type.as_str() {
            "route.created" => self.handle_route_created(tx, event).await,
            "route.updated" => self.handle_route_updated(tx, event).await,
            "route.deleted" => self.handle_route_deleted(tx, event).await,
            _ => {
                debug!(event_type = %event.event_type, "Ignoring unknown event type");
                Ok(())
            }
        }
    }
}

impl RoutesProjection {
    async fn handle_route_created(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: RouteCreatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        let proxy_protocol = matches!(payload.proxy_protocol, RouteProxyProtocol::V2);
        let protocol_hint = match payload.protocol_hint {
            RouteProtocolHint::TlsPassthrough => "tls_passthrough",
            RouteProtocolHint::TcpRaw => "tcp_raw",
        };

        debug!(
            route_id = %payload.route_id,
            hostname = %payload.hostname,
            env_id = %payload.env_id,
            "Inserting route into routes_view"
        );

        sqlx::query(
            r#"
            INSERT INTO routes_view (
                route_id,
                org_id,
                app_id,
                env_id,
                hostname,
                listen_port,
                protocol_hint,
                backend_process_type,
                backend_port,
                proxy_protocol,
                ipv4_required,
                resource_version,
                created_at,
                updated_at,
                is_deleted
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 1, $12, $12, false)
            ON CONFLICT (route_id) DO UPDATE SET
                hostname = EXCLUDED.hostname,
                listen_port = EXCLUDED.listen_port,
                protocol_hint = EXCLUDED.protocol_hint,
                backend_process_type = EXCLUDED.backend_process_type,
                backend_port = EXCLUDED.backend_port,
                proxy_protocol = EXCLUDED.proxy_protocol,
                ipv4_required = EXCLUDED.ipv4_required,
                is_deleted = false,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(payload.route_id.to_string())
        .bind(payload.org_id.to_string())
        .bind(payload.app_id.to_string())
        .bind(payload.env_id.to_string())
        .bind(&payload.hostname)
        .bind(payload.listen_port)
        .bind(protocol_hint)
        .bind(&payload.backend_process_type)
        .bind(payload.backend_port)
        .bind(proxy_protocol)
        .bind(payload.ipv4_required)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_route_updated(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: RouteUpdatedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(route_id = %payload.route_id, "Updating route in routes_view");

        let proxy_protocol: Option<bool> = payload
            .proxy_protocol
            .map(|p| matches!(p, RouteProxyProtocol::V2));

        sqlx::query(
            r#"
            UPDATE routes_view
            SET backend_process_type = COALESCE($2, backend_process_type),
                backend_port = COALESCE($3, backend_port),
                proxy_protocol = COALESCE($4, proxy_protocol),
                ipv4_required = COALESCE($5, ipv4_required),
                resource_version = resource_version + 1,
                updated_at = $6
            WHERE route_id = $1 AND NOT is_deleted
            "#,
        )
        .bind(payload.route_id.to_string())
        .bind(payload.backend_process_type.as_deref())
        .bind(payload.backend_port)
        .bind(proxy_protocol)
        .bind(payload.ipv4_required)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn handle_route_deleted(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()> {
        let payload: RouteDeletedPayload = serde_json::from_value(event.payload.clone())
            .map_err(|e| ProjectionError::InvalidPayload(e.to_string()))?;

        debug!(route_id = %payload.route_id, "Soft-deleting route in routes_view");

        sqlx::query(
            r#"
            UPDATE routes_view
            SET is_deleted = true,
                resource_version = resource_version + 1,
                updated_at = $2
            WHERE route_id = $1
            "#,
        )
        .bind(payload.route_id.to_string())
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
    fn route_created_payload_roundtrip() {
        let json = r#"{
            "route_id": "rt_01HV4Z4NYPLTRS0JTUA8XDME5F",
            "org_id": "org_01HV4Z2WQXKJNM8GPQY6VBKC3D",
            "app_id": "app_01HV4Z3MXNKPQR9HSTZ7WCLD4E",
            "env_id": "env_01HV4Z3MXNKPQR9HSTZ7WCLD4E",
            "hostname": "example.com",
            "listen_port": 443,
            "protocol_hint": "tls_passthrough",
            "backend_process_type": "web",
            "backend_port": 8080,
            "proxy_protocol": "off",
            "backend_expects_proxy_protocol": false,
            "ipv4_required": false
        }"#;

        let payload: RouteCreatedPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.hostname, "example.com");
        assert!(matches!(payload.proxy_protocol, RouteProxyProtocol::Off));
    }
}
