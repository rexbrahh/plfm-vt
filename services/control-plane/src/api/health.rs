//! Health check endpoints.
//!
//! These endpoints are used by load balancers and orchestration systems
//! to determine if the service is healthy and ready to receive traffic.

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use chrono::Utc;
use serde::Serialize;

/// Health check response.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
pub struct HealthResponse {
    /// Service status: "ok" or "degraded".
    pub status: &'static str,

    /// Service name.
    pub service: &'static str,

    /// Service version.
    pub version: &'static str,

    /// Current timestamp (ISO 8601).
    pub timestamp: String,

    /// Detailed component health (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<ComponentHealth>,
}

/// Component health details.
#[derive(Debug, Serialize)]
pub struct ComponentHealth {
    /// Database connection status.
    pub database: ComponentStatus,

    /// Event log status.
    pub event_log: ComponentStatus,
}

/// Individual component status.
#[derive(Debug, Serialize)]
pub struct ComponentStatus {
    /// Status: "ok", "degraded", or "unavailable".
    pub status: &'static str,

    /// Optional message with details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Create health check routes.
pub fn routes() -> Router {
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
        status: "ok",
        service: "control-plane",
        version: env!("CARGO_PKG_VERSION"),
        timestamp: Utc::now().to_rfc3339(),
        components: None,
    })
}

/// Readiness check - is the service ready to receive traffic?
///
/// This checks that all critical dependencies are available.
/// Returns 503 if the service is not ready.
async fn readyz() -> impl IntoResponse {
    // TODO: Actually check database and other dependencies
    let db_ok = true; // Placeholder
    let event_log_ok = true; // Placeholder

    let components = ComponentHealth {
        database: ComponentStatus {
            status: if db_ok { "ok" } else { "unavailable" },
            message: None,
        },
        event_log: ComponentStatus {
            status: if event_log_ok { "ok" } else { "unavailable" },
            message: None,
        },
    };

    let all_ok = db_ok && event_log_ok;

    let response = HealthResponse {
        status: if all_ok { "ok" } else { "degraded" },
        service: "control-plane",
        version: env!("CARGO_PKG_VERSION"),
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
    use axum::{body::Body, http::Request};
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn test_healthz_returns_ok() {
        let app = routes();

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_livez_returns_ok() {
        let app = routes();

        let response = app
            .oneshot(Request::builder().uri("/livez").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_includes_components() {
        let app = routes();

        let response = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();

        assert!(health.components.is_some());
        assert_eq!(health.status, "ok");
    }
}
