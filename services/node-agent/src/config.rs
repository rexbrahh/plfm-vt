use anyhow::Result;
use plfm_id::NodeId;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub node_id: NodeId,
    pub control_plane_url: String,
    pub control_plane_grpc_url: String,
    pub data_dir: String,
    pub heartbeat_interval_secs: u64,
    pub log_level: String,
    pub exec_listen_addr: SocketAddr,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let node_id = std::env::var("GHOST_NODE_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let control_plane_url = std::env::var("GHOST_CONTROL_PLANE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());

        let control_plane_grpc_url = std::env::var("GHOST_CONTROL_PLANE_GRPC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:9090".to_string());

        let data_dir =
            std::env::var("GHOST_DATA_DIR").unwrap_or_else(|_| "/var/lib/ghost".to_string());

        let heartbeat_interval_secs = std::env::var("GHOST_HEARTBEAT_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let exec_listen_addr = std::env::var("GHOST_EXEC_LISTEN_ADDR")
            .or_else(|_| std::env::var("PLFM_EXEC_LISTEN_ADDR"))
            .unwrap_or_else(|_| "0.0.0.0:5090".to_string())
            .parse()?;

        Ok(Self {
            node_id,
            control_plane_url,
            control_plane_grpc_url,
            data_dir,
            heartbeat_interval_secs,
            log_level,
            exec_listen_addr,
        })
    }
}
