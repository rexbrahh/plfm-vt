//! Debug and introspection endpoints.
//!
//! These routes are intended for development and operator debugging.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projections", get(list_projections))
        .route(
            "/projections/{projection_name}/reset",
            post(reset_projection),
        )
        .route("/idempotency/cleanup", post(cleanup_idempotency))
}

#[derive(Debug, Serialize)]
struct ProjectionStatus {
    projection_name: String,
    last_applied_event_id: i64,
    updated_at: DateTime<Utc>,
    lag: i64,
}

#[derive(Debug, Serialize)]
struct ProjectionsResponse {
    items: Vec<ProjectionStatus>,
}

async fn list_projections(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;
    let projection_store = state.db().projection_store();

    let checkpoints = projection_store.list_checkpoints().await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list projections");
        ApiError::internal("internal_error", "Failed to list projections")
            .with_request_id(request_id.clone())
    })?;

    let lag = projection_store.calculate_lag().await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to calculate projection lag");
        ApiError::internal("internal_error", "Failed to list projections")
            .with_request_id(request_id.clone())
    })?;

    let lag_by_name: HashMap<String, i64> = lag.into_iter().collect();
    let items = checkpoints
        .into_iter()
        .map(|checkpoint| ProjectionStatus {
            lag: lag_by_name
                .get(&checkpoint.projection_name)
                .copied()
                .unwrap_or(0),
            projection_name: checkpoint.projection_name,
            last_applied_event_id: checkpoint.last_applied_event_id,
            updated_at: checkpoint.updated_at,
        })
        .collect();

    Ok(Json(ProjectionsResponse { items }))
}

async fn reset_projection(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(projection_name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;
    state
        .db()
        .projection_store()
        .reset_checkpoint(&projection_name)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                projection_name = %projection_name,
                "Failed to reset projection checkpoint"
            );
            ApiError::internal("internal_error", "Failed to reset projection checkpoint")
                .with_request_id(request_id.clone())
        })?;

    Ok((StatusCode::OK, Json(serde_json::json!({ "ok": true }))))
}

#[derive(Debug, serde::Deserialize)]
struct CleanupIdempotencyQuery {
    #[serde(default)]
    max_age_hours: Option<i32>,
}

async fn cleanup_idempotency(
    State(state): State<AppState>,
    ctx: RequestContext,
    Query(query): Query<CleanupIdempotencyQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;
    let max_age_hours = query.max_age_hours.unwrap_or(48).max(24);

    let rows_deleted = state
        .db()
        .idempotency_store()
        .cleanup_expired(max_age_hours)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                max_age_hours = max_age_hours,
                "Failed to cleanup idempotency records"
            );
            ApiError::internal("internal_error", "Failed to cleanup idempotency records")
                .with_request_id(request_id.clone())
        })?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "ok": true, "rows_deleted": rows_deleted })),
    ))
}
