//! Projection worker and handlers.
//!
//! This module provides the background worker that reads events from the event log
//! and updates materialized views. Each projection:
//!
//! - Maintains a durable checkpoint of the last processed event_id
//! - Processes events in order, updating view tables
//! - Handles restarts by resuming from checkpoint
//!
//! See: docs/specs/state/materialized-views.md

mod apps;
mod deploys;
mod env_config;
mod envs;
mod instances;
mod members;
mod nodes;
mod orgs;
mod projects;
mod releases;
mod restore_jobs;
mod routes;
mod secret_bundles;
mod snapshots;
mod volume_attachments;
mod volumes;
pub mod worker;

pub use worker::ProjectionWorker;

use async_trait::async_trait;

use crate::db::{DbError, EventRow};

/// Result type for projection operations.
pub type ProjectionResult<T> = Result<T, ProjectionError>;

/// Errors that can occur during projection processing.
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("database error: {0}")]
    Database(#[from] DbError),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("invalid event payload: {0}")]
    InvalidPayload(String),
}

/// Trait for projection handlers.
///
/// Each handler processes specific event types and updates the corresponding view table.
#[async_trait]
pub trait ProjectionHandler: Send + Sync {
    /// The name of this projection (used for checkpointing).
    fn name(&self) -> &'static str;

    /// The event types this handler processes.
    fn event_types(&self) -> &'static [&'static str];

    /// Apply a single event to the view.
    ///
    /// This is called within a transaction that also updates the checkpoint.
    async fn apply(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &EventRow,
    ) -> ProjectionResult<()>;
}

/// Registry of all projection handlers.
pub struct ProjectionRegistry {
    handlers: Vec<Box<dyn ProjectionHandler>>,
}

impl ProjectionRegistry {
    /// Create a new registry with all standard handlers.
    pub fn new() -> Self {
        Self {
            handlers: vec![
                Box::new(orgs::OrgsProjection),
                Box::new(members::MembersProjection),
                Box::new(projects::ProjectsProjection),
                Box::new(apps::AppsProjection),
                Box::new(envs::EnvsProjection),
                Box::new(releases::ReleasesProjection),
                Box::new(deploys::DeploysProjection),
                Box::new(nodes::NodesProjection),
                Box::new(instances::InstancesProjection),
                Box::new(env_config::EnvConfigProjection),
                Box::new(routes::RoutesProjection),
                Box::new(secret_bundles::SecretBundlesProjection),
                Box::new(volumes::VolumesProjection),
                Box::new(volume_attachments::VolumeAttachmentsProjection),
                Box::new(snapshots::SnapshotsProjection),
                Box::new(restore_jobs::RestoreJobsProjection),
            ],
        }
    }

    /// Get the handler for a given event type.
    pub fn handler_for(&self, event_type: &str) -> Option<&dyn ProjectionHandler> {
        for handler in &self.handlers {
            if handler.event_types().contains(&event_type) {
                return Some(handler.as_ref());
            }
        }
        None
    }

    /// Get all handlers.
    pub fn handlers(&self) -> &[Box<dyn ProjectionHandler>] {
        &self.handlers
    }

    /// Get all unique projection names.
    pub fn projection_names(&self) -> Vec<&'static str> {
        self.handlers.iter().map(|h| h.name()).collect()
    }
}

impl Default for ProjectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_contains_handlers() {
        let registry = ProjectionRegistry::new();
        assert!(!registry.handlers().is_empty());
    }

    #[test]
    fn test_registry_finds_org_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("org.created").is_some());
    }

    #[test]
    fn test_registry_finds_project_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("project.created").is_some());
    }

    #[test]
    fn test_registry_finds_app_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("app.created").is_some());
    }

    #[test]
    fn test_registry_finds_env_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("env.created").is_some());
    }

    #[test]
    fn test_registry_returns_none_for_unknown() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("unknown.event").is_none());
    }

    #[test]
    fn test_registry_finds_release_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("release.created").is_some());
    }

    #[test]
    fn test_registry_finds_deploy_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("deploy.created").is_some());
        assert!(registry.handler_for("deploy.status_changed").is_some());
    }

    #[test]
    fn test_registry_finds_node_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("node.enrolled").is_some());
        assert!(registry.handler_for("node.state_changed").is_some());
        assert!(registry.handler_for("node.capacity_updated").is_some());
    }

    #[test]
    fn test_registry_finds_instance_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("instance.allocated").is_some());
        assert!(registry
            .handler_for("instance.desired_state_changed")
            .is_some());
    }

    #[test]
    fn test_registry_finds_env_config_handler() {
        let registry = ProjectionRegistry::new();
        assert!(registry.handler_for("env.desired_release_set").is_some());
        assert!(registry.handler_for("env.scale_set").is_some());
    }
}
