//! Application state shared across request handlers.

use std::sync::Arc;

use crate::db::Database;

/// Shared application state.
///
/// This is passed to all request handlers via Axum's state extractor.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    db: Database,
}

impl AppState {
    /// Create a new application state.
    pub fn new(db: Database) -> Self {
        Self {
            inner: Arc::new(AppStateInner { db }),
        }
    }

    /// Get a reference to the database.
    pub fn db(&self) -> &Database {
        &self.inner.db
    }
}
