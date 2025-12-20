//! plfm-vt Node Agent Binary
//!
//! This is the main entry point for the node agent.
//! See the library crate (`plfm_node_agent`) for documentation.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// Use the library crate
use plfm_node_agent::actors::NodeSupervisor;
use plfm_node_agent::config::Config;
use plfm_node_agent::heartbeat;
use plfm_node_agent::reconciler::{Reconciler, ReconcilerConfig};
use plfm_node_agent::{InstanceManager, MockRuntime};

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = Config::from_env()?;

    // Initialize tracing (prefer RUST_LOG, fallback to GHOST_LOG_LEVEL)
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| config.log_level.clone().into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt node agent");
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

    // Determine whether to use actor-based supervision or legacy mode
    let use_actors = std::env::var("VT_USE_ACTORS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    if use_actors {
        // === Actor-based supervision tree ===
        info!("Using actor-based supervision tree");

        let mut supervisor = NodeSupervisor::new(config.clone(), runtime, shutdown_rx.clone());
        supervisor.start();

        // Handle shutdown signal
        let supervisor_handle = tokio::spawn(async move {
            supervisor.run().await;
        });

        // Wait for shutdown signal
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Received shutdown signal");
            }
            _ = supervisor_handle => {
                info!("Supervisor exited");
            }
        }

        // Signal shutdown
        let _ = shutdown_tx.send(true);
    } else {
        // === Legacy mode (backward compatible) ===
        info!("Using legacy reconciliation mode");

        // Create the instance manager
        let instance_manager = Arc::new(InstanceManager::new(runtime));

        // Start the heartbeat loop
        let heartbeat_handle = tokio::spawn({
            let config = config.clone();
            let instance_manager = Arc::clone(&instance_manager);
            let shutdown_rx = shutdown_rx.clone();
            async move { heartbeat::run_heartbeat_loop(config, instance_manager, shutdown_rx).await }
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
    }

    info!("Node agent shutdown complete");
    Ok(())
}
