//! plfm-vt Node Agent
//!
//! The node agent runs on each bare-metal host and manages workload lifecycle.
//! It receives desired state from the control plane and converges the node
//! to match that state by booting/stopping Firecracker microVMs.

use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
mod heartbeat;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt node agent");

    // Load configuration
    let config = config::Config::from_env()?;
    info!(
        node_id = %config.node_id,
        control_plane_url = %config.control_plane_url,
        "Configuration loaded"
    );

    // Start heartbeat loop
    let heartbeat_handle = tokio::spawn(heartbeat::run_heartbeat_loop(config.clone()));

    // TODO: Start reconciliation loop
    // TODO: Start health check loop
    // TODO: Start exec session handler

    // Wait for heartbeat to complete (it shouldn't unless there's an error)
    heartbeat_handle.await??;

    Ok(())
}
