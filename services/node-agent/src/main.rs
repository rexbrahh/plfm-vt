//! plfm-vt Node Agent
//!
//! The node agent runs on each bare-metal host and manages workload lifecycle.
//! It receives desired state from the control plane and converges the node
//! to match that state by booting/stopping Firecracker microVMs.
//!
//! ## Architecture
//!
//! - **Heartbeat Loop**: Reports node status to control plane periodically
//! - **Reconciler**: Fetches plans and applies them via the instance manager
//! - **Instance Manager**: Tracks desired vs actual state of instances
//! - **Runtime**: Abstracts VM lifecycle operations (mock in dev, Firecracker in prod)

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod client;
mod config;
mod heartbeat;
mod instance;
mod reconciler;
mod runtime;

use instance::InstanceManager;
use reconciler::{Reconciler, ReconcilerConfig};
use runtime::MockRuntime;

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
        data_dir = %config.data_dir,
        "Configuration loaded"
    );

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create the runtime (mock for now)
    let runtime = Arc::new(MockRuntime::new());

    // Create the instance manager
    let instance_manager = Arc::new(InstanceManager::new(runtime));

    // Start the heartbeat loop
    let heartbeat_handle = tokio::spawn({
        let config = config.clone();
        let instance_manager = Arc::clone(&instance_manager);
        let shutdown_rx = shutdown_rx.clone();
        async move {
            heartbeat::run_heartbeat_loop(config, instance_manager, shutdown_rx).await
        }
    });

    // Start the reconciliation loop
    let reconciler = Reconciler::new(
        &config,
        Arc::clone(&instance_manager),
        ReconcilerConfig::default(),
    );
    let reconciler_handle = tokio::spawn({
        let shutdown_rx = shutdown_rx.clone();
        async move {
            reconciler.run(shutdown_rx).await;
        }
    });

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
        result = heartbeat_handle => {
            match result {
                Ok(Ok(())) => info!("Heartbeat loop exited normally"),
                Ok(Err(e)) => error!(error = %e, "Heartbeat loop error"),
                Err(e) => error!(error = %e, "Heartbeat task panicked"),
            }
        }
        _ = reconciler_handle => {
            info!("Reconciler exited");
        }
    }

    // Signal shutdown to all workers
    let _ = shutdown_tx.send(true);

    // Give workers time to shut down gracefully
    info!("Waiting for workers to shut down...");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    info!("Node agent shutdown complete");
    Ok(())
}
