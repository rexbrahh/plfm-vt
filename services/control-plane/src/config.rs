//! Configuration for the control plane.

use std::net::SocketAddr;

use anyhow::Result;

/// Control plane configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to listen on for HTTP connections.
    pub listen_addr: SocketAddr,

    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,

    /// Whether we're in development mode.
    pub dev_mode: bool,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let listen_addr = std::env::var("GHOST_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()?;

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let dev_mode = std::env::var("GHOST_DEV")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        Ok(Self {
            listen_addr,
            log_level,
            dev_mode,
        })
    }
}
