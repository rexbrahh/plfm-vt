//! Ingress configuration.
//!
//! For now, ingress focuses on consuming routing-related events from the control plane.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};

/// Ingress configuration (env-driven).
#[derive(Debug, Clone)]
pub struct Config {
    /// Control plane base URL (example: http://localhost:8080).
    pub control_plane_url: String,

    /// Organization ID to sync routes for (stub mode).
    pub org_id: String,

    /// Max events to fetch per poll.
    pub fetch_limit: i64,

    /// Poll interval when no new events are available.
    pub poll_interval: Duration,

    /// Optional cursor file to persist last applied event_id.
    pub cursor_file: Option<PathBuf>,

    /// Exit once fully caught up.
    pub once: bool,

    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let control_plane_url = std::env::var("GHOST_CONTROL_PLANE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());

        let org_id = std::env::var("GHOST_ORG_ID")
            .or_else(|_| std::env::var("VT_ORG"))
            .context("Missing org id. Set GHOST_ORG_ID (or VT_ORG for dev convenience).")?;

        let fetch_limit: i64 = std::env::var("GHOST_SYNC_LIMIT")
            .ok()
            .map(|v| v.parse())
            .transpose()
            .context("GHOST_SYNC_LIMIT must be an integer.")?
            .unwrap_or(200)
            .clamp(1, 200);

        let poll_interval_ms: u64 = std::env::var("GHOST_SYNC_POLL_INTERVAL_MS")
            .ok()
            .map(|v| v.parse())
            .transpose()
            .context("GHOST_SYNC_POLL_INTERVAL_MS must be an integer (milliseconds).")?
            .unwrap_or(1000);
        let poll_interval = Duration::from_millis(poll_interval_ms.max(50));

        let cursor_file = std::env::var("GHOST_SYNC_CURSOR_FILE")
            .ok()
            .map(PathBuf::from);

        let once = std::env::var("GHOST_SYNC_ONCE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        Ok(Self {
            control_plane_url,
            org_id,
            fetch_limit,
            poll_interval,
            cursor_file,
            once,
            log_level,
        })
    }
}
