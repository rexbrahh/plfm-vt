//! Deploy API endpoints.
//!
//! Provides operations for creating and querying deploys.
//! A deploy promotes a release to an environment.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{AggregateType, DeployStatus};
use plfm_id::{AppId, DeployId, EnvId, OrgId, ReleaseId};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create deploy routes.
///
/// Deploys are nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_deploy))
        .route("/", get(list_deploys))
        .route("/:deploy_id", get(get_deploy))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new deploy.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateDeployRequest {
    /// Release ID to deploy.
    pub release_id: String,

    /// Optional process types to deploy (defaults to all).
    #[serde(default)]
    pub process_types: Option<Vec<String>>,

    /// Deploy strategy (v1 only supports rolling).
    #[serde(default)]
    pub strategy: DeployStrategy,
}

/// Deploy strategy (v1).
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStrategy {
    Rolling,
}

impl Default for DeployStrategy {
    fn default() -> Self {
        Self::Rolling
    }
}

/// Request to create a rollback (select a previous release).
#[derive(Debug, Deserialize, Serialize)]
pub struct RollbackRequest {
    /// Release ID to roll back to.
    pub release_id: String,
}

/// Response for a single deploy.
#[derive(Debug, Serialize)]
pub struct DeployResponse {
    /// Deploy ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// Application ID.
    pub app_id: String,

    /// Environment ID.
    pub env_id: String,

    /// Kind of deploy (deploy or rollback).
    pub kind: String,

    /// Release ID being deployed.
    pub release_id: String,

    /// Process types being deployed.
    pub process_types: Vec<String>,

    /// Current status.
    pub status: String,

    /// Status message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the deploy was created.
    pub created_at: DateTime<Utc>,

    /// When the deploy was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing deploys.
#[derive(Debug, Serialize)]
pub struct ListDeploysResponse {
    /// List of deploys.
    pub items: Vec<DeployResponse>,

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing deploys.
#[derive(Debug, Deserialize)]
pub struct ListDeploysQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as a deploy_id.
    pub cursor: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new deploy.
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys
async fn create_deploy(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(req): Json<CreateDeployRequest>,
) -> Result<Response, ApiError> {
    let RequestContext {
        request_id,
        idempotency_key,
        actor_type,
        actor_id,
    } = ctx;
    let endpoint_name = "deploys.create";

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

    let release_id: ReleaseId = req.release_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_release_id", "Invalid release ID format")
            .with_request_id(request_id.clone())
    })?;

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

    // Validate env exists and belongs to app
    let env_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE env_id = $1 AND app_id = $2 AND NOT is_deleted)",
    )
    .bind(env_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check env existence");
        ApiError::internal("internal_error", "Failed to verify environment")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found in application {}", env_id, app_id),
        )
        .with_request_id(request_id.clone()));
    }

    // Validate release exists and belongs to app
    let release_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM releases_view WHERE release_id = $1 AND app_id = $2)",
    )
    .bind(release_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check release existence");
        ApiError::internal("internal_error", "Failed to verify release")
            .with_request_id(request_id.clone())
    })?;

    if !release_exists {
        return Err(ApiError::not_found(
            "release_not_found",
            format!("Release {} not found in application {}", release_id, app_id),
        )
        .with_request_id(request_id.clone()));
    }

    let deploy_id = DeployId::new();
    let kind = "deploy";
    let process_types = req.process_types.unwrap_or_else(|| vec!["web".to_string()]);

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Deploy,
        aggregate_id: deploy_id.to_string(),
        aggregate_seq: 1,
        event_type: "deploy.created".to_string(),
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
            "release_id": release_id.to_string(),
            "kind": kind,
            "process_types": process_types,
            "status": DeployStatus::Queued
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create deploy");
        ApiError::internal("internal_error", "Failed to create deploy")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "deploys",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, DeployRow>(
        r#"
        SELECT deploy_id, org_id, app_id, env_id, kind, release_id, process_types,
               status, message, resource_version, created_at, updated_at
        FROM deploys_view
        WHERE deploy_id = $1 AND org_id = $2 AND app_id = $3 AND env_id = $4
        "#,
    )
    .bind(deploy_id.to_string())
    .bind(&org_scope)
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load deploy");
        ApiError::internal("internal_error", "Failed to load deploy")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Deploy was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = DeployResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create deploy")
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

/// Create a rollback (represented as a deploy with kind=rollback).
///
/// POST /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/rollbacks
pub async fn create_rollback(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Json(req): Json<RollbackRequest>,
) -> Result<Response, ApiError> {
    let RequestContext {
        request_id,
        idempotency_key,
        actor_type,
        actor_id,
    } = ctx;
    let endpoint_name = "rollbacks.create";

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

    let release_id: ReleaseId = req.release_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_release_id", "Invalid release ID format")
            .with_request_id(request_id.clone())
    })?;

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

    // Validate env exists and belongs to app
    let env_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE env_id = $1 AND app_id = $2 AND NOT is_deleted)",
    )
    .bind(env_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check env existence");
        ApiError::internal("internal_error", "Failed to verify environment")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found in application {}", env_id, app_id),
        )
        .with_request_id(request_id.clone()));
    }

    // Validate release exists and belongs to app
    let release_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM releases_view WHERE release_id = $1 AND app_id = $2)",
    )
    .bind(release_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check release existence");
        ApiError::internal("internal_error", "Failed to verify release")
            .with_request_id(request_id.clone())
    })?;

    if !release_exists {
        return Err(ApiError::not_found(
            "release_not_found",
            format!("Release {} not found in application {}", release_id, app_id),
        )
        .with_request_id(request_id.clone()));
    }

    let deploy_id = DeployId::new();
    let process_types = vec!["web".to_string()];

    let event = AppendEvent {
        aggregate_type: AggregateType::Deploy,
        aggregate_id: deploy_id.to_string(),
        aggregate_seq: 1,
        event_type: "deploy.created".to_string(),
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
            "release_id": release_id.to_string(),
            "kind": "rollback",
            "process_types": process_types,
            "status": DeployStatus::Queued
        }),
    };

    let event_store = state.db().event_store();
    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create rollback");
        ApiError::internal("internal_error", "Failed to create rollback")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "deploys",
            event_id.value(),
            std::time::Duration::from_secs(2),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, DeployRow>(
        r#"
        SELECT deploy_id, org_id, app_id, env_id, kind, release_id, process_types,
               status, message, resource_version, created_at, updated_at
        FROM deploys_view
        WHERE deploy_id = $1 AND org_id = $2 AND app_id = $3 AND env_id = $4
        "#,
    )
    .bind(deploy_id.to_string())
    .bind(&org_scope)
    .bind(app_id.to_string())
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load rollback deploy");
        ApiError::internal("internal_error", "Failed to load rollback")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Rollback deploy was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = DeployResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create rollback")
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

/// List deploys for an environment.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys
async fn list_deploys(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
    Query(query): Query<ListDeploysQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate IDs
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match query.cursor.as_deref() {
        Some(raw) => {
            let _: DeployId = raw.parse().map_err(|_| {
                ApiError::bad_request("invalid_cursor", "Invalid cursor format")
                    .with_request_id(request_id.clone())
            })?;
            Some(raw.to_string())
        }
        None => None,
    };

    // Query the deploys_view table (stable ordering by deploy_id)
    let rows = sqlx::query_as::<_, DeployRow>(
        r#"
        SELECT deploy_id, org_id, app_id, env_id, kind, release_id, process_types,
               status, message, resource_version, created_at, updated_at
        FROM deploys_view
        WHERE org_id = $1 AND app_id = $2 AND env_id = $3
          AND ($4::TEXT IS NULL OR deploy_id > $4)
        ORDER BY deploy_id ASC
        LIMIT $5
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list deploys");
        ApiError::internal("internal_error", "Failed to list deploys")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<DeployResponse> = rows.into_iter().map(DeployResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|d| d.id.clone())
    } else {
        None
    };

    Ok(Json(ListDeploysResponse { items, next_cursor }))
}

/// Get a single deploy by ID.
///
/// GET /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys/{deploy_id}
async fn get_deploy(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id, deploy_id)): Path<(String, String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate IDs
    let _org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _app_id: AppId = app_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_app_id", "Invalid application ID format")
            .with_request_id(request_id.clone())
    })?;

    let _env_id: EnvId = env_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_env_id", "Invalid environment ID format")
            .with_request_id(request_id.clone())
    })?;

    let _deploy_id: DeployId = deploy_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_deploy_id", "Invalid deploy ID format")
            .with_request_id(request_id.clone())
    })?;

    // Query the deploys_view table
    let row = sqlx::query_as::<_, DeployRow>(
        r#"
        SELECT deploy_id, org_id, app_id, env_id, kind, release_id, process_types,
               status, message, resource_version, created_at, updated_at
        FROM deploys_view
        WHERE org_id = $1 AND app_id = $2 AND env_id = $3 AND deploy_id = $4
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .bind(&deploy_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, deploy_id = %deploy_id, "Failed to get deploy");
        ApiError::internal("internal_error", "Failed to get deploy")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(DeployResponse::from(row))),
        None => Err(ApiError::not_found(
            "deploy_not_found",
            format!("Deploy {} not found", deploy_id),
        )
        .with_request_id(request_id)),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from deploys_view table.
struct DeployRow {
    deploy_id: String,
    org_id: String,
    app_id: String,
    env_id: String,
    kind: String,
    release_id: String,
    process_types: serde_json::Value,
    status: String,
    message: Option<String>,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for DeployRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            deploy_id: row.try_get("deploy_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            kind: row.try_get("kind")?,
            release_id: row.try_get("release_id")?,
            process_types: row.try_get("process_types")?,
            status: row.try_get("status")?,
            message: row.try_get("message")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<DeployRow> for DeployResponse {
    fn from(row: DeployRow) -> Self {
        let process_types: Vec<String> =
            serde_json::from_value(row.process_types).unwrap_or_else(|_| vec!["web".to_string()]);

        Self {
            id: row.deploy_id,
            org_id: row.org_id,
            app_id: row.app_id,
            env_id: row.env_id,
            kind: row.kind,
            release_id: row.release_id,
            process_types,
            status: row.status,
            message: row.message,
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
    fn test_create_deploy_request_deserialization() {
        let json = r#"{
            "release_id": "rel_123",
            "process_types": ["web", "worker"]
        }"#;
        let req: CreateDeployRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.release_id, "rel_123");
        assert_eq!(
            req.process_types,
            Some(vec!["web".to_string(), "worker".to_string()])
        );
        assert!(matches!(req.strategy, DeployStrategy::Rolling));
    }

    #[test]
    fn test_create_deploy_request_minimal() {
        let json = r#"{"release_id": "rel_123"}"#;
        let req: CreateDeployRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.release_id, "rel_123");
        assert_eq!(req.process_types, None);
        assert!(matches!(req.strategy, DeployStrategy::Rolling));
    }

    #[test]
    fn test_deploy_response_serialization() {
        let response = DeployResponse {
            id: "dep_123".to_string(),
            org_id: "org_456".to_string(),
            app_id: "app_789".to_string(),
            env_id: "env_abc".to_string(),
            kind: "deploy".to_string(),
            release_id: "rel_def".to_string(),
            process_types: vec!["web".to_string()],
            status: "queued".to_string(),
            message: None,
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"dep_123\""));
        assert!(json.contains("\"status\":\"queued\""));
    }
}
