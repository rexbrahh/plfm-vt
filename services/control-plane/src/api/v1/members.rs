//! Organization membership API endpoints.
//!
//! Provides listing and management of org members.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{
    event_types, AggregateType, MemberRole, OrgMemberAddedPayload, OrgMemberRemovedPayload,
    OrgMemberRoleUpdatedPayload,
};
use plfm_id::{MemberId, OrgId};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_members))
        .route("/", post(create_member))
        .route("/{member_id}", axum::routing::patch(update_member))
        .route("/{member_id}", axum::routing::delete(delete_member))
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListMembersQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateMemberRequest {
    pub email: String,
    pub role: MemberRole,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateMemberRequest {
    pub role: MemberRole,
    pub expected_version: i32,
}

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    pub id: String,
    pub org_id: String,
    pub email: String,
    pub role: String,
    pub resource_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListMembersResponse {
    pub items: Vec<MemberResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeleteResponse {
    ok: bool,
}

// =============================================================================
// Handlers
// =============================================================================

async fn list_members(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Query(query): Query<ListMembersQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let _role = authz::require_org_member(&state, &org_id, &ctx).await?;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor;

    let rows = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT member_id, org_id, email, role, resource_version, created_at, updated_at, is_deleted
        FROM org_members_view
        WHERE org_id = $1
          AND NOT is_deleted
          AND ($2::text IS NULL OR member_id > $2)
        ORDER BY member_id ASC
        LIMIT $3
        "#,
    )
    .bind(org_id.to_string())
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to list members");
        ApiError::internal("internal_error", "Failed to list members")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<MemberResponse> = rows.into_iter().map(MemberResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|m| m.id.clone())
    } else {
        None
    };

    Ok(Json(ListMembersResponse { items, next_cursor }))
}

async fn create_member(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(org_id): Path<String>,
    Json(req): Json<CreateMemberRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let endpoint_name = "members.create";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;

    let caller_role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_admin(caller_role, &request_id)?;

    let email = req.email.trim().to_string();
    if email.is_empty() || email.len() > 320 || !email.contains('@') {
        return Err(
            ApiError::bad_request("invalid_email", "Invalid email format")
                .with_request_id(request_id),
        );
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

    let existing: Option<String> = sqlx::query_scalar(
        r#"
        SELECT member_id
        FROM org_members_view
        WHERE org_id = $1 AND email = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_scope.clone())
    .bind(&email)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            email = %email,
            "Failed to check existing membership"
        );
        ApiError::internal("internal_error", "Failed to create member")
            .with_request_id(request_id.clone())
    })?;

    if existing.is_some() {
        return Err(ApiError::conflict(
            "member_already_exists",
            "Member already exists for this org",
        )
        .with_request_id(request_id));
    }

    let member_id = MemberId::new();
    let payload = OrgMemberAddedPayload {
        member_id,
        org_id,
        email: email.clone(),
        role: req.role,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize member payload");
        ApiError::internal("internal_error", "Failed to create member")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
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
        payload,
        ..Default::default()
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, member_id = %member_id, "Failed to add member");
        ApiError::internal("internal_error", "Failed to create member")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "members",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT member_id, org_id, email, role, resource_version, created_at, updated_at, is_deleted
        FROM org_members_view
        WHERE member_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(member_id.to_string())
    .bind(org_scope.clone())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load member");
        ApiError::internal("internal_error", "Failed to create member")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Member was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = MemberResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to create member")
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

async fn update_member(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, member_id)): Path<(String, String)>,
    Json(req): Json<UpdateMemberRequest>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let endpoint_name = "members.update";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let member_id_typed: MemberId = member_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_member_id", "Invalid member ID format")
            .with_request_id(request_id.clone())
    })?;

    let org_scope = org_id.to_string();

    let caller_role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_admin(caller_role, &request_id)?;

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

    let current = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT member_id, org_id, email, role, resource_version, created_at, updated_at, is_deleted
        FROM org_members_view
        WHERE member_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(member_id_typed.to_string())
    .bind(org_scope.clone())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load member");
        ApiError::internal("internal_error", "Failed to update member")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("member_not_found", "Member not found")
            .with_request_id(request_id.clone())
    })?;

    if req.expected_version != current.resource_version {
        return Err(
            ApiError::conflict("version_conflict", "Resource version mismatch")
                .with_request_id(request_id),
        );
    }

    let old_role = authz::parse_member_role(&current.role).ok_or_else(|| {
        ApiError::internal("internal_error", "Invalid membership role")
            .with_request_id(request_id.clone())
    })?;

    if old_role == req.role {
        let response = MemberResponse::from(current);
        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    if old_role == MemberRole::Owner && req.role != MemberRole::Owner {
        let owners: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM org_members_view
            WHERE org_id = $1 AND role = 'owner' AND NOT is_deleted
            "#,
        )
        .bind(org_scope.clone())
        .fetch_one(state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to count owners");
            ApiError::internal("internal_error", "Failed to update member")
                .with_request_id(request_id.clone())
        })?;

        if owners <= 1 {
            return Err(ApiError::conflict(
                "last_owner",
                "Cannot remove the last owner from the org",
            )
            .with_request_id(request_id));
        }
    }

    let payload = OrgMemberRoleUpdatedPayload {
        member_id: member_id_typed,
        org_id,
        old_role,
        new_role: req.role,
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize role update payload");
        ApiError::internal("internal_error", "Failed to update member")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::OrgMember,
        aggregate_id: member_id_typed.to_string(),
        aggregate_seq: current.resource_version + 1,
        event_type: event_types::ORG_MEMBER_ROLE_UPDATED.to_string(),
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
        ..Default::default()
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, member_id = %member_id_typed, "Failed to update member");
        ApiError::internal("internal_error", "Failed to update member")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "members",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let row = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT member_id, org_id, email, role, resource_version, created_at, updated_at, is_deleted
        FROM org_members_view
        WHERE member_id = $1 AND org_id = $2 AND NOT is_deleted
        "#,
    )
    .bind(member_id_typed.to_string())
    .bind(org_scope.clone())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load member");
        ApiError::internal("internal_error", "Failed to update member")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::internal("internal_error", "Member was not materialized")
            .with_request_id(request_id.clone())
    })?;

    let response = MemberResponse::from(row);

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to update member")
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

async fn delete_member(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, member_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let endpoint_name = "members.delete";

    let org_id: OrgId = org_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_org_id", "Invalid organization ID format")
            .with_request_id(request_id.clone())
    })?;
    let member_id_typed: MemberId = member_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_member_id", "Invalid member ID format")
            .with_request_id(request_id.clone())
    })?;

    let org_scope = org_id.to_string();

    let caller_role = authz::require_org_member(&state, &org_id, &ctx).await?;
    authz::require_org_admin(caller_role, &request_id)?;

    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({
                "member_id": member_id_typed.to_string()
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

    let current = sqlx::query_as::<_, MemberRow>(
        r#"
        SELECT member_id, org_id, email, role, resource_version, created_at, updated_at, is_deleted
        FROM org_members_view
        WHERE member_id = $1 AND org_id = $2
        "#,
    )
    .bind(member_id_typed.to_string())
    .bind(org_scope.clone())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load member");
        ApiError::internal("internal_error", "Failed to delete member")
            .with_request_id(request_id.clone())
    })?
    .ok_or_else(|| {
        ApiError::not_found("member_not_found", "Member not found")
            .with_request_id(request_id.clone())
    })?;

    if current.role == "owner" && !current.is_deleted {
        let owners: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM org_members_view
            WHERE org_id = $1 AND role = 'owner' AND NOT is_deleted
            "#,
        )
        .bind(org_scope.clone())
        .fetch_one(state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, org_id = %org_id, "Failed to count owners");
            ApiError::internal("internal_error", "Failed to delete member")
                .with_request_id(request_id.clone())
        })?;

        if owners <= 1 {
            return Err(ApiError::conflict(
                "last_owner",
                "Cannot remove the last owner from the org",
            )
            .with_request_id(request_id));
        }
    }

    if current.is_deleted {
        let response = DeleteResponse { ok: true };
        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    let payload = OrgMemberRemovedPayload {
        member_id: member_id_typed,
        org_id,
        email: current.email.clone(),
    };

    let payload = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to serialize remove payload");
        ApiError::internal("internal_error", "Failed to delete member")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::OrgMember,
        aggregate_id: member_id_typed.to_string(),
        aggregate_seq: current.resource_version + 1,
        event_type: event_types::ORG_MEMBER_REMOVED.to_string(),
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
        ..Default::default()
    };

    let event_id = state.db().event_store().append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, member_id = %member_id_typed, "Failed to remove member");
        ApiError::internal("internal_error", "Failed to delete member")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "members",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let response = DeleteResponse { ok: true };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to delete member")
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
// Database Row Types
// =============================================================================

#[derive(Debug)]
struct MemberRow {
    member_id: String,
    org_id: String,
    email: String,
    role: String,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    is_deleted: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for MemberRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            member_id: row.try_get("member_id")?,
            org_id: row.try_get("org_id")?,
            email: row.try_get("email")?,
            role: row.try_get("role")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            is_deleted: row.try_get("is_deleted")?,
        })
    }
}

impl From<MemberRow> for MemberResponse {
    fn from(row: MemberRow) -> Self {
        Self {
            id: row.member_id,
            org_id: row.org_id,
            email: row.email,
            role: row.role,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
