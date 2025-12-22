//! Application API endpoints.
//!
//! Provides CRUD operations for applications within organizations.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{event_types, AggregateType};
use plfm_id::{AppId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create app routes.
///
/// Apps are nested under orgs: /v1/orgs/{org_id}/apps
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_app))
        .route("/", get(list_apps))
        .route("/{app_id}", patch(update_app))
        .route("/{app_id}", delete(delete_app))
        .route("/{app_id}", get(get_app))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new application.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateAppRequest {
    /// Application name (unique within org).
    pub name: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateAppRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub expected_version: i32,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub ok: bool,
}

/// Response for a single application.
#[derive(Debug, Serialize)]
pub struct AppResponse {
    /// Application ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// Application name.
    pub name: String,

    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the app was created.
    pub created_at: DateTime<Utc>,

    /// When the app was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing applications.
#[derive(Debug, Serialize)]
pub struct ListAppsResponse {
    /// List of applications.
    pub items: Vec<AppResponse>,

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing applications.
#[derive(Debug, Deserialize)]
pub struct ListAppsQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as an app_id.
    pub cursor: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new application.
///
/// POST /v1/orgs/{org_id}/apps
async fn create_app(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Json(req): Json<CreateAppRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "apps.create";

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    // Validate name
    if req.name.is_empty() {
        return Err(
            ApiError::bad_request("invalid_name", "Application name cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    if req.name.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Application name cannot exceed 100 characters",
        )
        .with_request_id(request_id.clone()));
    }

    let org_scope = org_id.to_string();
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

    // Validate org exists
    let org_exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM orgs_view WHERE org_id = $1)")
            .bind(org_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check org existence");
                ApiError::internal("internal_error", "Failed to verify organization")
                    .with_request_id(request_id.clone())
            })?;

    if !org_exists {
        return Err(ApiError::not_found(
            "org_not_found",
            format!("Organization {} not found", org_id),
        )
        .with_request_id(request_id.clone()));
    }

    // Check for duplicate name within org
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM apps_view WHERE org_id = $1 AND name = $2 AND NOT is_deleted)",
    )
    .bind(org_id.to_string())
    .bind(&req.name)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check app name uniqueness");
        ApiError::internal("internal_error", "Failed to verify application name")
            .with_request_id(request_id.clone())
    })?;

    if name_exists {
        return Err(ApiError::conflict(
            "app_name_exists",
            format!(
                "Application '{}' already exists in this organization",
                req.name
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let app_id = AppId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::App,
        aggregate_id: app_id.to_string(),
        aggregate_seq: 1,
        event_type: "app.created".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "app_id": app_id.to_string(),
            "org_id": org_id.to_string(),
            "name": req.name,
            "description": req.description
        }),
        ..Default::default()
    };

    // Append the event
    let event_store = state.db().event_store();
    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create app");
        ApiError::internal("internal_error", "Failed to create application")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "apps",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE app_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load app");
        ApiError::internal("internal_error", "Failed to load application")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Application was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = AppResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create application")
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

async fn update_app(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
    Json(req): Json<UpdateAppRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "apps.update";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    if req.expected_version < 0 {
        return Err(ApiError::bad_request(
            "invalid_expected_version",
            "expected_version must be >= 0",
        )
        .with_request_id(request_id.clone()));
    }

    if req.name.is_none() && req.description.is_none() {
        return Err(
            ApiError::bad_request("invalid_update", "No updatable fields provided")
                .with_request_id(request_id.clone()),
        );
    }

    if let Some(name) = req.name.as_ref() {
        if name.is_empty() {
            return Err(
                ApiError::bad_request("invalid_name", "Application name cannot be empty")
                    .with_request_id(request_id.clone()),
            );
        }
        if name.len() > 100 {
            return Err(ApiError::bad_request(
                "invalid_name",
                "Application name cannot exceed 100 characters",
            )
            .with_request_id(request_id.clone()));
        }
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "org_id": org_scope.clone(),
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

    let current = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE app_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, app_id = %app_id, "Failed to load app");
        ApiError::internal("internal_error", "Failed to update application")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("app_not_found", format!("Application {} not found", app_id))
            .with_request_id(request_id.clone())
    })?;

    if req.expected_version != current.resource_version {
        return Err(
            ApiError::conflict("version_conflict", "Resource version mismatch")
                .with_request_id(request_id.clone()),
        );
    }

    if let Some(name) = req.name.as_ref() {
        if name != &current.name {
            let name_exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM apps_view WHERE org_id = $1 AND name = $2 AND NOT is_deleted AND app_id != $3)",
            )
            .bind(org_scope.clone())
            .bind(name)
            .bind(app_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to check app name uniqueness");
                ApiError::internal("internal_error", "Failed to verify application name")
                    .with_request_id(request_id.clone())
            })?;

            if name_exists {
                return Err(ApiError::conflict(
                    "app_name_exists",
                    format!("Application '{}' already exists in this organization", name),
                )
                .with_request_id(request_id.clone()));
            }
        }
    }

    let next_version = current.resource_version + 1;
    let payload = serde_json::json!({
        "app_id": app_id.to_string(),
        "org_id": org_id.to_string(),
        "name": req.name,
        "description": req.description
    });

    let event = AppendEvent {
        aggregate_type: AggregateType::App,
        aggregate_id: app_id.to_string(),
        aggregate_seq: next_version,
        event_type: event_types::APP_UPDATED.to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
        ..Default::default()
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to update app");
        ApiError::internal("internal_error", "Failed to update application")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "apps",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE app_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load app");
        ApiError::internal("internal_error", "Failed to update application")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Application was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = AppResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to update application")
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

async fn delete_app(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "apps.delete";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "org_id": org_scope.clone(),
                "app_id": app_id.to_string()
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

    let row = sqlx::query_as::<_, AppDeleteRow>(
        r#"
        SELECT resource_version, is_deleted
        FROM apps_view
        WHERE app_id = $1 AND org_id = $2
        "#,
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, app_id = %app_id, "Failed to load app");
        ApiError::internal("internal_error", "Failed to delete application")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(ApiError::not_found(
            "app_not_found",
            format!("Application {} not found", app_id),
        )
        .with_request_id(request_id.clone()));
    };

    let response = DeleteResponse { ok: true };
    if row.is_deleted {
        if let Some((key, hash)) = request_hash {
            let body = serde_json::to_value(&response).map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
                ApiError::internal("internal_error", "Failed to delete application")
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

        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    let next_version = row.resource_version + 1;
    let payload = serde_json::json!({});

    let event = AppendEvent {
        aggregate_type: AggregateType::App,
        aggregate_id: app_id.to_string(),
        aggregate_seq: next_version,
        event_type: event_types::APP_DELETED.to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: Some(app_id),
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
        ..Default::default()
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to delete app");
        ApiError::internal("internal_error", "Failed to delete application")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "apps",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to delete application")
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

/// List applications in an organization.
///
/// GET /v1/orgs/{org_id}/apps
async fn list_apps(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<ListAppsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    // Validate org_id format
    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match query.cursor.as_deref() {
        Some(raw) => {
            let _: AppId = raw.parse().map_err(|_| {
                ApiError::bad_request("invalid_cursor", "Invalid cursor format")
                    .with_request_id(request_id.clone())
            })?;
            Some(raw.to_string())
        }
        None => None,
    };

    // Query the apps_view table (stable ordering by app_id)
    let rows = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE org_id = $1 AND NOT is_deleted
          AND ($2::TEXT IS NULL OR app_id > $2)
        ORDER BY app_id ASC
        LIMIT $3
        "#,
    )
    .bind(org_id.to_string())
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list apps");
        ApiError::internal("internal_error", "Failed to list applications")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<AppResponse> = rows.into_iter().map(AppResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|a| a.id.clone())
    } else {
        None
    };

    Ok(Json(ListAppsResponse { items, next_cursor }))
}

/// Get a single application by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}
async fn get_app(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

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

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    // Query the apps_view table
    let row = sqlx::query_as::<_, AppRow>(
        r#"
        SELECT app_id, org_id, name, description, resource_version, created_at, updated_at
        FROM apps_view
        WHERE app_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(app_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            app_id = %app_id,
            org_id = %org_id,
            "Failed to get app"
        );
        ApiError::internal("internal_error", "Failed to get application")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(AppResponse::from(row))),
        None => Err(ApiError::not_found(
            "app_not_found",
            format!("Application {} not found", app_id),
        )
        .with_request_id(request_id.clone())),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from apps_view table.
struct AppRow {
    app_id: String,
    org_id: String,
    name: String,
    description: Option<String>,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct AppDeleteRow {
    resource_version: i32,
    is_deleted: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AppRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            app_id: row.try_get("app_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AppDeleteRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            resource_version: row.try_get("resource_version")?,
            is_deleted: row.try_get("is_deleted")?,
        })
    }
}

impl From<AppRow> for AppResponse {
    fn from(row: AppRow) -> Self {
        Self {
            id: row.app_id,
            org_id: row.org_id,
            name: row.name,
            description: row.description,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_app_request_deserialization() {
        let json = r#"{"name": "my-app", "description": "A test app"}"#;
        let req: CreateAppRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-app");
        assert_eq!(req.description, Some("A test app".to_string()));
    }

    #[test]
    fn test_create_app_request_without_description() {
        let json = r#"{"name": "my-app"}"#;
        let req: CreateAppRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-app");
        assert_eq!(req.description, None);
    }

    #[test]
    fn test_app_response_serialization() {
        let response = AppResponse {
            id: "app_123".to_string(),
            org_id: "org_456".to_string(),
            name: "Test App".to_string(),
            description: Some("A test".to_string()),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"app_123\""));
        assert!(json.contains("\"org_id\":\"org_456\""));
    }
}
