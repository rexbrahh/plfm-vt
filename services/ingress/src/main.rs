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

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
mod persistence;
pub mod proxy;
mod sync;

// Re-export proxy types for external use
pub use proxy::{
    Backend, BackendPool, BackendSelector, Listener, ListenerConfig, ProtocolHint, ProxyProtocol,
    ProxyProtocolV2, Route, RouteTable, RoutingDecision, SharedRouteTable, SniConfig, SniInspector,
    SniResult,
};

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::Config::from_env()?;

    // Initialize tracing (prefer RUST_LOG, fallback to GHOST_LOG_LEVEL)
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| config.log_level.clone().into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    info!("Starting plfm-vt ingress");
    info!(
        control_plane_url = %config.control_plane_url,
        org_id = %config.org_id,
        proxy_enabled = config.proxy_enabled,
        listener_count = config.listeners.len(),
        "Configuration loaded"
    );

    // Create shared state
    let route_table = Arc::new(RouteTable::new());
    let backend_selector = Arc::new(BackendSelector::new());

    if config.proxy_enabled {
        // Start listeners
        let mut listener_handles = Vec::new();

        for binding in &config.listeners {
            let mut listener_config = ListenerConfig::new(binding.bind_addr);
            listener_config.max_connections = binding.max_connections;

            match Listener::bind(
                listener_config,
                Arc::clone(&route_table),
                Arc::clone(&backend_selector),
            )
            .await
            {
                Ok(listener) => {
                    info!(
                        bind_addr = %binding.bind_addr,
                        "Listener bound"
                    );
                    let listener = Arc::new(listener);
                    let handle = tokio::spawn(async move {
                        if let Err(e) = listener.run().await {
                            error!(error = %e, "Listener error");
                        }
                    });
                    listener_handles.push(handle);
                }
                Err(e) => {
                    error!(
                        bind_addr = %binding.bind_addr,
                        error = %e,
                        "Failed to bind listener"
                    );
                    return Err(e.into());
                }
            }
        }

        // Start backend sync loop
        let backend_config = config.clone();
        let backend_route_table = Arc::clone(&route_table);
        let backend_selector_clone = Arc::clone(&backend_selector);
        tokio::spawn(async move {
            if let Err(e) = sync::run_backend_sync_loop(
                backend_config,
                backend_route_table,
                backend_selector_clone,
            )
            .await
            {
                error!(error = %e, "Backend sync loop failed");
            }
        });

        // Run route sync loop (blocks until error or shutdown)
        sync::run_route_sync_loop(&config, route_table, backend_selector).await
    } else {
        // Sync-only mode (for debugging/testing)
        info!("Running in sync-only mode (proxy disabled)");
        sync::run_route_sync_loop(&config, route_table, backend_selector).await
    }
}
