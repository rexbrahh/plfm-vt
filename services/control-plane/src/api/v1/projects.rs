//! Project API endpoints.
//!
//! Projects are optional organizational groupings for apps and environments.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::AggregateType;
use plfm_id::{OrgId, ProjectId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create project routes.
///
/// Projects are nested under orgs: /v1/orgs/{org_id}/projects
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_project))
        .route("/", get(list_projects))
        .route("/{project_id}", patch(update_project))
        .route("/{project_id}", get(get_project))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new project.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateProjectRequest {
    /// Project name (unique within org).
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub expected_version: i32,
}

/// Response for a single project.
#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    /// Project ID.
    pub id: String,

    /// Organization ID.
    pub org_id: String,

    /// Project name.
    pub name: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the project was created.
    pub created_at: DateTime<Utc>,

    /// When the project was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing projects.
#[derive(Debug, Serialize)]
pub struct ListProjectsResponse {
    /// List of projects.
    pub items: Vec<ProjectResponse>,

    /// Next cursor (or null when there are no more items).
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListProjectsQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new project.
///
/// POST /v1/orgs/{org_id}/projects
async fn create_project(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "projects.create";

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
            ApiError::bad_request("invalid_name", "Project name cannot be empty")
                .with_request_id(request_id),
        );
    }

    if req.name.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Project name cannot exceed 100 characters",
        )
        .with_request_id(request_id));
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
    let org_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM orgs_view WHERE org_id = $1)",
    )
    .bind(org_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check org existence");
        ApiError::internal("internal_error", "Failed to verify organization")
            .with_request_id(request_id.clone())
    })?;

    if !org_exists {
        return Err(ApiError::not_found(
            "org_not_found",
            format!("Organization {} not found", org_id),
        )
        .with_request_id(request_id));
    }

    // Check for duplicate name within org
    let name_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM projects_view WHERE org_id = $1 AND name = $2 AND NOT is_deleted)",
    )
    .bind(org_id.to_string())
    .bind(&req.name)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check project name uniqueness");
        ApiError::internal("internal_error", "Failed to verify project name")
            .with_request_id(request_id.clone())
    })?;

    if name_exists {
        return Err(ApiError::conflict(
            "project_name_exists",
            format!("Project '{}' already exists in this organization", req.name),
        )
        .with_request_id(request_id));
    }

    let project_id = ProjectId::new();

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Project,
        aggregate_id: project_id.to_string(),
        aggregate_seq: 1,
        event_type: "project.created".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "name": req.name
        }),
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create project");
        ApiError::internal("internal_error", "Failed to create project")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "projects",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project_id, org_id, name, resource_version, created_at, updated_at
        FROM projects_view
        WHERE project_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(project_id.to_string())
    .bind(org_scope.clone())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load project");
        ApiError::internal("internal_error", "Failed to load project")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Project was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = ProjectResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create project")
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

async fn update_project(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, project_id)): Path<(String, String)>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "projects.update";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let project_id: ProjectId = project_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_project_id", "Invalid project ID format")
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

    if req.name.is_none() {
        return Err(
            ApiError::bad_request("invalid_update", "No updatable fields provided")
                .with_request_id(request_id.clone()),
        );
    }

    if let Some(name) = req.name.as_ref() {
        if name.is_empty() {
            return Err(
                ApiError::bad_request("invalid_name", "Project name cannot be empty")
                    .with_request_id(request_id.clone()),
            );
        }
        if name.len() > 100 {
            return Err(ApiError::bad_request(
                "invalid_name",
                "Project name cannot exceed 100 characters",
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
                "project_id": project_id.to_string(),
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

    let current = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project_id, org_id, name, resource_version, created_at, updated_at
        FROM projects_view
        WHERE project_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(project_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, project_id = %project_id, "Failed to load project");
        ApiError::internal("internal_error", "Failed to update project")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("project_not_found", format!("Project {} not found", project_id))
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
                "SELECT EXISTS(SELECT 1 FROM projects_view WHERE org_id = $1 AND name = $2 AND NOT is_deleted AND project_id != $3)",
            )
            .bind(org_scope.clone())
            .bind(name)
            .bind(project_id.to_string())
            .fetch_one(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to check project name uniqueness");
                ApiError::internal("internal_error", "Failed to verify project name")
                    .with_request_id(request_id.clone())
            })?;

            if name_exists {
                return Err(ApiError::conflict(
                    "project_name_exists",
                    format!("Project '{}' already exists in this organization", name),
                )
                .with_request_id(request_id.clone()));
            }
        }
    }

    let next_version = current.resource_version + 1;
    let payload = serde_json::json!({
        "name": req.name
    });

    let event = AppendEvent {
        aggregate_type: AggregateType::Project,
        aggregate_id: project_id.to_string(),
        aggregate_seq: next_version,
        event_type: "project.updated".to_string(),
        event_version: 1,
        actor_type,
        actor_id: actor_id.clone(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: idempotency_key.clone(),
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload,
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to update project");
        ApiError::internal("internal_error", "Failed to update project")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "projects",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project_id, org_id, name, resource_version, created_at, updated_at
        FROM projects_view
        WHERE project_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(project_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, project_id = %project_id, "Failed to load project");
        ApiError::internal("internal_error", "Failed to update project")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Project was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = ProjectResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to update project")
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

/// List projects in an organization.
///
/// GET /v1/orgs/{org_id}/projects
async fn list_projects(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<ListProjectsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor.as_deref();

    let rows = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project_id, org_id, name, resource_version, created_at, updated_at
        FROM projects_view
        WHERE org_id = $1 AND NOT is_deleted
          AND ($2::TEXT IS NULL OR project_id > $2)
        ORDER BY project_id ASC
        LIMIT $3
        "#,
    )
    .bind(org_id.to_string())
    .bind(cursor)
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list projects");
        ApiError::internal("internal_error", "Failed to list projects")
            .with_request_id(request_id.clone())
    })?;

    let next_cursor = rows.last().map(|row| row.project_id.clone());
    let items: Vec<ProjectResponse> = rows.into_iter().map(ProjectResponse::from).collect();

    Ok(Json(ListProjectsResponse { items, next_cursor }))
}

/// Get a project by ID.
///
/// GET /v1/orgs/{org_id}/projects/{project_id}
async fn get_project(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, project_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let project_id: ProjectId = project_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_project_id", "Invalid project ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let row = sqlx::query_as::<_, ProjectRow>(
        r#"
        SELECT project_id, org_id, name, resource_version, created_at, updated_at
        FROM projects_view
        WHERE project_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(project_id.to_string())
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, project_id = %project_id, "Failed to get project");
        ApiError::internal("internal_error", "Failed to get project")
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(ProjectResponse::from(row))),
        None => Err(ApiError::not_found(
            "project_not_found",
            format!("Project {} not found", project_id),
        )
        .with_request_id(request_id.clone())),
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

struct ProjectRow {
    project_id: String,
    org_id: String,
    name: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ProjectRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            project_id: row.try_get("project_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<ProjectRow> for ProjectResponse {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: row.project_id,
            org_id: row.org_id,
            name: row.name,
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
    fn test_create_project_request_deserialization() {
        let json = r#"{"name":"my-project"}"#;
        let req: CreateProjectRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "my-project");
    }

    #[test]
    fn test_project_response_serialization() {
        let response = ProjectResponse {
            id: "prj_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            org_id: "org_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            name: "my-project".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\""));
        assert!(json.contains("\"org_id\""));
        assert!(json.contains("\"name\""));
    }
}
