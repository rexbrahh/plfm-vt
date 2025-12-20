//! Route API endpoints.
//!
//! Routes bind hostnames to backend process targets within an environment.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{
    event_types, AggregateType, RouteCreatedPayload, RouteDeletedPayload, RouteProtocolHint,
    RouteProxyProtocol, RouteUpdatedPayload,
};
use plfm_id::{AppId, EnvId, OrgId, RouteId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::{AppendEvent, EventRow};
use crate::state::AppState;

/// Create route routes.
///
/// Routes are nested under envs:
/// /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_routes))
        .route("/", post(create_route))
        .route("/{route_id}", get(get_route))
        .route("/{route_id}", patch(update_route))
        .route("/{route_id}", delete(delete_route))
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListRoutesQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RouteResponse {
    pub id: String,
    pub env_id: String,
    pub hostname: String,
    pub listen_port: i32,
    pub protocol_hint: RouteProtocolHint,
    pub backend_process_type: String,
    pub backend_port: i32,
    pub proxy_protocol: RouteProxyProtocol,
    #[serde(default)]
    pub ipv4_required: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resource_version: i32,
}

#[derive(Debug, Serialize)]
pub struct ListRoutesResponse {
    pub items: Vec<RouteResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateRouteRequest {
    pub hostname: String,
    pub listen_port: i32,
    pub protocol_hint: RouteProtocolHint,
    pub backend_process_type: String,
    pub backend_port: i32,
    #[serde(default)]
    pub proxy_protocol: RouteProxyProtocol,
    #[serde(default)]
    pub backend_expects_proxy_protocol: bool,
    #[serde(default)]
    pub ipv4_required: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateRouteRequest {
    pub expected_version: i32,
    #[serde(default)]
    pub backend_process_type: Option<String>,
    #[serde(default)]
    pub backend_port: Option<i32>,
    #[serde(default)]
    pub proxy_protocol: Option<RouteProxyProtocol>,
    #[serde(default)]
    pub backend_expects_proxy_protocol: Option<bool>,
    #[serde(default)]
    pub ipv4_required: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub ok: bool,
}

// =============================================================================
// Handlers
// =============================================================================

/// List routes for an environment.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes
async fn list_routes(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<ListRoutesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

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

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor.as_deref();

    let rows = sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            route_id,
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
            updated_at
        FROM routes_view
        WHERE org_id = $1
          AND app_id = $2
          AND env_id = $3
          AND NOT is_deleted
          AND ($4::TEXT IS NULL OR route_id > $4)
        ORDER BY route_id ASC
        LIMIT $5
        "#,
    )
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .bind(cursor)
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            app_id = %app_id,
            env_id = %env_id,
            "Failed to list routes"
        );
        ApiError::internal("internal_error", "Failed to list routes")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<RouteResponse> = rows.into_iter().map(RouteResponse::from).collect();
    let next_cursor = items
        .last()
        .filter(|_| items.len() as i64 == limit)
        .map(|r| r.id.clone());

    Ok(Json(ListRoutesResponse { items, next_cursor }))
}

/// Create a route.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes
async fn create_route(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(req): Json<CreateRouteRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "routes.create";

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

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    validate_hostname(&req.hostname, &request_id)?;
    validate_port(req.listen_port, "listen_port", &request_id)?;
    validate_port(req.backend_port, "backend_port", &request_id)?;

    if matches!(req.proxy_protocol, RouteProxyProtocol::V2) && !req.backend_expects_proxy_protocol {
        return Err(ApiError::bad_request(
            "invalid_proxy_protocol",
            "backend_expects_proxy_protocol must be true when proxy_protocol is v2",
        )
        .with_request_id(request_id.clone()));
    }

    if matches!(req.proxy_protocol, RouteProxyProtocol::Off) && req.backend_expects_proxy_protocol {
        return Err(ApiError::bad_request(
            "invalid_proxy_protocol",
            "backend_expects_proxy_protocol must be false when proxy_protocol is off",
        )
        .with_request_id(request_id.clone()));
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "env_id": env_id.to_string(),
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

    // Validate env exists (scoped to org/app).
    let env_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM envs_view
            WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted
        )
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            app_id = %app_id,
            env_id = %env_id,
            "Failed to check env existence"
        );
        ApiError::internal("internal_error", "Failed to verify environment")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.clone()));
    }

    // Enforce global hostname uniqueness by policy (view + event-log fallback for projection lag).
    let hostname_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
          EXISTS (SELECT 1 FROM routes_view WHERE hostname = $1 AND NOT is_deleted)
          OR EXISTS (
            SELECT 1
            FROM events e
            WHERE e.event_type = 'route.created'
              AND e.payload->>'hostname' = $1
              AND NOT EXISTS (
                SELECT 1
                FROM events d
                WHERE d.aggregate_type = e.aggregate_type
                  AND d.aggregate_id = e.aggregate_id
                  AND d.event_type = 'route.deleted'
              )
          )
        "#,
    )
    .bind(&req.hostname)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            hostname = %req.hostname,
            "Failed to check hostname uniqueness"
        );
        ApiError::internal("internal_error", "Failed to verify hostname uniqueness")
            .with_request_id(request_id.clone())
    })?;

    if hostname_exists {
        return Err(ApiError::conflict(
            "hostname_in_use",
            format!("Hostname '{}' is already in use", req.hostname),
        )
        .with_request_id(request_id.clone()));
    }

    let route_id = RouteId::new();
    let payload = RouteCreatedPayload {
        route_id,
        org_id,
        app_id,
        env_id,
        hostname: req.hostname.clone(),
        listen_port: req.listen_port,
        protocol_hint: req.protocol_hint,
        backend_process_type: req.backend_process_type.clone(),
        backend_port: req.backend_port,
        proxy_protocol: req.proxy_protocol,
        backend_expects_proxy_protocol: req.backend_expects_proxy_protocol,
        ipv4_required: req.ipv4_required,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            "Failed to serialize route payload"
        );
        ApiError::internal("internal_error", "Failed to create route")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Route,
        aggregate_id: route_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::ROUTE_CREATED.to_string(),
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
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            route_id = %route_id,
            "Failed to create route"
        );
        ApiError::internal("internal_error", "Failed to create route")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "routes",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            route_id,
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
            updated_at
        FROM routes_view
        WHERE route_id = $1
          AND org_id = $2
          AND app_id = $3
          AND env_id = $4
          AND NOT is_deleted
        "#,
    )
    .bind(route_id.to_string())
    .bind(org_scope.clone())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load route");
        ApiError::internal("internal_error", "Failed to load route")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Route was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = RouteResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create route")
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

/// Get route.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}
async fn get_route(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, route_id)): Path<(String, String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

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
    let route_id: RouteId = route_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_route_id", "Invalid route ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let row = sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            route_id,
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
            updated_at
        FROM routes_view
        WHERE route_id = $1
          AND org_id = $2
          AND app_id = $3
          AND env_id = $4
          AND NOT is_deleted
        "#,
    )
    .bind(route_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            route_id = %route_id,
            "Failed to get route"
        );
        ApiError::internal("internal_error", "Failed to get route")
            .with_request_id(request_id.clone())
    })?;

    if let Some(row) = row {
        return Ok(Json(RouteResponse::from(row)));
    }

    // Fallback: reconstruct from event log for projection lag.
    let event_store = state.db().event_store();
    let Some(route) = load_route_from_events(&event_store, &route_id, &request_id).await? else {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    };

    if route.is_deleted
        || route.org_id != org_id
        || route.app_id != app_id
        || route.env_id != env_id
    {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    }

    Ok(Json(route.to_response()))
}

/// Update route.
///
/// PATCH /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}
async fn update_route(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, route_id)): Path<(String, String, String, String)>,
    Json(req): Json<UpdateRouteRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "routes.update";

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
    let route_id: RouteId = route_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_route_id", "Invalid route ID format")
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

    if req.backend_process_type.is_none()
        && req.backend_port.is_none()
        && req.proxy_protocol.is_none()
        && req.backend_expects_proxy_protocol.is_none()
        && req.ipv4_required.is_none()
    {
        return Err(
            ApiError::bad_request("invalid_update", "No updatable fields provided")
                .with_request_id(request_id.clone()),
        );
    }

    if let Some(port) = req.backend_port {
        validate_port(port, "backend_port", &request_id)?;
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "env_id": env_id.to_string(),
                "route_id": route_id.to_string(),
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

    let event_store = state.db().event_store();
    let Some(current) = load_route_from_events(&event_store, &route_id, &request_id).await? else {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    };

    if current.is_deleted
        || current.org_id != org_id
        || current.app_id != app_id
        || current.env_id != env_id
    {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    }

    if current.resource_version != req.expected_version {
        return Err(ApiError::conflict(
            "version_conflict",
            format!(
                "Route version conflict: expected {}, current {}",
                req.expected_version, current.resource_version
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let next_version = current.resource_version + 1;

    // Validate proxy protocol invariants (v1).
    let desired_proxy_protocol = req.proxy_protocol.unwrap_or(current.proxy_protocol);
    if desired_proxy_protocol == RouteProxyProtocol::V2 {
        let is_transition = current.proxy_protocol != RouteProxyProtocol::V2;
        if is_transition && req.backend_expects_proxy_protocol != Some(true) {
            return Err(ApiError::bad_request(
                "invalid_proxy_protocol",
                "backend_expects_proxy_protocol must be true when enabling proxy_protocol v2",
            )
            .with_request_id(request_id.clone()));
        }
        if req.backend_expects_proxy_protocol == Some(false) {
            return Err(ApiError::bad_request(
                "invalid_proxy_protocol",
                "backend_expects_proxy_protocol cannot be false when proxy_protocol is v2",
            )
            .with_request_id(request_id.clone()));
        }
    } else if req.backend_expects_proxy_protocol == Some(true) {
        return Err(ApiError::bad_request(
            "invalid_proxy_protocol",
            "backend_expects_proxy_protocol must be false when proxy_protocol is off",
        )
        .with_request_id(request_id.clone()));
    }

    let payload = RouteUpdatedPayload {
        route_id,
        org_id,
        env_id,
        backend_process_type: req.backend_process_type.clone(),
        backend_port: req.backend_port,
        proxy_protocol: req.proxy_protocol,
        backend_expects_proxy_protocol: req.backend_expects_proxy_protocol,
        ipv4_required: req.ipv4_required,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize route update payload");
        ApiError::internal("internal_error", "Failed to update route")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Route,
        aggregate_id: route_id.to_string(),
        aggregate_seq: next_version,
        event_type: event_types::ROUTE_UPDATED.to_string(),
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
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            route_id = %route_id,
            "Failed to update route"
        );
        ApiError::internal("internal_error", "Failed to update route")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "routes",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            route_id,
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
            updated_at
        FROM routes_view
        WHERE route_id = $1
          AND org_id = $2
          AND app_id = $3
          AND env_id = $4
          AND NOT is_deleted
        "#,
    )
    .bind(route_id.to_string())
    .bind(org_scope.clone())
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load route");
        ApiError::internal("internal_error", "Failed to load route")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Route was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = RouteResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to update route")
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

/// Delete route (idempotent for already-deleted routes).
///
/// DELETE /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes/{route_id}
async fn delete_route(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, route_id)): Path<(String, String, String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "routes.delete";

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
    let route_id: RouteId = route_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_route_id", "Invalid route ID format")
            .with_request_id(request_id.clone())
    })?;

    let role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_write(role, &request_id)?;

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "app_id": app_id.to_string(),
                "env_id": env_id.to_string(),
                "route_id": route_id.to_string()
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

    let event_store = state.db().event_store();
    let Some(current) = load_route_from_events(&event_store, &route_id, &request_id).await? else {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    };

    if current.org_id != org_id || current.app_id != app_id || current.env_id != env_id {
        return Err(ApiError::not_found("route_not_found", "Route not found")
            .with_request_id(request_id.clone()));
    }

    let response = DeleteResponse { ok: true };
    if current.is_deleted {
        if let Some((key, hash)) = request_hash {
            let body = serde_json::to_value(&response).map_err(|e| {
                tracing::error!(
                    error = %e,
                    request_id = %request_id,
                    "Failed to serialize response"
                );
                ApiError::internal("internal_error", "Failed to delete route")
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

    let next_version = current.resource_version + 1;
    let payload = RouteDeletedPayload {
        route_id,
        org_id,
        env_id,
        hostname: current.hostname.clone(),
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize route delete payload");
        ApiError::internal("internal_error", "Failed to delete route")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Route,
        aggregate_id: route_id.to_string(),
        aggregate_seq: next_version,
        event_type: event_types::ROUTE_DELETED.to_string(),
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
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            route_id = %route_id,
            "Failed to delete route"
        );
        ApiError::internal("internal_error", "Failed to delete route")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "routes",
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
            ApiError::internal("internal_error", "Failed to delete route")
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

// =============================================================================
// Helpers
// =============================================================================

struct RouteRow {
    route_id: String,
    env_id: String,
    hostname: String,
    listen_port: i32,
    protocol_hint: Option<String>,
    backend_process_type: String,
    backend_port: i32,
    proxy_protocol: bool,
    ipv4_required: bool,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for RouteRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            route_id: row.try_get("route_id")?,
            env_id: row.try_get("env_id")?,
            hostname: row.try_get("hostname")?,
            listen_port: row.try_get("listen_port")?,
            protocol_hint: row.try_get("protocol_hint")?,
            backend_process_type: row.try_get("backend_process_type")?,
            backend_port: row.try_get("backend_port")?,
            proxy_protocol: row.try_get("proxy_protocol")?,
            ipv4_required: row.try_get("ipv4_required")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<RouteRow> for RouteResponse {
    fn from(row: RouteRow) -> Self {
        let protocol_hint = match row.protocol_hint.as_deref() {
            Some("tls_passthrough") => RouteProtocolHint::TlsPassthrough,
            _ => RouteProtocolHint::TcpRaw,
        };

        Self {
            id: row.route_id,
            env_id: row.env_id,
            hostname: row.hostname,
            listen_port: row.listen_port,
            protocol_hint,
            backend_process_type: row.backend_process_type,
            backend_port: row.backend_port,
            proxy_protocol: if row.proxy_protocol {
                RouteProxyProtocol::V2
            } else {
                RouteProxyProtocol::Off
            },
            ipv4_required: row.ipv4_required,
            created_at: row.created_at,
            updated_at: row.updated_at,
            resource_version: row.resource_version,
        }
    }
}

struct RouteState {
    route_id: RouteId,
    org_id: OrgId,
    app_id: AppId,
    env_id: EnvId,
    hostname: String,
    listen_port: i32,
    protocol_hint: RouteProtocolHint,
    backend_process_type: String,
    backend_port: i32,
    proxy_protocol: RouteProxyProtocol,
    ipv4_required: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    resource_version: i32,
    is_deleted: bool,
}

impl RouteState {
    fn to_response(&self) -> RouteResponse {
        RouteResponse {
            id: self.route_id.to_string(),
            env_id: self.env_id.to_string(),
            hostname: self.hostname.clone(),
            listen_port: self.listen_port,
            protocol_hint: self.protocol_hint,
            backend_process_type: self.backend_process_type.clone(),
            backend_port: self.backend_port,
            proxy_protocol: self.proxy_protocol,
            ipv4_required: self.ipv4_required,
            created_at: self.created_at,
            updated_at: self.updated_at,
            resource_version: self.resource_version,
        }
    }
}

async fn load_route_from_events(
    store: &crate::db::EventStore,
    route_id: &RouteId,
    request_id: &str,
) -> Result<Option<RouteState>, ApiError> {
    let route_id_str = route_id.to_string();
    let rows = store
        .query_by_aggregate(&AggregateType::Route, &route_id_str)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                route_id = %route_id,
                "Failed to query route events"
            );
            ApiError::internal("internal_error", "Failed to load route")
                .with_request_id(request_id.to_string())
        })?;

    fold_route_events(route_id, &rows, request_id)
}

fn fold_route_events(
    route_id: &RouteId,
    events: &[EventRow],
    request_id: &str,
) -> Result<Option<RouteState>, ApiError> {
    let mut state: Option<RouteState> = None;

    for event in events {
        match event.event_type.as_str() {
            "route.created" => {
                let payload: RouteCreatedPayload = serde_json::from_value(event.payload.clone())
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            request_id = %request_id,
                            route_id = %route_id,
                            "Invalid route.created payload"
                        );
                        ApiError::internal("internal_error", "Invalid route event payload")
                            .with_request_id(request_id.to_string())
                    })?;

                state = Some(RouteState {
                    route_id: payload.route_id,
                    org_id: payload.org_id,
                    app_id: payload.app_id,
                    env_id: payload.env_id,
                    hostname: payload.hostname,
                    listen_port: payload.listen_port,
                    protocol_hint: payload.protocol_hint,
                    backend_process_type: payload.backend_process_type,
                    backend_port: payload.backend_port,
                    proxy_protocol: payload.proxy_protocol,
                    ipv4_required: payload.ipv4_required,
                    created_at: event.occurred_at,
                    updated_at: event.occurred_at,
                    resource_version: event.aggregate_seq,
                    is_deleted: false,
                });
            }
            "route.updated" => {
                let payload: RouteUpdatedPayload = serde_json::from_value(event.payload.clone())
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            request_id = %request_id,
                            route_id = %route_id,
                            "Invalid route.updated payload"
                        );
                        ApiError::internal("internal_error", "Invalid route event payload")
                            .with_request_id(request_id.to_string())
                    })?;

                let Some(s) = state.as_mut() else { continue };
                if payload.org_id != s.org_id || payload.env_id != s.env_id {
                    continue;
                }

                if let Some(v) = payload.backend_process_type {
                    s.backend_process_type = v;
                }
                if let Some(v) = payload.backend_port {
                    s.backend_port = v;
                }
                if let Some(v) = payload.proxy_protocol {
                    s.proxy_protocol = v;
                }
                if let Some(v) = payload.ipv4_required {
                    s.ipv4_required = v;
                }

                s.updated_at = event.occurred_at;
                s.resource_version = event.aggregate_seq;
            }
            "route.deleted" => {
                let payload: RouteDeletedPayload = serde_json::from_value(event.payload.clone())
                    .map_err(|e| {
                        tracing::error!(
                            error = %e,
                            request_id = %request_id,
                            route_id = %route_id,
                            "Invalid route.deleted payload"
                        );
                        ApiError::internal("internal_error", "Invalid route event payload")
                            .with_request_id(request_id.to_string())
                    })?;

                let Some(s) = state.as_mut() else { continue };
                if payload.org_id != s.org_id || payload.env_id != s.env_id {
                    continue;
                }

                s.is_deleted = true;
                s.updated_at = event.occurred_at;
                s.resource_version = event.aggregate_seq;
            }
            _ => {}
        }
    }

    Ok(state)
}

fn validate_hostname(hostname: &str, request_id: &str) -> Result<(), ApiError> {
    if hostname.trim().is_empty() {
        return Err(
            ApiError::bad_request("invalid_hostname", "hostname cannot be empty")
                .with_request_id(request_id.to_string()),
        );
    }

    if hostname.len() > 253 {
        return Err(ApiError::bad_request(
            "invalid_hostname",
            "hostname cannot exceed 253 characters",
        )
        .with_request_id(request_id.to_string()));
    }

    if hostname.contains(char::is_whitespace) {
        return Err(ApiError::bad_request(
            "invalid_hostname",
            "hostname cannot contain whitespace",
        )
        .with_request_id(request_id.to_string()));
    }

    Ok(())
}

fn validate_port(port: i32, field: &str, request_id: &str) -> Result<(), ApiError> {
    if !(1..=65535).contains(&port) {
        return Err(ApiError::bad_request(
            format!("invalid_{field}"),
            format!("{field} must be between 1 and 65535"),
        )
        .with_request_id(request_id.to_string()));
    }

    Ok(())
}
