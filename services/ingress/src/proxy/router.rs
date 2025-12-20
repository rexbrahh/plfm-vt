//! Route table and routing decisions.
//!
//! This module manages the routing table derived from control plane state
//! and makes routing decisions based on listener port and SNI hostname.
//!
//! Per spec (docs/specs/networking/ingress-l4.md):
//! - Exact hostname match only (no wildcards in v1)
//! - Hostnames normalized to lowercase, trailing dot trimmed
//! - Routes bind hostname+port to environment/backend
//! - Config updates must be applied atomically
//! - Config reload must not drop established connections
//!
//! Reference: docs/specs/networking/ingress-l4.md

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{debug, info, warn};

/// Protocol hint for a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolHint {
    /// TLS passthrough with SNI inspection.
    TlsPassthrough,
    /// Raw TCP without payload inspection.
    TcpRaw,
}

/// PROXY protocol configuration for a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyProtocol {
    /// PROXY protocol disabled.
    Off,
    /// PROXY protocol v2 enabled.
    V2,
}

impl Default for ProxyProtocol {
    fn default() -> Self {
        Self::Off
    }
}

/// A route configuration.
#[derive(Debug, Clone)]
pub struct Route {
    /// Unique route identifier.
    pub id: String,
    /// Hostname for this route (normalized).
    pub hostname: String,
    /// Listener port.
    pub port: u16,
    /// Protocol hint.
    pub protocol: ProtocolHint,
    /// PROXY protocol configuration.
    pub proxy_protocol: ProxyProtocol,
    /// Application ID this route belongs to.
    pub app_id: String,
    /// Environment ID this route belongs to.
    pub env_id: String,
    /// Backend process type to route to.
    pub backend_process_type: String,
    /// Backend port inside the microVM.
    pub backend_port: u16,
    /// Whether to allow non-TLS fallback (only if unambiguous).
    pub allow_non_tls_fallback: bool,
}

impl Route {
    /// Normalize a hostname for matching.
    ///
    /// - Convert to lowercase
    /// - Trim trailing dot
    pub fn normalize_hostname(hostname: &str) -> String {
        hostname.to_lowercase().trim_end_matches('.').to_string()
    }
}

/// Result of a routing decision.
#[derive(Debug, Clone)]
pub enum RoutingDecision {
    /// Route found, proceed with connection.
    Matched { route: Route },
    /// No matching route found.
    NoMatch { reason: String },
    /// Routing is ambiguous (multiple routes, no SNI).
    Ambiguous { reason: String },
}

/// Key for route lookup (port + optional hostname).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    port: u16,
    hostname: Option<String>,
}

/// Immutable snapshot of route data for lock-free reads.
#[derive(Debug, Default)]
struct RouteSnapshot {
    /// Routes indexed by (port, hostname).
    by_key: HashMap<RouteKey, Route>,
    /// Routes indexed by port only (for fallback lookup).
    by_port: HashMap<u16, Vec<Route>>,
    /// All routes indexed by ID.
    by_id: HashMap<String, Route>,
}

impl RouteSnapshot {
    /// Create a new snapshot from a list of routes.
    fn from_routes(routes: Vec<Route>) -> Self {
        let mut by_key = HashMap::new();
        let mut by_port: HashMap<u16, Vec<Route>> = HashMap::new();
        let mut by_id = HashMap::new();

        for route in routes {
            let key = RouteKey {
                port: route.port,
                hostname: Some(route.hostname.clone()),
            };

            by_key.insert(key, route.clone());
            by_port.entry(route.port).or_default().push(route.clone());
            by_id.insert(route.id.clone(), route);
        }

        Self {
            by_key,
            by_port,
            by_id,
        }
    }

    /// Create a new snapshot with a route added/updated.
    fn with_upsert(&self, route: Route) -> Self {
        let mut by_key = self.by_key.clone();
        let mut by_port = self.by_port.clone();
        let mut by_id = self.by_id.clone();

        let key = RouteKey {
            port: route.port,
            hostname: Some(route.hostname.clone()),
        };

        by_key.insert(key, route.clone());

        // Update port index
        let port_routes = by_port.entry(route.port).or_default();
        port_routes.retain(|r| r.id != route.id);
        port_routes.push(route.clone());

        by_id.insert(route.id.clone(), route);

        Self {
            by_key,
            by_port,
            by_id,
        }
    }

    /// Create a new snapshot with a route removed.
    fn without(&self, route_id: &str) -> Self {
        let route = match self.by_id.get(route_id) {
            Some(r) => r.clone(),
            None => {
                return Self {
                    by_key: self.by_key.clone(),
                    by_port: self.by_port.clone(),
                    by_id: self.by_id.clone(),
                }
            }
        };

        let mut by_key = self.by_key.clone();
        let mut by_port = self.by_port.clone();
        let mut by_id = self.by_id.clone();

        let key = RouteKey {
            port: route.port,
            hostname: Some(route.hostname.clone()),
        };

        by_key.remove(&key);
        by_id.remove(route_id);

        if let Some(port_routes) = by_port.get_mut(&route.port) {
            port_routes.retain(|r| r.id != route_id);
            if port_routes.is_empty() {
                by_port.remove(&route.port);
            }
        }

        Self {
            by_key,
            by_port,
            by_id,
        }
    }
}

/// Route table managing all active routes.
///
/// Uses ArcSwap for lock-free atomic config updates.
/// Readers get consistent snapshots without blocking.
/// Writers atomically swap in new snapshots.
pub struct RouteTable {
    /// Atomically swappable route snapshot.
    snapshot: ArcSwap<RouteSnapshot>,
}

impl RouteTable {
    /// Create a new empty route table.
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(RouteSnapshot::default()),
        }
    }

    /// Update the route table with a new set of routes.
    ///
    /// This replaces the entire route table atomically in a single
    /// pointer swap. Existing readers continue to see the old snapshot
    /// until they finish, then the old snapshot is dropped.
    pub async fn update(&self, routes: Vec<Route>) {
        let route_count = routes.len();
        let new_snapshot = Arc::new(RouteSnapshot::from_routes(routes));

        // Atomic swap - readers get consistent snapshots
        self.snapshot.store(new_snapshot);

        info!(route_count = route_count, "Route table updated atomically");
    }

    /// Add or update a single route atomically.
    pub async fn upsert(&self, route: Route) {
        // Load current, compute new, swap atomically
        let current = self.snapshot.load();
        let new_snapshot = Arc::new(current.with_upsert(route));
        self.snapshot.store(new_snapshot);
    }

    /// Remove a route by ID atomically.
    pub async fn remove(&self, route_id: &str) {
        let current = self.snapshot.load();
        let new_snapshot = Arc::new(current.without(route_id));
        self.snapshot.store(new_snapshot);
    }

    /// Get a route by ID.
    pub async fn get(&self, route_id: &str) -> Option<Route> {
        let snapshot = self.snapshot.load();
        snapshot.by_id.get(route_id).cloned()
    }

    /// Make a routing decision based on listener address and optional SNI.
    ///
    /// This is lock-free and can be called from hot path without blocking.
    ///
    /// # Arguments
    /// * `listener_addr` - The address the connection arrived on
    /// * `sni` - Optional SNI hostname extracted from TLS ClientHello
    pub async fn route(&self, listener_addr: SocketAddr, sni: Option<&str>) -> RoutingDecision {
        let port = listener_addr.port();
        let snapshot = self.snapshot.load();

        // Try exact match with SNI
        if let Some(hostname) = sni {
            let normalized = Route::normalize_hostname(hostname);
            let key = RouteKey {
                port,
                hostname: Some(normalized.clone()),
            };

            if let Some(route) = snapshot.by_key.get(&key) {
                debug!(
                    route_id = %route.id,
                    hostname = %normalized,
                    port = port,
                    "Route matched by SNI"
                );
                return RoutingDecision::Matched {
                    route: route.clone(),
                };
            }

            // No match for this hostname
            return RoutingDecision::NoMatch {
                reason: format!("No route for hostname '{}' on port {}", normalized, port),
            };
        }

        // No SNI - check if routing is unambiguous
        match snapshot.by_port.get(&port) {
            None => RoutingDecision::NoMatch {
                reason: format!("No routes bound to port {}", port),
            },
            Some(routes) if routes.is_empty() => RoutingDecision::NoMatch {
                reason: format!("No routes bound to port {}", port),
            },
            Some(routes) if routes.len() == 1 => {
                let route = &routes[0];
                // Check if fallback is allowed
                if route.protocol == ProtocolHint::TlsPassthrough && !route.allow_non_tls_fallback {
                    warn!(
                        route_id = %route.id,
                        port = port,
                        "TLS passthrough route without SNI and fallback disabled"
                    );
                    return RoutingDecision::NoMatch {
                        reason: format!(
                            "TLS route on port {} requires SNI but none provided",
                            port
                        ),
                    };
                }

                debug!(
                    route_id = %route.id,
                    port = port,
                    "Route matched (unambiguous, no SNI)"
                );
                RoutingDecision::Matched {
                    route: route.clone(),
                }
            }
            Some(routes) => {
                // Multiple routes, ambiguous without SNI
                RoutingDecision::Ambiguous {
                    reason: format!(
                        "Multiple routes ({}) bound to port {}, SNI required",
                        routes.len(),
                        port
                    ),
                }
            }
        }
    }

    /// Get all routes for a specific port.
    pub async fn routes_for_port(&self, port: u16) -> Vec<Route> {
        let snapshot = self.snapshot.load();
        snapshot.by_port.get(&port).cloned().unwrap_or_default()
    }

    /// Get all configured ports.
    pub async fn ports(&self) -> Vec<u16> {
        let snapshot = self.snapshot.load();
        snapshot.by_port.keys().copied().collect()
    }

    /// Get the total number of routes.
    pub async fn len(&self) -> usize {
        let snapshot = self.snapshot.load();
        snapshot.by_id.len()
    }

    /// Check if the route table is empty.
    pub async fn is_empty(&self) -> bool {
        let snapshot = self.snapshot.load();
        snapshot.by_id.is_empty()
    }

    /// Get all route IDs.
    pub async fn route_ids(&self) -> Vec<String> {
        let snapshot = self.snapshot.load();
        snapshot.by_id.keys().cloned().collect()
    }
}

impl Default for RouteTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared route table reference.
pub type SharedRouteTable = Arc<RouteTable>;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_route(id: &str, hostname: &str, port: u16) -> Route {
        Route {
            id: id.to_string(),
            hostname: Route::normalize_hostname(hostname),
            port,
            protocol: ProtocolHint::TlsPassthrough,
            proxy_protocol: ProxyProtocol::Off,
            app_id: "app-1".to_string(),
            env_id: "env-1".to_string(),
            backend_process_type: "web".to_string(),
            backend_port: 8080,
            allow_non_tls_fallback: false,
        }
    }

    #[test]
    fn test_normalize_hostname() {
        assert_eq!(Route::normalize_hostname("Example.COM"), "example.com");
        assert_eq!(Route::normalize_hostname("example.com."), "example.com");
        assert_eq!(Route::normalize_hostname("EXAMPLE.COM."), "example.com");
    }

    #[tokio::test]
    async fn test_route_table_update() {
        let table = RouteTable::new();

        let routes = vec![
            make_route("r1", "example.com", 443),
            make_route("r2", "example.org", 443),
        ];

        table.update(routes).await;

        assert_eq!(table.len().await, 2);
        assert!(table.get("r1").await.is_some());
        assert!(table.get("r2").await.is_some());
    }

    #[tokio::test]
    async fn test_route_with_sni() {
        let table = RouteTable::new();
        table.upsert(make_route("r1", "example.com", 443)).await;
        table.upsert(make_route("r2", "example.org", 443)).await;

        let addr: SocketAddr = "[::]:443".parse().unwrap();

        // Match with SNI
        match table.route(addr, Some("example.com")).await {
            RoutingDecision::Matched { route } => {
                assert_eq!(route.id, "r1");
            }
            other => panic!("Expected Matched, got {:?}", other),
        }

        // No match
        match table.route(addr, Some("unknown.com")).await {
            RoutingDecision::NoMatch { .. } => {}
            other => panic!("Expected NoMatch, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_route_without_sni_ambiguous() {
        let table = RouteTable::new();
        table.upsert(make_route("r1", "example.com", 443)).await;
        table.upsert(make_route("r2", "example.org", 443)).await;

        let addr: SocketAddr = "[::]:443".parse().unwrap();

        // Without SNI, should be ambiguous
        match table.route(addr, None).await {
            RoutingDecision::Ambiguous { .. } => {}
            other => panic!("Expected Ambiguous, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_route_without_sni_unambiguous() {
        let table = RouteTable::new();

        // Single route with fallback allowed
        let mut route = make_route("r1", "example.com", 443);
        route.allow_non_tls_fallback = true;
        table.upsert(route).await;

        let addr: SocketAddr = "[::]:443".parse().unwrap();

        // Without SNI, should match the single route
        match table.route(addr, None).await {
            RoutingDecision::Matched { route } => {
                assert_eq!(route.id, "r1");
            }
            other => panic!("Expected Matched, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_route_remove() {
        let table = RouteTable::new();
        table.upsert(make_route("r1", "example.com", 443)).await;

        assert!(table.get("r1").await.is_some());

        table.remove("r1").await;

        assert!(table.get("r1").await.is_none());
        assert!(table.is_empty().await);
    }

    #[tokio::test]
    async fn test_raw_tcp_route() {
        let table = RouteTable::new();

        let mut route = make_route("r1", "any", 5432);
        route.protocol = ProtocolHint::TcpRaw;
        route.allow_non_tls_fallback = true;
        table.upsert(route).await;

        let addr: SocketAddr = "[::]:5432".parse().unwrap();

        // Raw TCP routes without SNI should match if unambiguous
        match table.route(addr, None).await {
            RoutingDecision::Matched { route } => {
                assert_eq!(route.protocol, ProtocolHint::TcpRaw);
            }
            other => panic!("Expected Matched, got {:?}", other),
        }
    }
}
