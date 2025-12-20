//! Ingress configuration.
//!
//! For now, ingress focuses on consuming routing-related events from the control plane.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct RedactedString(String);

impl RedactedString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Listener configuration for a single port.
#[derive(Debug, Clone)]
pub struct ListenerBinding {
    /// Address to bind to.
    pub bind_addr: SocketAddr,
    /// Maximum concurrent connections.
    pub max_connections: usize,
}

/// Ingress configuration (env-driven).
#[derive(Debug, Clone)]
pub struct Config {
    /// Control plane base URL (example: http://localhost:8080).
    pub control_plane_url: String,

    /// Optional bearer token for control-plane API access (dev stub).
    pub control_plane_token: Option<RedactedString>,

    /// Organization ID to sync routes for (stub mode).
    pub org_id: String,

    /// Max events to fetch per poll.
    pub fetch_limit: i64,

    /// Poll interval when no new events are available.
    pub poll_interval: Duration,

    /// Optional cursor file to persist last applied event_id (deprecated, use state_file).
    pub cursor_file: Option<PathBuf>,

    /// Optional state file to persist full route state for atomic reload.
    pub state_file: Option<PathBuf>,

    /// Exit once fully caught up (sync mode only).
    pub once: bool,

    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,

    /// Listener bindings (address:port pairs).
    pub listeners: Vec<ListenerBinding>,

    /// Enable proxy mode (start listeners). If false, only sync routes.
    pub proxy_enabled: bool,

    /// Backend sync interval (how often to refresh backend instance lists).
    pub backend_sync_interval: Duration,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let control_plane_url = std::env::var("GHOST_CONTROL_PLANE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());

        let control_plane_token = std::env::var("GHOST_API_TOKEN")
            .or_else(|_| std::env::var("VT_TOKEN"))
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(RedactedString::new);

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

        // State file for full route persistence (for atomic reload on restart)
        let state_file = std::env::var("GHOST_STATE_FILE")
            .ok()
            .map(PathBuf::from);

        let once = std::env::var("GHOST_SYNC_ONCE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        // Parse listener bindings from GHOST_LISTENERS (comma-separated addr:port)
        // Example: "[::]:443,[::]:80"
        let listeners = parse_listeners(
            std::env::var("GHOST_LISTENERS")
                .ok()
                .as_deref()
                .unwrap_or("[::]:443"),
        )?;

        // Enable proxy mode by default (set GHOST_PROXY_ENABLED=false for sync-only)
        let proxy_enabled = std::env::var("GHOST_PROXY_ENABLED")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);

        // Backend sync interval (default 5s)
        let backend_sync_interval_ms: u64 = std::env::var("GHOST_BACKEND_SYNC_INTERVAL_MS")
            .ok()
            .map(|v| v.parse())
            .transpose()
            .context("GHOST_BACKEND_SYNC_INTERVAL_MS must be an integer (milliseconds).")?
            .unwrap_or(5000);
        let backend_sync_interval = Duration::from_millis(backend_sync_interval_ms.max(1000));

        Ok(Self {
            control_plane_url,
            control_plane_token,
            org_id,
            fetch_limit,
            poll_interval,
            cursor_file,
            state_file,
            once,
            log_level,
            listeners,
            proxy_enabled,
            backend_sync_interval,
        })
    }
}

/// Parse listener bindings from a comma-separated string.
fn parse_listeners(s: &str) -> Result<Vec<ListenerBinding>> {
    let mut listeners = Vec::new();

    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let bind_addr: SocketAddr = part
            .parse()
            .with_context(|| format!("Invalid listener address: {}", part))?;

        listeners.push(ListenerBinding {
            bind_addr,
            max_connections: 10000, // Default max connections
        });
    }

    if listeners.is_empty() {
        anyhow::bail!("No listeners configured. Set GHOST_LISTENERS (e.g., '[::]:443')");
    }

    Ok(listeners)
}
