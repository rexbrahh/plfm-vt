//! L4 TCP proxy implementation.
//!
//! This module provides:
//! - TCP listener management
//! - SNI inspection for TLS passthrough
//! - Backend selection and load balancing
//! - PROXY protocol v2 injection
//! - Connection proxying
//!
//! ## Architecture
//!
//! ```text
//! Client -> Listener -> SNI Inspector -> Router -> Backend Pool -> Backend
//!                                                      |
//!                                          PROXY v2 Header (if enabled)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! use proxy::{Listener, ListenerConfig, RouteTable, BackendSelector};
//!
//! let route_table = Arc::new(RouteTable::new());
//! let backend_selector = Arc::new(BackendSelector::new());
//!
//! let config = ListenerConfig::new("[::]:443".parse()?);
//! let listener = Listener::bind(config, route_table, backend_selector).await?;
//! listener.run().await?;
//! ```

mod backend;
mod listener;
mod proxy_protocol;
mod router;
mod sni;

pub use backend::{Backend, BackendPool, BackendPoolStats, BackendSelector, HealthStatus};
pub use listener::{Listener, ListenerConfig, ListenerStats};
pub use proxy_protocol::ProxyProtocolV2;
pub use router::{
    ProtocolHint, ProxyProtocol, Route, RouteTable, RoutingDecision, SharedRouteTable,
};
pub use sni::{SniConfig, SniInspector, SniResult};
