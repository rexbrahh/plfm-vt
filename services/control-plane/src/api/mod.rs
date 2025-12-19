//! HTTP API handlers and routing.

pub mod authz;
pub mod error;
mod health;
pub mod idempotency;
pub mod request_context;
mod v1;

use axum::{
    http::{header, Method},
    Router,
};
use plfm_id::RequestId as PlfmRequestId;
use tower_http::{
    cors::{Any, CorsLayer},
    request_id::{
        MakeRequestId, PropagateRequestIdLayer, RequestId as TowerRequestId, SetRequestIdLayer,
    },
    trace::TraceLayer,
};

use crate::state::AppState;

#[derive(Clone, Copy)]
struct MakePlfmRequestId;

impl MakeRequestId for MakePlfmRequestId {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<TowerRequestId> {
        let request_id = PlfmRequestId::new().to_string();
        let header_value = axum::http::HeaderValue::from_str(&request_id).ok()?;
        Some(TowerRequestId::new(header_value))
    }
}

/// Create the main API router with all routes and middleware.
pub fn create_router(state: AppState) -> Router {
    // CORS configuration
    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_origin(Any);

    let request_id_header = header::HeaderName::from_static("x-request-id");
    let set_request_id = SetRequestIdLayer::new(request_id_header.clone(), MakePlfmRequestId);
    let propagate_request_id = PropagateRequestIdLayer::new(request_id_header);

    Router::new()
        // Health endpoints (no auth required)
        .nest("/", health::routes())
        // API v1 routes
        .nest("/v1", v1::routes())
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(propagate_request_id)
        .layer(set_request_id)
        .layer(cors)
        // Application state
        .with_state(state)
}
