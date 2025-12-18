//! API v1 routes.

mod apps;
mod deploys;
mod envs;
mod nodes;
mod orgs;
mod releases;

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
        // Releases are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/releases
        .nest("/orgs/{org_id}/apps/{app_id}/releases", releases::routes())
        // Deploys are nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys
        .nest("/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys", deploys::routes())
        // Nodes are infrastructure resources: /v1/nodes
        .nest("/nodes", nodes::routes())
}
