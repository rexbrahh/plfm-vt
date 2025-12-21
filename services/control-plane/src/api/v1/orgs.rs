//! Organization API endpoints.
//!
//! Provides CRUD operations for organizations.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{event_types, AggregateType, MemberRole, OrgMemberAddedPayload};
use plfm_id::{MemberId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create org routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_org))
        .route("/", get(list_orgs))
        .route("/{org_id}", patch(update_org))
        .route("/{org_id}", get(get_org))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new organization.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateOrgRequest {
    /// Organization name.
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateOrgRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub expected_version: i32,
}

/// Response for a single organization.
#[derive(Debug, Serialize)]
pub struct OrgResponse {
    /// Organization ID.
    pub id: String,

    /// Organization name.
    pub name: String,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the org was created.
    pub created_at: DateTime<Utc>,

    /// When the org was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing organizations.
#[derive(Debug, Serialize)]
pub struct ListOrgsResponse {
    /// List of organizations.
    pub items: Vec<OrgResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new organization.
///
/// POST /v1/orgs
async fn create_org(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<CreateOrgRequest>,
) -> Result<Response, ApiError> {
    authz::require_authenticated(&ctx)?;

    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let Some(actor_email) = ctx.actor_email.clone() else {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "Token subject email is required for org creation (use Bearer user:<email> in dev)",
        )
        .with_request_id(request_id));
    };
    let endpoint_name = "orgs.create";

    // Validate name
    if req.name.is_empty() {
        return Err(
            ApiError::bad_request("invalid_name", "Organization name cannot be empty")
                .with_request_id(request_id),
        );
    }

    if req.name.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Organization name cannot exceed 100 characters",
        )
        .with_request_id(request_id));
    }

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
            idempotency::IDEMPOTENCY_SCOPE_GLOBAL,
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

    let org_id = OrgId::new();
    let member_id = MemberId::new();

    let org_event = AppendEvent {
        aggregate_type: AggregateType::Org,
        aggregate_id: org_id.to_string(),
        aggregate_seq: 1, // First event for this org
        event_type: event_types::ORG_CREATED.to_string(),
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

    let member_payload = OrgMemberAddedPayload {
        member_id,
        org_id,
        email: actor_email,
        role: MemberRole::Owner,
    };

    let member_payload = serde_json::to_value(&member_payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize org owner membership payload");
        ApiError::internal("internal_error", "Failed to create organization")
            .with_request_id(request_id.clone())
    })?;

    let member_event = AppendEvent {
        aggregate_type: AggregateType::OrgMember,
        aggregate_id: member_id.to_string(),
        aggregate_seq: 1,
        event_type: event_types::ORG_MEMBER_ADDED.to_string(),
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
        payload: member_payload,
    };

    let event_ids = state
        .db()
        .event_store()
        .append_batch(vec![org_event, member_event])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to create org");
            ApiError::internal("internal_error", "Failed to create organization")
                .with_request_id(request_id.clone())
        })?;

    let (org_event_id, member_event_id) = match event_ids.as_slice() {
        [org_event_id, member_event_id] => (*org_event_id, *member_event_id),
        _ => {
            return Err(
                ApiError::internal("internal_error", "Failed to create organization")
                    .with_request_id(request_id.clone()),
            );
        }
    };

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "orgs",
            org_event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "members",
            member_event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        WHERE org_id = $1
        "#,
    )
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load org");
        ApiError::internal("internal_error", "Failed to load organization")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Organization was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = OrgResponse {
        id: row.org_id,
        name: row.name,
        resource_version: row.resource_version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create organization")
                .with_request_id(request_id.clone())
        })?;

        let _ = idempotency::store(
            &state,
            idempotency::StoreIdempotencyParams {
                org_scope: idempotency::IDEMPOTENCY_SCOPE_GLOBAL,
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

async fn update_org(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Json(req): Json<UpdateOrgRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "orgs.update";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
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
                ApiError::bad_request("invalid_name", "Organization name cannot be empty")
                    .with_request_id(request_id.clone()),
            );
        }
        if name.len() > 100 {
            return Err(ApiError::bad_request(
                "invalid_name",
                "Organization name cannot exceed 100 characters",
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

    let current = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        WHERE org_id = $1
        "#,
    )
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to load org");
        ApiError::internal("internal_error", "Failed to update organization")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("org_not_found", format!("Organization {} not found", org_id))
            .with_request_id(request_id.clone())
    })?;

    if req.expected_version != current.resource_version {
        return Err(
            ApiError::conflict("version_conflict", "Resource version mismatch")
                .with_request_id(request_id.clone()),
        );
    }

    let next_version = current.resource_version + 1;
    let payload = serde_json::json!({
        "name": req.name
    });

    let event = AppendEvent {
        aggregate_type: AggregateType::Org,
        aggregate_id: org_id.to_string(),
        aggregate_seq: next_version,
        event_type: event_types::ORG_UPDATED.to_string(),
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
        tracing::error!(error = %e, request_id = %request_id, "Failed to update org");
        ApiError::internal("internal_error", "Failed to update organization")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "orgs",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        WHERE org_id = $1
        "#,
    )
    .bind(org_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load org");
        ApiError::internal("internal_error", "Failed to update organization")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Organization was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = OrgResponse {
        id: row.org_id,
        name: row.name,
        resource_version: row.resource_version,
        created_at: row.created_at,
        updated_at: row.updated_at,
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to update organization")
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

/// List organizations.
///
/// GET /v1/orgs
async fn list_orgs(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<impl IntoResponse, ApiError> {
    authz::require_authenticated(&ctx)?;

    let request_id = ctx.request_id.clone();
    let Some(email) = ctx.actor_email.as_deref() else {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "Token subject email is required for org-scoped APIs (use Bearer user:<email> in dev)",
        )
        .with_request_id(request_id));
    };

    let rows = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT o.org_id, o.name, o.resource_version, o.created_at, o.updated_at
        FROM orgs_view o
        INNER JOIN org_members_view m ON m.org_id = o.org_id
        WHERE m.email = $1 AND NOT m.is_deleted
        ORDER BY o.org_id ASC
        LIMIT 200
        "#,
    )
    .bind(email)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            email = %email,
            "Failed to list orgs"
        );
        ApiError::internal("internal_error", "Failed to list organizations")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<OrgResponse> = rows
        .into_iter()
        .map(|row| OrgResponse {
            id: row.org_id,
            name: row.name,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect();

    let total = items.len() as i64;

    Ok(Json(ListOrgsResponse { items, total }))
}

/// Get a single organization by ID.
///
/// GET /v1/orgs/{org_id}
async fn get_org(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    // Validate org_id format
    let org_id_typed: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    // Query the orgs_view table
    let result = sqlx::query_as::<_, OrgRow>(
        r#"
        SELECT org_id, name, resource_version, created_at, updated_at
        FROM orgs_view
        WHERE org_id = $1
        "#,
    )
    .bind(&org_id)
    .fetch_optional(state.db().pool())
    .await;

    match result {
        Ok(Some(row)) => {
            let _role = authz::require_org_member(&state, &org_id_typed, &ctx).await?;

            Ok(Json(OrgResponse {
                id: row.org_id,
                name: row.name,
                resource_version: row.resource_version,
                created_at: row.created_at,
                updated_at: row.updated_at,
            }))
        }
        Ok(None) => Err(ApiError::not_found(
            "org_not_found",
            format!("Organization {} not found", org_id),
        )
        .with_request_id(request_id)),
        Err(e) => {
            tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to get org");
            Err(
                ApiError::internal("internal_error", "Failed to get organization")
                    .with_request_id(request_id),
            )
        }
    }
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from orgs_view table.
struct OrgRow {
    org_id: String,
    name: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for OrgRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_org_request_deserialization() {
        let json = r#"{"name": "Acme Corp"}"#;
        let req: CreateOrgRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "Acme Corp");
    }

    #[test]
    fn test_org_response_serialization() {
        let response = OrgResponse {
            id: "org_123".to_string(),
            name: "Test Org".to_string(),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"org_123\""));
    }
}
