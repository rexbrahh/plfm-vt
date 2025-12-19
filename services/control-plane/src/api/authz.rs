//! Authorization helpers (v1).
//!
//! v1 uses org-scoped membership for tenant isolation.

use plfm_events::MemberRole;
use plfm_id::OrgId;

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

pub fn parse_member_role(role: &str) -> Option<MemberRole> {
    match role {
        "owner" => Some(MemberRole::Owner),
        "admin" => Some(MemberRole::Admin),
        "developer" => Some(MemberRole::Developer),
        "readonly" => Some(MemberRole::Readonly),
        _ => None,
    }
}

pub fn member_role_label(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Owner => "owner",
        MemberRole::Admin => "admin",
        MemberRole::Developer => "developer",
        MemberRole::Readonly => "readonly",
    }
}

pub fn require_authenticated(ctx: &RequestContext) -> Result<(), ApiError> {
    if ctx.actor_type == plfm_events::ActorType::System {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "Missing or invalid Authorization token",
        )
        .with_request_id(ctx.request_id.clone()));
    }
    Ok(())
}

pub async fn require_org_member(
    state: &AppState,
    org_id: &OrgId,
    ctx: &RequestContext,
) -> Result<MemberRole, ApiError> {
    require_authenticated(ctx)?;

    let request_id = &ctx.request_id;
    let Some(email) = ctx.actor_email.as_deref() else {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "Token subject email is required for org-scoped APIs (use Bearer user:<email> in dev)",
        )
        .with_request_id(request_id.clone()));
    };

    let role: Option<String> = sqlx::query_scalar(
        r#"
        SELECT role
        FROM org_members_view
        WHERE org_id = $1 AND email = $2 AND NOT is_deleted
        "#,
    )
    .bind(org_id.to_string())
    .bind(email)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(
            error = %e,
            request_id = %request_id,
            org_id = %org_id,
            email = %email,
            "Failed to load org membership"
        );
        ApiError::internal("internal_error", "Failed to authorize request")
            .with_request_id(request_id.clone())
    })?;

    let Some(role) = role else {
        return Err(ApiError::forbidden("forbidden", "Not a member of this org")
            .with_request_id(request_id.clone()));
    };

    parse_member_role(&role).ok_or_else(|| {
        ApiError::internal("internal_error", "Invalid membership role")
            .with_request_id(request_id.clone())
    })
}

pub fn require_org_write(role: MemberRole, request_id: &str) -> Result<(), ApiError> {
    match role {
        MemberRole::Owner | MemberRole::Admin | MemberRole::Developer => Ok(()),
        MemberRole::Readonly => Err(ApiError::forbidden(
            "forbidden",
            "Insufficient permissions for write operation",
        )
        .with_request_id(request_id.to_string())),
    }
}

pub fn require_org_admin(role: MemberRole, request_id: &str) -> Result<(), ApiError> {
    match role {
        MemberRole::Owner | MemberRole::Admin => Ok(()),
        MemberRole::Developer | MemberRole::Readonly => Err(ApiError::forbidden(
            "forbidden",
            "Admin role required for this operation",
        )
        .with_request_id(request_id.to_string())),
    }
}
