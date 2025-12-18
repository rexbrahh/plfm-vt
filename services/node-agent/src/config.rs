//! Configuration for the node agent.

use anyhow::Result;
use plfm_id::NodeId;

/// Node agent configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Unique identifier for this node.
    pub node_id: NodeId,

    /// Control plane API URL.
    pub control_plane_url: String,

    /// Data directory for local state.
    pub data_dir: String,

    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,

    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        // Node ID can be provided or auto-generated
        let node_id = std::env::var("GHOST_NODE_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(NodeId::new);

        let control_plane_url = std::env::var("GHOST_CONTROL_PLANE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());

        let data_dir =
            std::env::var("GHOST_DATA_DIR").unwrap_or_else(|_| "/var/lib/ghost".to_string());

        let heartbeat_interval_secs = std::env::var("GHOST_HEARTBEAT_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        Ok(Self {
            node_id,
            control_plane_url,
            data_dir,
            heartbeat_interval_secs,
            log_level,
        })
    }
}
