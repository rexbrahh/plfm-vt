//! plfm-vt Control Plane
//!
//! The control plane is the central coordination service for the platform.
//! It provides the REST API for all platform operations and drives
//! reconciliation of desired vs current state.

use anyhow::Result;
use plfm_control_plane::{
    api,
    cleanup::{CleanupWorker, CleanupWorkerConfig},
    config,
    db::Database,
    projections::{worker::WorkerConfig, ProjectionWorker},
    scheduler::SchedulerWorker,
    state::AppState,
};
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = config::Config::from_env()?;

    // Initialize tracing (prefer RUST_LOG, fallback to GHOST_LOG_LEVEL)
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| config.log_level.clone().into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt control plane");
    info!(listen_addr = %config.listen_addr, "Configuration loaded");

    // Connect to database
    let db = match Database::connect(&config.database).await {
        Ok(db) => {
            info!("Database connection established");
            db
        }
        Err(e) => {
            error!(error = %e, "Failed to connect to database");
            return Err(e.into());
        }
    };

    // Run migrations in dev mode
    if config.dev_mode {
        info!("Running database migrations (dev mode)");
        if let Err(e) = db.run_migrations().await {
            error!(error = %e, "Failed to run migrations");
            return Err(e.into());
        }
    }

    // Create shutdown channel for graceful shutdown
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start projection worker in background
    let projection_worker = ProjectionWorker::new(db.pool().clone(), WorkerConfig::default());
    let projection_handle = tokio::spawn({
        let shutdown_rx = shutdown_rx.clone();
        async move {
            if let Err(e) = projection_worker.run(shutdown_rx).await {
                error!(error = %e, "Projection worker failed");
            }
        }
    });

    // Start scheduler worker in background
    let scheduler_worker =
        SchedulerWorker::new(db.pool().clone(), std::time::Duration::from_secs(5));
    let scheduler_handle = tokio::spawn({
        let shutdown_rx = shutdown_rx.clone();
        async move {
            scheduler_worker.run(shutdown_rx).await;
        }
    });

    // Start cleanup worker in background
    let cleanup_worker = CleanupWorker::new(db.pool().clone(), CleanupWorkerConfig::default());
    let cleanup_handle = tokio::spawn({
        let shutdown_rx = shutdown_rx.clone();
        async move {
            cleanup_worker.run(shutdown_rx).await;
        }
    });

    // Create application state
    let state = AppState::new(db);

    // Build and run the server
    let app = api::create_router(state);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!(addr = %config.listen_addr, "Listening for connections");

    // Spawn the server with graceful shutdown
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut shutdown_rx = shutdown_rx;
                loop {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
                info!("HTTP server shutting down");
            })
            .await
    });

    // Wait for shutdown signal (Ctrl+C)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal");
        }
        result = server_handle => {
            match result {
                Ok(Ok(())) => info!("Server exited normally"),
                Ok(Err(e)) => error!(error = %e, "Server error"),
                Err(e) => error!(error = %e, "Server task panicked"),
            }
        }
    }

    // Signal shutdown to all workers
    let _ = shutdown_tx.send(true);

    // Wait for workers to finish
    info!("Waiting for workers to shut down...");
    let shutdown_timeout = std::time::Duration::from_secs(10);

    if let Err(e) = tokio::time::timeout(shutdown_timeout, projection_handle).await {
        warn!(error = %e, "Projection worker did not shut down in time");
    }

    if let Err(e) = tokio::time::timeout(shutdown_timeout, scheduler_handle).await {
        warn!(error = %e, "Scheduler worker did not shut down in time");
    }

    if let Err(e) = tokio::time::timeout(shutdown_timeout, cleanup_handle).await {
        warn!(error = %e, "Cleanup worker did not shut down in time");
    }

    info!("Control plane shutdown complete");
    Ok(())
}
