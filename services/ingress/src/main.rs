//! plfm-vt Ingress
//!
//! The long-term ingress goal is an L4 proxy (TLS passthrough + SNI routing).
//! For now, this binary implements a minimal sync loop that consumes route
//! events from the control plane and prints deterministic diffs.

use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
mod sync;

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::Config::from_env()?;

    // Initialize tracing (prefer RUST_LOG, fallback to GHOST_LOG_LEVEL)
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| config.log_level.clone().into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt ingress (route sync)");
    info!(
        control_plane_url = %config.control_plane_url,
        org_id = %config.org_id,
        fetch_limit = config.fetch_limit,
        poll_interval_ms = config.poll_interval.as_millis() as u64,
        cursor_file = ?config.cursor_file,
        once = config.once,
        "Configuration loaded"
    );

    sync::run_route_sync_loop(&config).await
}
