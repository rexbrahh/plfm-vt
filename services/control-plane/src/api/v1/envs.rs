//! Environment API endpoints.
//!
//! Provides CRUD operations for environments within applications.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::AggregateType;
use plfm_id::{AppId, EnvId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create env routes.
///
/// Envs are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/envs
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_env))
        .route("/", get(list_envs))
        .route("/{env_id}", get(get_env))
}

/// Create env status routes.
///
/// Status is nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/status
pub fn status_routes() -> Router<AppState> {
    Router::new().route("/", get(get_status))
}

/// Create env scale routes.
///
/// Scale is nested under orgs/apps/envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
pub fn scale_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_scale))
        .route("/", put(update_scale))
        // Backwards-compatible dev endpoint (deprecated; use PUT).
        .route("/", post(update_scale))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new environment.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateEnvRequest {
    /// Environment name (unique within app, e.g., "production", "staging").
    pub name: String,
}

/// Response for a single environment.
#[derive(Debug, Serialize)]
pub struct EnvResponse {
    /// Environment ID.
    pub id: String,

    /// Application ID.
    pub app_id: String,

    /// Organization ID.
    pub org_id: String,

    /// Environment name.
    pub name: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the env was created.
    pub created_at: DateTime<Utc>,

    /// When the env was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing environments.
#[derive(Debug, Serialize)]
pub struct ListEnvsResponse {
    /// List of environments.
    pub items: Vec<EnvResponse>,

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing environments.
#[derive(Debug, Deserialize)]
pub struct ListEnvsQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as an env_id.
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProcessScale {
    pub process_type: String,
    pub desired: i32,
}

#[derive(Debug, Serialize)]
pub struct ScaleState {
    pub env_id: String,
    pub processes: Vec<ProcessScale>,
    pub updated_at: DateTime<Utc>,
    pub resource_version: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ScaleUpdateRequest {
    pub processes: Vec<ProcessScale>,
    pub expected_version: i32,
}

/// Response for environment status (desired vs current state).
#[derive(Debug, Serialize)]
pub struct EnvStatusResponse {
    /// Environment ID.
    pub env_id: String,

    /// Environment name.
    pub env_name: String,

    /// App ID.
    pub app_id: String,

    /// App name.
    pub app_name: String,

    /// Current live release ID (most recently completed deploy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_release_id: Option<String>,

    /// Desired release ID (target of latest deploy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desired_release_id: Option<String>,

    /// Whether current matches desired.
    pub release_synced: bool,

    /// Instance counts.
    pub instances: InstanceCounts,

    /// Route/endpoint summary.
    pub routes: Vec<RouteStatus>,

    /// Last reconciliation timestamp (most recent instance status update).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reconcile_at: Option<DateTime<Utc>>,

    /// Last error (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,

    /// Overall status (healthy, degraded, failed).
    pub status: String,
}

/// Instance count summary.
#[derive(Debug, Serialize)]
pub struct InstanceCounts {
    /// Desired instance count (sum of all process types).
    pub desired: i32,

    /// Ready instances.
    pub ready: i32,

    /// Booting instances.
    pub booting: i32,

    /// Draining instances.
    pub draining: i32,

    /// Failed instances.
    pub failed: i32,
}

/// Route/endpoint status.
#[derive(Debug, Serialize)]
pub struct RouteStatus {
    /// Route ID.
    pub id: String,

    /// Hostname.
    pub hostname: String,

    /// Target port.
    pub target_port: i32,

    /// Status (active, pending, error).
    pub status: String,

    /// Backend count (number of ready instances for this route's process type).
    pub backend_count: i32,
}

// =============================================================================
// Handlers
// =============================================================================

async fn load_scale_state(
    state: &AppState,
    request_id: &str,
    org_id: &OrgId,
    app_id: &AppId,
    env_id: &EnvId,
) -> Result<ScaleState, ApiError> {
    let env_updated_at: DateTime<Utc> = sqlx::query_scalar(
        r#"
        SELECT updated_at
        FROM envs_view
        WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            env_id = %env_id,
            "Failed to load env"
        );
        ApiError::internal("internal_error", "Failed to get scale")
            .with_request_id(request_id.to_string())
    })?
    .ok_or_else(|| {
        ApiError::not_found("env_not_found", format!("Environment {} not found", env_id))
            .with_request_id(request_id.to_string())
    })?;

    let rows = sqlx::query_as::<_, ScaleRow>(
        r#"
        SELECT process_type, desired_replicas, resource_version, updated_at
        FROM env_scale_view
        WHERE env_id = $1 AND org_id = $2 AND app_id = $3
        ORDER BY process_type ASC
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            env_id = %env_id,
            "Failed to load scale"
        );
        ApiError::internal("internal_error", "Failed to get scale")
            .with_request_id(request_id.to_string())
    })?;

    let mut resource_version = 0;
    let mut updated_at = env_updated_at;
    let mut processes = Vec::with_capacity(rows.len());
    for row in rows {
        resource_version = resource_version.max(row.resource_version);
        updated_at = updated_at.max(row.updated_at);
        processes.push(ProcessScale {
            process_type: row.process_type,
            desired: row.desired_replicas,
        });
    }

    Ok(ScaleState {
        env_id: env_id.to_string(),
        processes,
        updated_at,
        resource_version,
    })
}

/// Create a new environment.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs
async fn create_env(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
    Json(req): Json<CreateEnvRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "envs.create";

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    // Validate app_id format
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    // Get app and verify it exists
    let app_row = sqlx::query_as::<_, AppInfoRow>(
        "SELECT app_id, org_id FROM apps_view WHERE app_id = $1 AND NOT is_deleted",
    )
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check app existence");
        ApiError::internal("internal_error", "Failed to verify application")
            .with_request_id(request_id.clone())
    })?;

    let app_row = app_row.ok_or_else(|| {
        ApiError::not_found("app_not_found", format!("Application {} not found", app_id))
            .with_request_id(request_id.clone())
    })?;

    let app_org_id: OrgId = app_row.org_id.parse().map_err(|_| {
        ApiError::internal("internal_error", "Invalid org_id in database")
            .with_request_id(request_id.clone())
    })?;

    if app_org_id != org_id {
        return Err(ApiError::not_found(
            "app_not_found",
            format!(
                "Application {} not found in organization {}",
                app_id, org_id
            ),
        )
        .with_request_id(request_id.clone()));
    }

    // Validate name
    if req.name.is_empty() {
        return Err(
            ApiError::bad_request("invalid_name", "Environment name cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    if req.name.len() > 50 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Environment name cannot exceed 50 characters",
        )
        .with_request_id(request_id.clone()));
    }

    // Validate name format (lowercase alphanumeric and hyphens)
    if !req
        .name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Environment name must contain only lowercase letters, numbers, and hyphens",
        )
        .with_request_id(request_id.clone()));
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "body": &req
            });
            idempotency::request_hash(endpoint_name, &hash_input)
                .map(|hash| (key.to_string(), hash))
        })
        .transpose()
        .map_err(|e| e.with_request_id(request_id.clone()))?;

    if let Some((key, hash)) = request_hash.as_ref() {
        if let Some((status, body)) = idempotency::check(
            &state,
            &org_scope,
            &actor_id,
            endpoint_name,
            key,
            hash,
            &request_id,
        )
        .await?
        {
            return Ok(
                (status, Json(body.unwrap_or_else(|| serde_json::json!({})))).into_response(),
            );
        }
    }

    // Check for duplicate name within app
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE app_id = $1 AND name = $2 AND NOT is_deleted)",
    )
    .bind(app_id.to_string())
    .bind(&req.name)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check env name uniqueness");
        ApiError::internal("internal_error", "Failed to verify environment name")
            .with_request_id(request_id.clone())
    })?;

    if name_exists {
        return Err(ApiError::conflict(
            "env_name_exists",
            format!(
                "Environment '{}' already exists in this application",
                req.name
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let env_id = EnvId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id.to_string(),
        aggregate_seq: 1,
        event_type: "env.created".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
        env_id: Some(env_id),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "name": req.name
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create env");
        ApiError::internal("internal_error", "Failed to create environment")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint("envs", event_id.value(), crate::api::projection_wait_timeout())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, EnvRow>(
        r#"
        SELECT env_id, app_id, org_id, name, resource_version, created_at, updated_at
        FROM envs_view
        WHERE env_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load env");
        ApiError::internal("internal_error", "Failed to load environment")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Environment was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = EnvResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create environment")
                .with_request_id(request_id.clone())
        })?;

        let _ = idempotency::store(
            &state,
            idempotency::StoreIdempotencyParams {
                org_scope: &org_scope,
                actor_id: &actor_id,
                endpoint_name,
                idempotency_key: &key,
                request_hash: &hash,
                status: StatusCode::OK,
                body: Some(body),
            },
            &request_id,
        )
        .await;
    }

    Ok((StatusCode::OK, Json(response)).into_response())
}

/// List environments in an application.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs
async fn list_envs(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
    Query(query): Query<ListEnvsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    // Validate IDs
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match query.cursor.as_deref() {
        Some(raw) => {
            let _: EnvId = raw.parse().map_err(|_| {
                ApiError::bad_request("invalid_cursor", "Invalid cursor format")
                    .with_request_id(request_id.clone())
            })?;
            Some(raw.to_string())
        }
        None => None,
    };

    // Query the envs_view table (stable ordering by env_id)
    let rows = sqlx::query_as::<_, EnvRow>(
        r#"
        SELECT env_id, app_id, org_id, name, resource_version, created_at, updated_at
        FROM envs_view
        WHERE org_id = $1 AND app_id = $2 AND NOT is_deleted
          AND ($3::TEXT IS NULL OR env_id > $3)
        ORDER BY env_id ASC
        LIMIT $4
        "#,
    )
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list envs");
        ApiError::internal("internal_error", "Failed to list environments")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<EnvResponse> = rows.into_iter().map(EnvResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|e| e.id.clone())
    } else {
        None
    };

    Ok(Json(ListEnvsResponse { items, next_cursor }))
}

/// Get desired scale for an environment.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
async fn get_scale(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let app_id_typed: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let env_id_typed: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;

    Ok(Json(
        load_scale_state(
            &state,
            &request_id,
            &org_id_typed,
            &app_id_typed,
            &env_id_typed,
        )
        .await?,
    ))
}

/// Set desired scale for an environment.
///
/// PUT /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
async fn update_scale(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(mut req): Json<ScaleUpdateRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "envs.set_scale";

    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let app_id_typed: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let env_id_typed: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    if req.expected_version < 0 {
        return Err(ApiError::bad_request(
            "invalid_expected_version",
            "expected_version must be >= 0",
        )
        .with_request_id(request_id));
    }

    if req.processes.is_empty() {
        return Err(
            ApiError::bad_request("invalid_processes", "processes cannot be empty")
                .with_request_id(request_id),
        );
    }

    for process in &req.processes {
        if process.process_type.trim().is_empty() {
            return Err(ApiError::bad_request(
                "invalid_process_type",
                "process_type cannot be empty",
            )
            .with_request_id(request_id.clone()));
        }
        if process.desired < 0 {
            return Err(
                ApiError::bad_request("invalid_desired", "desired must be >= 0")
                    .with_request_id(request_id.clone()),
            );
        }
    }

    req.processes
        .sort_by(|a, b| a.process_type.cmp(&b.process_type));
    for pair in req.processes.windows(2) {
        if let [a, b] = pair {
            if a.process_type == b.process_type {
                return Err(ApiError::bad_request(
                    "duplicate_process_type",
                    "process_type values must be unique",
                )
                .with_request_id(request_id));
            }
        }
    }

    let org_scope = org_id_typed.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            idempotency::request_hash(endpoint_name, &req).map(|hash| (key.to_string(), hash))
        })
        .transpose()
        .map_err(|e| e.with_request_id(request_id.clone()))?;

    if let Some((key, hash)) = request_hash.as_ref() {
        if let Some((status, body)) = idempotency::check(
            &state,
            &org_scope,
            &actor_id,
            endpoint_name,
            key,
            hash,
            &request_id,
        )
        .await?
        {
            return Ok(
                (status, Json(body.unwrap_or_else(|| serde_json::json!({})))).into_response(),
            );
        }
    }

    let current = load_scale_state(
        &state,
        &request_id,
        &org_id_typed,
        &app_id_typed,
        &env_id_typed,
    )
    .await?;

    if req.expected_version != current.resource_version {
        return Err(
            ApiError::conflict("version_conflict", "Resource version mismatch")
                .with_request_id(request_id.clone()),
        );
    }

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Env, &env_id_typed.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to set scale")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let scales: Vec<serde_json::Value> = req
        .processes
        .iter()
        .map(|p| {
            serde_json::json!({
                "process_type": &p.process_type,
                "desired": p.desired
            })
        })
        .collect();

    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id_typed.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: "env.scale_set".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id_typed),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id_typed),
        env_id: Some(env_id_typed),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "env_id": env_id,
            "org_id": org_id,
            "app_id": app_id,
            "scales": scales
        }),
    };

    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to set scale");
        ApiError::internal("internal_error", "Failed to set scale")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "env_config",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let updated = load_scale_state(
        &state,
        &request_id,
        &org_id_typed,
        &app_id_typed,
        &env_id_typed,
    )
    .await?;

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&updated).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to set scale")
                .with_request_id(request_id.clone())
        })?;

        let _ = idempotency::store(
            &state,
            idempotency::StoreIdempotencyParams {
                org_scope: &org_scope,
                actor_id: &actor_id,
                endpoint_name,
                idempotency_key: &key,
                request_hash: &hash,
                status: StatusCode::OK,
                body: Some(body),
            },
            &request_id,
        )
        .await;
    }

    Ok((StatusCode::OK, Json(updated)).into_response())
}

/// Get a single environment by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}
async fn get_env(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    // Validate IDs
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    // Query the envs_view table
    let row = sqlx::query_as::<_, EnvRow>(
        r#"
        SELECT env_id, app_id, org_id, name, resource_version, created_at, updated_at
        FROM envs_view
        WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            env_id = %env_id,
            "Failed to get env"
        );
        ApiError::internal("internal_error", "Failed to get environment")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(EnvResponse::from(row))),
        None => Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.clone())),
    }
}

/// Get environment status (desired vs current state).
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/status
async fn get_status(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    // Validate IDs
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    // 1. Get env and app info
    let env_app_info = sqlx::query_as::<_, EnvAppInfoRow>(
        r#"
        SELECT e.env_id, e.name as env_name, e.app_id, a.name as app_name
        FROM envs_view e
        JOIN apps_view a ON e.app_id = a.app_id AND NOT a.is_deleted
        WHERE e.env_id = $1 AND e.org_id = $2 AND e.app_id = $3 AND NOT e.is_deleted
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            env_id = %env_id,
            "Failed to get env/app info"
        );
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("env_not_found", format!("Environment {} not found", env_id))
            .with_request_id(request_id.clone())
    })?;

    // 2. Get desired release (from env_desired_releases_view, pick any process type)
    let desired_release: Option<String> = sqlx::query_scalar(
        r#"
        SELECT release_id
        FROM env_desired_releases_view
        WHERE env_id = $1
        LIMIT 1
        "#,
    )
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get desired release");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // 3. Get current release (from most recent completed deploy)
    let current_release: Option<String> = sqlx::query_scalar(
        r#"
        SELECT release_id
        FROM deploys_view
        WHERE env_id = $1 AND status = 'completed'
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get current release");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // 4. Get desired instance count (sum from env_scale_view)
    let desired_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(desired_replicas), 0)
        FROM env_scale_view
        WHERE env_id = $1
        "#,
    )
    .bind(env_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get desired count");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // 5. Get instance counts by status (join desired + status views)
    let status_counts = sqlx::query_as::<_, InstanceStatusCountRow>(
        r#"
        SELECT
            COALESCE(s.status, d.desired_state) as status,
            COUNT(*) as count
        FROM instances_desired_view d
        LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
        WHERE d.env_id = $1
        GROUP BY COALESCE(s.status, d.desired_state)
        "#,
    )
    .bind(env_id.to_string())
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get instance counts");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // Parse instance counts
    let mut ready = 0i32;
    let mut booting = 0i32;
    let mut draining = 0i32;
    let mut failed = 0i32;

    for row in status_counts {
        let count = row.count as i32;
        match row.status.as_str() {
            "running" => ready += count,
            "booting" | "starting" | "pending" => booting += count,
            "draining" | "stopping" => draining += count,
            "failed" | "crashed" | "error" => failed += count,
            _ => {} // Other statuses ignored
        }
    }

    // 6. Get routes with backend counts
    let route_rows = sqlx::query_as::<_, RouteInfoRow>(
        r#"
        SELECT
            r.route_id,
            r.hostname,
            r.backend_port,
            r.backend_process_type,
            (
                SELECT COUNT(*)
                FROM instances_desired_view d
                LEFT JOIN instances_status_view s ON d.instance_id = s.instance_id
                WHERE d.env_id = r.env_id
                  AND d.process_type = r.backend_process_type
                  AND COALESCE(s.status, d.desired_state) = 'running'
            ) as backend_count
        FROM routes_view r
        WHERE r.env_id = $1 AND NOT r.is_deleted
        ORDER BY r.hostname
        "#,
    )
    .bind(env_id.to_string())
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get routes");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    let routes: Vec<RouteStatus> = route_rows
        .into_iter()
        .map(|r| {
            let status = if r.backend_count > 0 {
                "active"
            } else {
                "pending"
            };
            RouteStatus {
                id: r.route_id,
                hostname: r.hostname,
                target_port: r.backend_port,
                status: status.to_string(),
                backend_count: r.backend_count as i32,
            }
        })
        .collect();

    // 7. Get last reconcile time (most recent instance status update)
    let last_reconcile: Option<DateTime<Utc>> = sqlx::query_scalar(
        r#"
        SELECT MAX(s.updated_at)
        FROM instances_status_view s
        WHERE s.env_id = $1
        "#,
    )
    .bind(env_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get last reconcile");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // 8. Get last error (from most recent failed deploy or instance)
    let last_error: Option<String> = sqlx::query_scalar(
        r#"
        SELECT failed_reason
        FROM deploys_view
        WHERE env_id = $1 AND status = 'failed' AND failed_reason IS NOT NULL
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get last error");
        ApiError::internal("internal_error", "Failed to get environment status")
            .with_request_id(request_id.clone())
    })?;

    // Calculate release_synced
    let release_synced = match (&current_release, &desired_release) {
        (Some(c), Some(d)) => c == d,
        (None, None) => true,
        _ => false,
    };

    // Determine overall status
    let overall_status = if failed > 0 {
        "failed"
    } else if ready < desired_count as i32 || !release_synced {
        "degraded"
    } else {
        "healthy"
    };

    let response = EnvStatusResponse {
        env_id: env_app_info.env_id,
        env_name: env_app_info.env_name,
        app_id: env_app_info.app_id,
        app_name: env_app_info.app_name,
        current_release_id: current_release,
        desired_release_id: desired_release,
        release_synced,
        instances: InstanceCounts {
            desired: desired_count as i32,
            ready,
            booting,
            draining,
            failed,
        },
        routes,
        last_reconcile_at: last_reconcile,
        last_error,
        status: overall_status.to_string(),
    };

    Ok(Json(response))
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Minimal app info for lookups.
struct AppInfoRow {
    #[allow(dead_code)]
    app_id: String,
    org_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AppInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
        })
    }
}

/// Row from envs_view table.
struct EnvRow {
    env_id: String,
    app_id: String,
    org_id: String,
    name: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for EnvRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            env_id: row.try_get("env_id")?,
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<EnvRow> for EnvResponse {
    fn from(row: EnvRow) -> Self {
        Self {
            id: row.env_id,
            app_id: row.app_id,
            org_id: row.org_id,
            name: row.name,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

struct ScaleRow {
    process_type: String,
    desired_replicas: i32,
    resource_version: i32,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ScaleRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            process_type: row.try_get("process_type")?,
            desired_replicas: row.try_get("desired_replicas")?,
            resource_version: row.try_get("resource_version")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Row for env + app info join.
struct EnvAppInfoRow {
    env_id: String,
    env_name: String,
    app_id: String,
    app_name: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for EnvAppInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            env_id: row.try_get("env_id")?,
            env_name: row.try_get("env_name")?,
            app_id: row.try_get("app_id")?,
            app_name: row.try_get("app_name")?,
        })
    }
}

/// Row for instance status counts.
struct InstanceStatusCountRow {
    status: String,
    count: i64,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceStatusCountRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            status: row.try_get("status")?,
            count: row.try_get("count")?,
        })
    }
}

/// Row for route info with backend count.
struct RouteInfoRow {
    route_id: String,
    hostname: String,
    backend_port: i32,
    #[allow(dead_code)]
    backend_process_type: String,
    backend_count: i64,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for RouteInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            route_id: row.try_get("route_id")?,
            hostname: row.try_get("hostname")?,
            backend_port: row.try_get("backend_port")?,
            backend_process_type: row.try_get("backend_process_type")?,
            backend_count: row.try_get("backend_count")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_env_request_deserialization() {
        let json = r#"{"name": "production"}"#;
        let req: CreateEnvRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "production");
    }

    #[test]
    fn test_env_response_serialization() {
        let response = EnvResponse {
            id: "env_123".to_string(),
            app_id: "app_456".to_string(),
            org_id: "org_789".to_string(),
            name: "staging".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"env_123\""));
        assert!(json.contains("\"app_id\":\"app_456\""));
        assert!(json.contains("\"name\":\"staging\""));
    }

    #[test]
    fn test_env_status_response_serialization() {
        let response = EnvStatusResponse {
            env_id: "env_123".to_string(),
            env_name: "production".to_string(),
            app_id: "app_456".to_string(),
            app_name: "myapp".to_string(),
            current_release_id: Some("rel_abc".to_string()),
            desired_release_id: Some("rel_abc".to_string()),
            release_synced: true,
            instances: InstanceCounts {
                desired: 3,
                ready: 3,
                booting: 0,
                draining: 0,
                failed: 0,
            },
            routes: vec![RouteStatus {
                id: "route_123".to_string(),
                hostname: "myapp.example.com".to_string(),
                target_port: 8080,
                status: "active".to_string(),
                backend_count: 3,
            }],
            last_reconcile_at: Some(Utc::now()),
            last_error: None,
            status: "healthy".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"env_id\":\"env_123\""));
        assert!(json.contains("\"app_name\":\"myapp\""));
        assert!(json.contains("\"release_synced\":true"));
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"desired\":3"));
        assert!(json.contains("\"ready\":3"));
        assert!(json.contains("\"hostname\":\"myapp.example.com\""));
        // last_error should be omitted when None
        assert!(!json.contains("\"last_error\""));
    }

    #[test]
    fn test_env_status_degraded_when_not_synced() {
        // When desired != ready, status should be degraded
        let response = EnvStatusResponse {
            env_id: "env_123".to_string(),
            env_name: "production".to_string(),
            app_id: "app_456".to_string(),
            app_name: "myapp".to_string(),
            current_release_id: Some("rel_old".to_string()),
            desired_release_id: Some("rel_new".to_string()),
            release_synced: false,
            instances: InstanceCounts {
                desired: 3,
                ready: 2,
                booting: 1,
                draining: 0,
                failed: 0,
            },
            routes: vec![],
            last_reconcile_at: None,
            last_error: None,
            status: "degraded".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"release_synced\":false"));
        assert!(json.contains("\"status\":\"degraded\""));
    }
}
