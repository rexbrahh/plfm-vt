//! Authentication and identity endpoints.
//!
//! For now, we expose `whoami` as the primary introspection surface.

use std::collections::BTreeSet;

use axum::{extract::State, response::IntoResponse, routing::get, Json, Router};
use plfm_events::{ActorType, MemberRole};
use serde::Serialize;

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/whoami", get(whoami))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SubjectType {
    User,
    ServicePrincipal,
}

#[derive(Debug, Serialize)]
struct OrgMembership {
    org_id: String,
    role: String,
}

#[derive(Debug, Serialize)]
struct WhoAmIResponse {
    subject_type: SubjectType,
    subject_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    org_memberships: Vec<OrgMembership>,
    scopes: Vec<String>,
}

fn scopes_for_role(role: MemberRole) -> &'static [&'static str] {
    match role {
        MemberRole::Owner | MemberRole::Admin => &[
            "orgs:read",
            "orgs:admin",
            "apps:read",
            "apps:write",
            "envs:read",
            "envs:write",
            "releases:read",
            "releases:write",
            "deploys:write",
            "rollbacks:write",
            "routes:read",
            "routes:write",
            "volumes:read",
            "volumes:write",
            "secrets:read-metadata",
            "secrets:write",
            "logs:read",
        ],
        MemberRole::Developer => &[
            "orgs:read",
            "apps:read",
            "apps:write",
            "envs:read",
            "envs:write",
            "releases:read",
            "releases:write",
            "deploys:write",
            "rollbacks:write",
            "routes:read",
            "routes:write",
            "volumes:read",
            "volumes:write",
            "secrets:read-metadata",
            "secrets:write",
            "logs:read",
        ],
        MemberRole::Readonly => &[
            "orgs:read",
            "apps:read",
            "envs:read",
            "releases:read",
            "routes:read",
            "volumes:read",
            "secrets:read-metadata",
            "logs:read",
        ],
    }
}

async fn whoami(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<impl IntoResponse, ApiError> {
    let RequestContext {
        request_id,
        actor_type,
        actor_id,
        actor_email,
        ..
    } = ctx;

    let subject_type = match actor_type {
        ActorType::User => SubjectType::User,
        ActorType::ServicePrincipal => SubjectType::ServicePrincipal,
        ActorType::System => {
            return Err(ApiError::unauthorized(
                "unauthorized",
                "Missing or invalid Authorization token",
            )
            .with_request_id(request_id));
        }
    };

    let mut org_memberships: Vec<OrgMembership> = Vec::new();
    let mut scopes: BTreeSet<String> = BTreeSet::new();
    let mut display_name: Option<String> = None;

    if actor_type == ActorType::User {
        let Some(email) = actor_email.as_deref() else {
            return Err(ApiError::unauthorized(
                "unauthorized",
                "Token subject email is required for org-scoped APIs (use Bearer user:<email> in dev)",
            )
            .with_request_id(request_id));
        };

        display_name = Some(email.to_string());

        let rows = sqlx::query_as::<_, OrgMembershipRow>(
            r#"
            SELECT org_id, role
            FROM org_members_view
            WHERE email = $1 AND NOT is_deleted
            ORDER BY org_id ASC
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
                "Failed to load org memberships"
            );
            ApiError::internal("internal_error", "Failed to load identity")
                .with_request_id(request_id.clone())
        })?;

        for row in rows {
            if let Some(role) = authz::parse_member_role(&row.role) {
                for scope in scopes_for_role(role) {
                    scopes.insert(scope.to_string());
                }
            }

            org_memberships.push(OrgMembership {
                org_id: row.org_id,
                role: row.role,
            });
        }
    }

    Ok(Json(WhoAmIResponse {
        subject_type,
        subject_id: actor_id,
        display_name,
        org_memberships,
        scopes: scopes.into_iter().collect(),
    }))
}

#[derive(Debug)]
struct OrgMembershipRow {
    org_id: String,
    role: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for OrgMembershipRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            role: row.try_get("role")?,
        })
    }
}
