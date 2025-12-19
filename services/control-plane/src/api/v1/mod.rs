//! API v1 routes.

mod apps;
mod auth;
mod debug;
mod deploys;
mod envs;
mod events;
mod instances;
mod logs;
mod members;
mod nodes;
mod orgs;
mod projects;
mod releases;
mod routes;

use axum::Router;

use crate::state::AppState;

/// Create API v1 routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/auth", auth::routes())
        .nest("/orgs", orgs::routes())
        .nest("/orgs/:org_id/members", members::routes())
        .nest("/orgs/:org_id/projects", projects::routes())
        .route(
            "/orgs/:org_id/events",
            axum::routing::get(events::list_events),
        )
        .route(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/logs",
            axum::routing::get(logs::query_logs),
        )
        .route(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/logs/stream",
            axum::routing::get(logs::stream_logs),
        )
        .route(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/rollbacks",
            axum::routing::post(deploys::create_rollback),
        )
        // Apps are nested under orgs: /v1/orgs/{org_id}/apps
        .nest("/orgs/:org_id/apps", apps::routes())
        // Envs are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/envs
        .nest("/orgs/:org_id/apps/:app_id/envs", envs::routes())
        // Releases are nested under apps: /v1/orgs/{org_id}/apps/{app_id}/releases
        .nest("/orgs/:org_id/apps/:app_id/releases", releases::routes())
        // Deploys are nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys
        .nest(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/deploys",
            deploys::routes(),
        )
        // Routes are nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/routes
        .nest(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/routes",
            routes::routes(),
        )
        // Scale is nested under envs: /v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale
        .nest(
            "/orgs/:org_id/apps/:app_id/envs/:env_id/scale",
            envs::scale_routes(),
        )
        // Nodes are infrastructure resources: /v1/nodes
        .nest("/nodes", nodes::routes())
        // Instances are VM instances: /v1/instances
        .nest("/instances", instances::routes())
        // Development/debug endpoints: /v1/_debug/*
        .nest("/_debug", debug::routes())
}
