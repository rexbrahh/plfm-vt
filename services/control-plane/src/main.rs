//! plfm-vt Control Plane
//!
//! The control plane is the central coordination service for the platform.
//! It provides the REST API for all platform operations and drives
//! reconciliation of desired vs current state.

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod api;
mod config;
mod db;
mod state;

use db::Database;
use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt control plane");

    // Load configuration
    let config = config::Config::from_env()?;
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

    // Create application state
    let state = AppState::new(db);

    // Build and run the server
    let app = api::create_router(state);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!(addr = %config.listen_addr, "Listening for connections");

    axum::serve(listener, app).await?;

    Ok(())
}
