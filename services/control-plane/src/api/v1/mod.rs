//! API v1 routes.

mod apps;
mod envs;
mod orgs;

use axum::Router;

use crate::state::AppState;

/// Create API v1 routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/orgs", orgs::routes())
        // Apps are nested under orgs: /v1/orgs/{org_id}/apps
        .nest("/orgs/{org_id}/apps", apps::routes())
        // Envs are nested under apps: /v1/apps/{app_id}/envs
        .nest("/apps/{app_id}/envs", envs::routes())
}
