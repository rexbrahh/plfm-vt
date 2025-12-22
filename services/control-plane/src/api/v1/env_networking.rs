use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::AggregateType;
use plfm_id::{AppId, EnvId, OrgId, Ulid};
use serde::Serialize;

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::idempotency;
use crate::api::request_context::RequestContext;
use crate::db::quotas::{check_quota, QuotaDimension};
use crate::db::AppendEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_networking))
        .route("/ipv4", post(enable_ipv4))
        .route("/ipv4", delete(disable_ipv4))
}

#[derive(Debug, Serialize)]
pub struct NetworkingStateResponse {
    pub env_id: String,
    pub org_id: String,
    pub app_id: String,
    pub ipv4_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_allocation_id: Option<String>,
    pub resource_version: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct Ipv4EnabledResponse {
    pub env_id: String,
    pub allocation_id: String,
    pub ipv4_address: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Ipv4DisabledResponse {
    pub env_id: String,
    pub message: String,
}

async fn get_networking(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
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

    let row = sqlx::query_as::<_, NetworkingRow>(
        r#"
        SELECT env_id, org_id, app_id, ipv4_enabled,
               host(ipv4_address)::TEXT as ipv4_address,
               ipv4_allocation_id, resource_version, updated_at
        FROM env_networking_view
        WHERE env_id = $1 AND org_id = $2 AND app_id = $3
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get networking state");
        ApiError::internal("internal_error", "Failed to get networking state")
            .with_request_id(request_id.clone())
    })?;

    let response = match row {
        Some(row) => NetworkingStateResponse {
            env_id: row.env_id,
            org_id: row.org_id,
            app_id: row.app_id,
            ipv4_enabled: row.ipv4_enabled,
            ipv4_address: row.ipv4_address,
            ipv4_allocation_id: row.ipv4_allocation_id,
            resource_version: row.resource_version,
            updated_at: row.updated_at,
        },
        None => NetworkingStateResponse {
            env_id: env_id.to_string(),
            org_id: org_id.to_string(),
            app_id: app_id.to_string(),
            ipv4_enabled: false,
            ipv4_address: None,
            ipv4_allocation_id: None,
            resource_version: 0,
            updated_at: Utc::now(),
        },
    };

    Ok(Json(response))
}

async fn enable_ipv4(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "envs.ipv4_enable";

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

    let env_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM envs_view WHERE env_id = $1 AND org_id = $2 AND app_id = $3 AND NOT is_deleted)",
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check env existence");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })?;

    if !env_exists {
        return Err(ApiError::not_found(
            "env_not_found",
            format!("Environment {} not found", env_id),
        )
        .with_request_id(request_id.clone()));
    }

    if let Some(exceeded) = check_quota(
        state.db().pool(),
        &org_id,
        QuotaDimension::MaxIpv4Allocations,
        1,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check quota");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })? {
        return Err(ApiError::conflict(
            "quota_exceeded",
            format!(
                "Quota exceeded for {}: limit={}, current={}, requested={}",
                exceeded.dimension,
                exceeded.limit,
                exceeded.current_usage,
                exceeded.requested_delta
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let already_enabled: Option<String> = sqlx::query_scalar(
        "SELECT ipv4_allocation_id FROM env_networking_view WHERE env_id = $1 AND ipv4_enabled = true",
    )
    .bind(env_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check IPv4 status");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })?;

    if let Some(allocation_id) = already_enabled {
        let existing: NetworkingRow = sqlx::query_as(
            r#"
            SELECT env_id, org_id, app_id, ipv4_enabled,
                   host(ipv4_address)::TEXT as ipv4_address,
                   ipv4_allocation_id, resource_version, updated_at
            FROM env_networking_view WHERE env_id = $1
            "#,
        )
        .bind(env_id.to_string())
        .fetch_one(state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to get existing allocation");
            ApiError::internal("internal_error", "Failed to enable IPv4")
                .with_request_id(request_id.clone())
        })?;

        return Ok((
            StatusCode::OK,
            Json(Ipv4EnabledResponse {
                env_id: env_id.to_string(),
                allocation_id,
                ipv4_address: existing.ipv4_address.unwrap_or_default(),
                message: "IPv4 is already enabled for this environment".to_string(),
            }),
        )
            .into_response());
    }

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({ "env_id": env_id.to_string() });
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

    let ipv4_address: Option<String> = sqlx::query_scalar(
        r#"
        SELECT host(ipv4_address)::TEXT
        FROM ipam_ipv4_pool
        WHERE is_available = true
          AND ipv4_address NOT IN (
              SELECT ipv4_address FROM ipam_ipv4_allocations
              WHERE released_at IS NULL OR cooldown_until > now()
          )
        ORDER BY ipv4_address
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to allocate IPv4");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })?;

    let ipv4_address = ipv4_address.ok_or_else(|| {
        ApiError::conflict(
            "ipv4_pool_exhausted",
            "No IPv4 addresses available in the pool",
        )
        .with_request_id(request_id.clone())
    })?;

    let allocation_id = format!("ipv4_{}", Ulid::new().to_string().to_lowercase());

    sqlx::query(
        r#"
        INSERT INTO ipam_ipv4_allocations (allocation_id, env_id, org_id, ipv4_address, allocated_at)
        VALUES ($1, $2, $3, $4::INET, now())
        "#,
    )
    .bind(&allocation_id)
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(&ipv4_address)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to insert IPv4 allocation");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })?;

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Env, &env_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to enable IPv4")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: "env.ipv4_addon_enabled".to_string(),
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
            "env_id": env_id.to_string(),
            "org_id": org_id.to_string(),
            "app_id": app_id.to_string(),
            "allocation_id": &allocation_id,
            "ipv4_address": &ipv4_address
        }),
        ..Default::default()
    };

    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to append event");
        ApiError::internal("internal_error", "Failed to enable IPv4")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "env_networking",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let response = Ipv4EnabledResponse {
        env_id: env_id.to_string(),
        allocation_id: allocation_id.clone(),
        ipv4_address: ipv4_address.clone(),
        message: "IPv4 add-on enabled successfully".to_string(),
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to enable IPv4")
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

async fn disable_ipv4(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((org_id, app_id, env_id)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    let request_id = ctx.request_id.clone();
    let idempotency_key = ctx.idempotency_key.clone();
    let actor_type = ctx.actor_type;
    let actor_id = ctx.actor_id.clone();
    let endpoint_name = "envs.ipv4_disable";

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

    let current: Option<NetworkingRow> = sqlx::query_as(
        r#"
        SELECT env_id, org_id, app_id, ipv4_enabled,
               host(ipv4_address)::TEXT as ipv4_address,
               ipv4_allocation_id, resource_version, updated_at
        FROM env_networking_view
        WHERE env_id = $1 AND org_id = $2 AND app_id = $3
        "#,
    )
    .bind(env_id.to_string())
    .bind(org_id.to_string())
    .bind(app_id.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get networking state");
        ApiError::internal("internal_error", "Failed to disable IPv4")
            .with_request_id(request_id.clone())
    })?;

    let current = current.ok_or_else(|| {
        ApiError::not_found("env_not_found", format!("Environment {} not found", env_id))
            .with_request_id(request_id.clone())
    })?;

    if !current.ipv4_enabled {
        return Ok((
            StatusCode::OK,
            Json(Ipv4DisabledResponse {
                env_id: env_id.to_string(),
                message: "IPv4 is already disabled for this environment".to_string(),
            }),
        )
            .into_response());
    }

    let ipv4_required_routes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM routes_view WHERE env_id = $1 AND ipv4_required = true AND NOT is_deleted",
    )
    .bind(env_id.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check routes");
        ApiError::internal("internal_error", "Failed to disable IPv4")
            .with_request_id(request_id.clone())
    })?;

    if ipv4_required_routes > 0 {
        return Err(ApiError::conflict(
            "routes_require_ipv4",
            format!(
                "Cannot disable IPv4: {} route(s) require IPv4. Delete those routes first.",
                ipv4_required_routes
            ),
        )
        .with_request_id(request_id.clone()));
    }

    let allocation_id = current.ipv4_allocation_id.clone().unwrap_or_default();

    let org_scope = org_id.to_string();
    let request_hash = idempotency_key
        .as_deref()
        .map(|key| {
            let hash_input = serde_json::json!({ "env_id": env_id.to_string() });
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

    sqlx::query(
        r#"
        UPDATE ipam_ipv4_allocations
        SET released_at = now(),
            cooldown_until = now() + INTERVAL '24 hours'
        WHERE allocation_id = $1 AND released_at IS NULL
        "#,
    )
    .bind(&allocation_id)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to release IPv4 allocation");
        ApiError::internal("internal_error", "Failed to disable IPv4")
            .with_request_id(request_id.clone())
    })?;

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Env, &env_id.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to disable IPv4")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let event = AppendEvent {
        aggregate_type: AggregateType::Env,
        aggregate_id: env_id.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: "env.ipv4_addon_disabled".to_string(),
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
            "env_id": env_id.to_string(),
            "org_id": org_id.to_string(),
            "allocation_id": &allocation_id
        }),
        ..Default::default()
    };

    let event_id = event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to append event");
        ApiError::internal("internal_error", "Failed to disable IPv4")
            .with_request_id(request_id.clone())
    })?;

    state
        .db()
        .projection_store()
        .wait_for_checkpoint(
            "env_networking",
            event_id.value(),
            crate::api::projection_wait_timeout(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Projection wait failed");
            ApiError::gateway_timeout("projection_timeout", "Request timed out waiting for state")
                .with_request_id(request_id.clone())
        })?;

    let response = Ipv4DisabledResponse {
        env_id: env_id.to_string(),
        message: "IPv4 add-on disabled successfully".to_string(),
    };

    if let Some((key, hash)) = request_hash {
        let body = serde_json::to_value(&response).map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to serialize response");
            ApiError::internal("internal_error", "Failed to disable IPv4")
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

struct NetworkingRow {
    env_id: String,
    org_id: String,
    app_id: String,
    ipv4_enabled: bool,
    ipv4_address: Option<String>,
    ipv4_allocation_id: Option<String>,
    resource_version: i32,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for NetworkingRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            env_id: row.try_get("env_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            ipv4_enabled: row.try_get("ipv4_enabled")?,
            ipv4_address: row.try_get("ipv4_address")?,
            ipv4_allocation_id: row.try_get("ipv4_allocation_id")?,
            resource_version: row.try_get("resource_version")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_networking_state_response_serialization() {
        let response = NetworkingStateResponse {
            env_id: "env_123".to_string(),
            org_id: "org_456".to_string(),
            app_id: "app_789".to_string(),
            ipv4_enabled: true,
            ipv4_address: Some("203.0.113.10".to_string()),
            ipv4_allocation_id: Some("ipv4_abc".to_string()),
            resource_version: 2,
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"ipv4_enabled\":true"));
        assert!(json.contains("\"ipv4_address\":\"203.0.113.10\""));
    }

    #[test]
    fn test_networking_state_response_without_ipv4() {
        let response = NetworkingStateResponse {
            env_id: "env_123".to_string(),
            org_id: "org_456".to_string(),
            app_id: "app_789".to_string(),
            ipv4_enabled: false,
            ipv4_address: None,
            ipv4_allocation_id: None,
            resource_version: 1,
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"ipv4_enabled\":false"));
        assert!(!json.contains("\"ipv4_address\""));
    }
}
