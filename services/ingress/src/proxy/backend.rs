//! Backend pool management and selection.
//!
//! This module manages backend endpoints for routes and provides
//! load balancing across healthy instances.
//!
//! Per spec (docs/specs/networking/ingress-l4.md):
//! - Round-robin among eligible backends
//! - Backend is eligible only if instance status=ready
//! - Connect timeout to backend: 2s default
//!
//! Reference: docs/specs/networking/ingress-l4.md

use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Default connect timeout for backend connections.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A backend endpoint representing a workload instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Backend {
    /// Overlay IPv6 address of the instance.
    pub overlay_ipv6: Ipv6Addr,
    /// Backend port inside the microVM.
    pub port: u16,
    /// Instance ID for tracking.
    pub instance_id: String,
}

impl Backend {
    /// Create a new backend endpoint.
    pub fn new(overlay_ipv6: Ipv6Addr, port: u16, instance_id: String) -> Self {
        Self {
            overlay_ipv6,
            port,
            instance_id,
        }
    }

    /// Get the socket address for this backend.
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::V6(SocketAddrV6::new(self.overlay_ipv6, self.port, 0, 0))
    }
}

/// Health status of a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Backend is healthy and eligible for traffic.
    Healthy,
    /// Backend is unhealthy (failed probe or connection).
    Unhealthy,
    /// Backend health is unknown (not yet probed).
    Unknown,
}

/// Internal state for a backend in the pool.
struct BackendState {
    backend: Backend,
    health: HealthStatus,
    last_failure: Option<Instant>,
    consecutive_failures: u32,
}

/// A pool of backends for a single route.
pub struct BackendPool {
    /// Route identifier.
    route_id: String,
    /// Backends in this pool.
    backends: RwLock<Vec<BackendState>>,
    /// Round-robin counter.
    rr_counter: AtomicUsize,
    /// Connect timeout.
    connect_timeout: Duration,
    /// Total connections attempted.
    connections_attempted: AtomicU64,
    /// Total connections succeeded.
    connections_succeeded: AtomicU64,
}

impl BackendPool {
    /// Create a new backend pool for a route.
    pub fn new(route_id: String) -> Self {
        Self {
            route_id,
            backends: RwLock::new(Vec::new()),
            rr_counter: AtomicUsize::new(0),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            connections_attempted: AtomicU64::new(0),
            connections_succeeded: AtomicU64::new(0),
        }
    }

    /// Create a new backend pool with custom connect timeout.
    pub fn with_timeout(route_id: String, connect_timeout: Duration) -> Self {
        Self {
            route_id,
            backends: RwLock::new(Vec::new()),
            rr_counter: AtomicUsize::new(0),
            connect_timeout,
            connections_attempted: AtomicU64::new(0),
            connections_succeeded: AtomicU64::new(0),
        }
    }

    /// Update the backend set from control plane state.
    ///
    /// This replaces the current backend set with the new set.
    /// Backends not in the new set are removed.
    /// New backends are added with Unknown health.
    pub async fn update_backends(&self, backends: Vec<Backend>) {
        let mut state = self.backends.write().await;

        // Build a map of existing backends for health preservation
        let existing: HashMap<Backend, BackendState> = state
            .drain(..)
            .map(|s| (s.backend.clone(), s))
            .collect();

        // Build new state, preserving health for existing backends
        *state = backends
            .into_iter()
            .map(|b| {
                if let Some(existing_state) = existing.get(&b) {
                    BackendState {
                        backend: b,
                        health: existing_state.health,
                        last_failure: existing_state.last_failure,
                        consecutive_failures: existing_state.consecutive_failures,
                    }
                } else {
                    BackendState {
                        backend: b,
                        health: HealthStatus::Unknown,
                        last_failure: None,
                        consecutive_failures: 0,
                    }
                }
            })
            .collect();

        debug!(
            route_id = %self.route_id,
            backend_count = state.len(),
            "Updated backend pool"
        );
    }

    /// Get the number of backends in the pool.
    pub async fn len(&self) -> usize {
        self.backends.read().await.len()
    }

    /// Check if the pool is empty.
    pub async fn is_empty(&self) -> bool {
        self.backends.read().await.is_empty()
    }

    /// Get the number of healthy backends.
    pub async fn healthy_count(&self) -> usize {
        self.backends
            .read()
            .await
            .iter()
            .filter(|s| s.health == HealthStatus::Healthy || s.health == HealthStatus::Unknown)
            .count()
    }

    /// Select a backend using round-robin and attempt connection.
    ///
    /// Returns the connected stream and the selected backend, or None if no
    /// backend is available or all connection attempts fail.
    pub async fn select_and_connect(&self) -> Option<(TcpStream, Backend)> {
        self.connections_attempted.fetch_add(1, Ordering::Relaxed);

        // Get eligible backends and their count
        let (eligible_count, start_idx) = {
            let backends = self.backends.read().await;
            let count = backends
                .iter()
                .filter(|s| s.health == HealthStatus::Healthy || s.health == HealthStatus::Unknown)
                .count();

            if count == 0 {
                warn!(route_id = %self.route_id, "No eligible backends");
                return None;
            }

            // Round-robin selection
            let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % count;
            (count, start)
        };

        // Try each eligible backend starting from round-robin position
        for i in 0..eligible_count {
            let try_idx = (start_idx + i) % eligible_count;

            let backend = {
                let backends = self.backends.read().await;
                let eligible: Vec<_> = backends
                    .iter()
                    .filter(|s| {
                        s.health == HealthStatus::Healthy || s.health == HealthStatus::Unknown
                    })
                    .collect();

                if try_idx >= eligible.len() {
                    continue;
                }
                eligible[try_idx].backend.clone()
            };

            match self.try_connect(&backend).await {
                Ok(stream) => {
                    self.mark_healthy(&backend).await;
                    self.connections_succeeded.fetch_add(1, Ordering::Relaxed);
                    return Some((stream, backend));
                }
                Err(e) => {
                    warn!(
                        route_id = %self.route_id,
                        backend_addr = %backend.socket_addr(),
                        error = %e,
                        "Backend connection failed"
                    );
                    self.mark_unhealthy(&backend).await;
                }
            }
        }

        None
    }

    /// Attempt to connect to a specific backend.
    async fn try_connect(&self, backend: &Backend) -> std::io::Result<TcpStream> {
        let addr = backend.socket_addr();
        debug!(
            route_id = %self.route_id,
            backend_addr = %addr,
            "Connecting to backend"
        );

        match timeout(self.connect_timeout, TcpStream::connect(addr)).await {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connect timeout",
            )),
        }
    }

    /// Mark a backend as healthy.
    async fn mark_healthy(&self, backend: &Backend) {
        let mut backends = self.backends.write().await;
        if let Some(state) = backends.iter_mut().find(|s| &s.backend == backend) {
            state.health = HealthStatus::Healthy;
            state.consecutive_failures = 0;
        }
    }

    /// Mark a backend as unhealthy.
    async fn mark_unhealthy(&self, backend: &Backend) {
        let mut backends = self.backends.write().await;
        if let Some(state) = backends.iter_mut().find(|s| &s.backend == backend) {
            state.health = HealthStatus::Unhealthy;
            state.last_failure = Some(Instant::now());
            state.consecutive_failures += 1;
        }
    }

    /// Get connection statistics.
    pub fn stats(&self) -> BackendPoolStats {
        BackendPoolStats {
            connections_attempted: self.connections_attempted.load(Ordering::Relaxed),
            connections_succeeded: self.connections_succeeded.load(Ordering::Relaxed),
        }
    }
}

/// Statistics for a backend pool.
#[derive(Debug, Clone)]
pub struct BackendPoolStats {
    pub connections_attempted: u64,
    pub connections_succeeded: u64,
}

/// Selector that manages backend pools for multiple routes.
pub struct BackendSelector {
    /// Backend pools keyed by route ID.
    pools: RwLock<HashMap<String, Arc<BackendPool>>>,
    /// Default connect timeout for new pools.
    connect_timeout: Duration,
}

impl BackendSelector {
    /// Create a new backend selector.
    pub fn new() -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Create a new backend selector with custom connect timeout.
    pub fn with_timeout(connect_timeout: Duration) -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
            connect_timeout,
        }
    }

    /// Get or create a backend pool for a route.
    pub async fn get_or_create_pool(&self, route_id: &str) -> Arc<BackendPool> {
        // Fast path: read lock
        {
            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(route_id) {
                return Arc::clone(pool);
            }
        }

        // Slow path: write lock
        let mut pools = self.pools.write().await;
        pools
            .entry(route_id.to_string())
            .or_insert_with(|| {
                Arc::new(BackendPool::with_timeout(
                    route_id.to_string(),
                    self.connect_timeout,
                ))
            })
            .clone()
    }

    /// Update backends for a specific route.
    pub async fn update_route_backends(&self, route_id: &str, backends: Vec<Backend>) {
        let pool = self.get_or_create_pool(route_id).await;
        pool.update_backends(backends).await;
    }

    /// Remove a route's backend pool.
    pub async fn remove_route(&self, route_id: &str) {
        let mut pools = self.pools.write().await;
        pools.remove(route_id);
    }

    /// Get a backend pool for a route (if it exists).
    pub async fn get_pool(&self, route_id: &str) -> Option<Arc<BackendPool>> {
        let pools = self.pools.read().await;
        pools.get(route_id).cloned()
    }

    /// Get all route IDs with active pools.
    pub async fn route_ids(&self) -> Vec<String> {
        let pools = self.pools.read().await;
        pools.keys().cloned().collect()
    }
}

impl Default for BackendSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_socket_addr() {
        let backend = Backend::new(
            "fd00::1".parse().unwrap(),
            8080,
            "inst-123".to_string(),
        );
        let addr = backend.socket_addr();
        assert_eq!(addr.to_string(), "[fd00::1]:8080");
    }

    #[tokio::test]
    async fn test_backend_pool_update() {
        let pool = BackendPool::new("route-1".to_string());

        let backends = vec![
            Backend::new("fd00::1".parse().unwrap(), 8080, "inst-1".to_string()),
            Backend::new("fd00::2".parse().unwrap(), 8080, "inst-2".to_string()),
        ];

        pool.update_backends(backends.clone()).await;
        assert_eq!(pool.len().await, 2);

        // Update with one removed
        pool.update_backends(vec![backends[0].clone()]).await;
        assert_eq!(pool.len().await, 1);
    }

    #[tokio::test]
    async fn test_backend_selector() {
        let selector = BackendSelector::new();

        let pool1 = selector.get_or_create_pool("route-1").await;
        let pool2 = selector.get_or_create_pool("route-1").await;

        // Should return the same pool
        assert!(Arc::ptr_eq(&pool1, &pool2));

        selector.remove_route("route-1").await;
        assert!(selector.get_pool("route-1").await.is_none());
    }
}
