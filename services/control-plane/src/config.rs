use std::net::SocketAddr;

use anyhow::Result;

use crate::db::DbConfig;

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub grpc_listen_addr: SocketAddr,
    pub log_level: String,
    pub dev_mode: bool,
    pub database: DbConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let listen_addr = std::env::var("GHOST_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()?;

        let grpc_listen_addr = std::env::var("GHOST_GRPC_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:9090".to_string())
            .parse()?;

        let log_level = std::env::var("GHOST_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let dev_mode = std::env::var("GHOST_DEV")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        let database = DbConfig::from_env();

        Ok(Self {
            listen_addr,
            grpc_listen_addr,
            log_level,
            dev_mode,
            database,
        })
    }
}
