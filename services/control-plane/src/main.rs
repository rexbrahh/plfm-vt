//! plfm-vt Control Plane
//!
//! The control plane is the central coordination service for the platform.
//! It provides the REST API for all platform operations and drives
//! reconciliation of desired vs current state.

use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod api;
mod config;

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

    // Build and run the server
    let app = api::create_router();

    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!(addr = %config.listen_addr, "Listening for connections");

    axum::serve(listener, app).await?;

    Ok(())
}
