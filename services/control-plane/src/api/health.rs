//! Health check endpoints.
//!
//! These endpoints are used by load balancers and orchestration systems
//! to determine if the service is healthy and ready to receive traffic.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use chrono::Utc;
use serde::Serialize;

use crate::state::AppState;

/// Health check response.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct HealthResponse {
    /// Service status: "ok" or "degraded".
    pub status: String,

    /// Service name.
    pub service: String,

    /// Service version.
    pub version: String,

    /// Current timestamp (ISO 8601).
    pub timestamp: String,

    /// Detailed component health (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<ComponentHealth>,
}

/// Component health details.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct ComponentHealth {
    /// Database connection status.
    pub database: ComponentStatus,

    /// Event log status.
    pub event_log: ComponentStatus,
}

/// Individual component status.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct ComponentStatus {
    /// Status: "ok", "degraded", or "unavailable".
    pub status: String,

    /// Optional message with details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Create health check routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/livez", get(livez))
}

/// Basic health check - is the service running?
///
/// This is a simple liveness probe that returns 200 if the server is up.
/// It does not check dependencies.
async fn healthz() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        service: "control-plane".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now().to_rfc3339(),
        components: None,
    })
}

/// Readiness check - is the service ready to receive traffic?
///
/// This checks that all critical dependencies are available.
/// Returns 503 if the service is not ready.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    // Check database connectivity
    let db_result = state.db().health_check().await;
    let db_ok = db_result.is_ok();
    let db_message = db_result.err().map(|e| e.to_string());

    // Event log uses the same database, so if DB is ok, event log is ok
    let event_log_ok = db_ok;

    let components = ComponentHealth {
        database: ComponentStatus {
            status: if db_ok { "ok" } else { "unavailable" }.to_string(),
            message: db_message.clone(),
        },
        event_log: ComponentStatus {
            status: if event_log_ok { "ok" } else { "unavailable" }.to_string(),
            message: if event_log_ok { None } else { db_message },
        },
    };

    let all_ok = db_ok && event_log_ok;

    let response = HealthResponse {
        status: if all_ok { "ok" } else { "degraded" }.to_string(),
        service: "control-plane".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now().to_rfc3339(),
        components: Some(components),
    };

    if all_ok {
        (StatusCode::OK, Json(response))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(response))
    }
}

/// Liveness check - is the service alive?
///
/// This is a minimal check for Kubernetes liveness probes.
/// Returns 200 with minimal body for efficiency.
async fn livez() -> impl IntoResponse {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_healthz_returns_ok() {
        // healthz doesn't need state, but we need to provide it for the router
        // For unit tests, we test the handler directly without state
        let response = healthz().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_livez_returns_ok() {
        let response = livez().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // Integration tests for readyz would require a database connection
    // Those belong in the integration test suite
}
