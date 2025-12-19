//! plfm-vt Ingress
//!
//! L4 proxy with TLS passthrough and SNI routing.
//!
//! This service:
//! - Syncs route configuration from the control plane
//! - Accepts TCP connections on configured listeners
//! - Inspects TLS ClientHello for SNI-based routing
//! - Proxies connections to backend instances
//! - Optionally injects PROXY protocol v2 headers

use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
pub mod proxy;
mod sync;

// Re-export proxy types for external use
pub use proxy::{
    Backend, BackendPool, BackendSelector, Listener, ListenerConfig, ProtocolHint,
    ProxyProtocol, ProxyProtocolV2, Route, RouteTable, RoutingDecision, SharedRouteTable,
    SniConfig, SniInspector, SniResult,
};

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
