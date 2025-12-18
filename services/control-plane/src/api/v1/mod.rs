//! API v1 routes.

mod orgs;

use axum::Router;

use crate::state::AppState;

/// Create API v1 routes.
pub fn routes() -> Router<AppState> {
    Router::new().nest("/orgs", orgs::routes())
}
