//! HTTP API handlers and routing.

pub mod error;
mod health;
mod v1;

use axum::{
    http::{header, Method},
    Router,
};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::state::AppState;

/// Create the main API router with all routes and middleware.
pub fn create_router(state: AppState) -> Router {
    // CORS configuration
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_origin(Any);

    Router::new()
        // Health endpoints (no auth required)
        .nest("/", health::routes())
        // API v1 routes
        .nest("/v1", v1::routes())
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        // Application state
        .with_state(state)
}
