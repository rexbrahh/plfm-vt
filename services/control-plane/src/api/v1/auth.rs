//! Authentication and identity endpoints.
//!
//! For now, we expose `whoami` as the primary introspection surface.

use axum::{response::IntoResponse, routing::get, Json, Router};
use plfm_events::ActorType;
use serde::Serialize;

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

async fn whoami(ctx: RequestContext) -> Result<impl IntoResponse, ApiError> {
    let RequestContext {
        request_id,
        actor_type,
        actor_id,
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

    Ok(Json(WhoAmIResponse {
        subject_type,
        subject_id: actor_id,
        display_name: None,
        org_memberships: Vec::new(),
        scopes: Vec::new(),
    }))
}
