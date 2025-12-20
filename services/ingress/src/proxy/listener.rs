//! TCP listener and connection handling.
//!
//! This module manages TCP listeners, accepts connections, performs
//! SNI inspection, routing, and proxies connections to backends.
//!
//! Per spec (docs/specs/networking/ingress-l4.md):
//! - TCP proxying at Layer 4
//! - SNI inspection for TLS passthrough routes
//! - PROXY v2 header injection when enabled
//! - Connection-level routing (not request-level)
//!
//! Reference: docs/specs/networking/ingress-l4.md

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn, Instrument};

use super::backend::BackendSelector;
use super::proxy_protocol::ProxyProtocolV2;
use super::router::{ProtocolHint, ProxyProtocol, RouteTable, RoutingDecision};
use super::sni::{SniConfig, SniInspector, SniResult};

/// Default maximum concurrent connections per listener.
pub const DEFAULT_MAX_CONNECTIONS: usize = 10000;

/// Default idle timeout (none for raw TCP per spec).
pub const DEFAULT_IDLE_TIMEOUT: Option<Duration> = None;

/// Configuration for a listener.
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Address to bind to.
    pub bind_addr: SocketAddr,
    /// Maximum concurrent connections.
    pub max_connections: usize,
    /// SNI inspection configuration.
    pub sni_config: SniConfig,
    /// Idle timeout for connections.
    pub idle_timeout: Option<Duration>,
}

impl ListenerConfig {
    /// Create a new listener configuration.
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            sni_config: SniConfig::default(),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

/// Statistics for a listener.
#[derive(Debug, Default)]
pub struct ListenerStats {
    /// Total connections accepted.
    pub connections_accepted: AtomicU64,
    /// Total connections currently active.
    pub connections_active: AtomicU64,
    /// Total connections closed.
    pub connections_closed: AtomicU64,
    /// Connections rejected due to max limit.
    pub connections_rejected: AtomicU64,
    /// SNI extraction successes.
    pub sni_found: AtomicU64,
    /// SNI extraction failures (timeout, not TLS, etc.).
    pub sni_failed: AtomicU64,
    /// Routing successes.
    pub routes_matched: AtomicU64,
    /// Routing failures (no match, ambiguous).
    pub routes_failed: AtomicU64,
    /// Backend connection successes.
    pub backend_connected: AtomicU64,
    /// Backend connection failures.
    pub backend_failed: AtomicU64,
    /// Bytes proxied to backend.
    pub bytes_to_backend: AtomicU64,
    /// Bytes proxied from backend.
    pub bytes_from_backend: AtomicU64,
}

/// A TCP listener for the L4 proxy.
pub struct Listener {
    /// Listener configuration.
    config: ListenerConfig,
    /// The TCP listener.
    listener: TcpListener,
    /// Route table for routing decisions.
    route_table: Arc<RouteTable>,
    /// Backend selector for connection pooling.
    backend_selector: Arc<BackendSelector>,
    /// Connection semaphore for limiting concurrent connections.
    conn_semaphore: Arc<Semaphore>,
    /// SNI inspector.
    sni_inspector: SniInspector,
    /// Statistics.
    stats: Arc<ListenerStats>,
}

impl Listener {
    /// Create a new listener.
    pub async fn bind(
        config: ListenerConfig,
        route_table: Arc<RouteTable>,
        backend_selector: Arc<BackendSelector>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(config.bind_addr).await?;
        let local_addr = listener.local_addr()?;

        info!(
            bind_addr = %local_addr,
            max_connections = config.max_connections,
            "Listener bound"
        );

        Ok(Self {
            conn_semaphore: Arc::new(Semaphore::new(config.max_connections)),
            sni_inspector: SniInspector::with_config(config.sni_config.clone()),
            listener,
            config,
            route_table,
            backend_selector,
            stats: Arc::new(ListenerStats::default()),
        })
    }

    /// Get the local address this listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Get listener statistics.
    pub fn stats(&self) -> &ListenerStats {
        &self.stats
    }

    /// Run the listener, accepting and handling connections.
    pub async fn run(self: Arc<Self>) -> io::Result<()> {
        let local_addr = self.listener.local_addr()?;
        info!(bind_addr = %local_addr, "Listener started");

        loop {
            match self.listener.accept().await {
                Ok((stream, peer_addr)) => {
                    // Try to acquire a permit
                    let permit = match self.conn_semaphore.clone().try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(_) => {
                            self.stats
                                .connections_rejected
                                .fetch_add(1, Ordering::Relaxed);
                            warn!(peer_addr = %peer_addr, "Connection rejected: max connections reached");
                            continue;
                        }
                    };

                    self.stats
                        .connections_accepted
                        .fetch_add(1, Ordering::Relaxed);
                    self.stats
                        .connections_active
                        .fetch_add(1, Ordering::Relaxed);

                    let listener = Arc::clone(&self);
                    let stats = Arc::clone(&self.stats);

                    tokio::spawn(
                        async move {
                            if let Err(e) = listener.handle_connection(stream, peer_addr).await {
                                debug!(
                                    peer_addr = %peer_addr,
                                    error = %e,
                                    "Connection error"
                                );
                            }

                            stats.connections_active.fetch_sub(1, Ordering::Relaxed);
                            stats.connections_closed.fetch_add(1, Ordering::Relaxed);
                            drop(permit);
                        }
                        .instrument(tracing::info_span!("connection", peer = %peer_addr)),
                    );
                }
                Err(e) => {
                    error!(error = %e, "Accept error");
                    // Brief sleep to avoid tight loop on persistent errors
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Handle a single connection.
    async fn handle_connection(
        &self,
        mut client: TcpStream,
        peer_addr: SocketAddr,
    ) -> io::Result<()> {
        let local_addr = client.local_addr()?;
        debug!(peer_addr = %peer_addr, local_addr = %local_addr, "Handling connection");

        // Determine if we need SNI inspection based on routes for this port
        let routes = self.route_table.routes_for_port(local_addr.port()).await;
        let needs_sni = routes
            .iter()
            .any(|r| r.protocol == ProtocolHint::TlsPassthrough);

        // Buffer for SNI inspection (will be forwarded to backend)
        let mut sniff_buffer = Vec::new();
        let sni: Option<String>;

        if needs_sni {
            let (result, _bytes_read) = self
                .sni_inspector
                .inspect(&mut client, &mut sniff_buffer)
                .await;

            match &result {
                SniResult::Found(hostname) => {
                    self.stats.sni_found.fetch_add(1, Ordering::Relaxed);
                    debug!(hostname = %hostname, "SNI extracted");
                    sni = Some(hostname.clone());
                }
                SniResult::NoSni => {
                    self.stats.sni_failed.fetch_add(1, Ordering::Relaxed);
                    debug!("No SNI in ClientHello");
                    sni = None;
                }
                SniResult::NotTls => {
                    self.stats.sni_failed.fetch_add(1, Ordering::Relaxed);
                    debug!("Not a TLS connection");
                    sni = None;
                }
                SniResult::Timeout => {
                    self.stats.sni_failed.fetch_add(1, Ordering::Relaxed);
                    warn!("SNI inspection timeout");
                    sni = None;
                }
                SniResult::IoError(e) => {
                    self.stats.sni_failed.fetch_add(1, Ordering::Relaxed);
                    return Err(io::Error::other(e.clone()));
                }
                SniResult::Malformed => {
                    self.stats.sni_failed.fetch_add(1, Ordering::Relaxed);
                    debug!("Malformed TLS ClientHello");
                    sni = None;
                }
            }
        } else {
            sni = None;
        }

        // Make routing decision
        let decision = self.route_table.route(local_addr, sni.as_deref()).await;

        let route = match decision {
            RoutingDecision::Matched { route } => {
                self.stats.routes_matched.fetch_add(1, Ordering::Relaxed);
                route
            }
            RoutingDecision::NoMatch { reason } => {
                self.stats.routes_failed.fetch_add(1, Ordering::Relaxed);
                debug!(reason = %reason, "No route match");
                return Ok(());
            }
            RoutingDecision::Ambiguous { reason } => {
                self.stats.routes_failed.fetch_add(1, Ordering::Relaxed);
                warn!(reason = %reason, "Ambiguous routing");
                return Ok(());
            }
        };

        debug!(
            route_id = %route.id,
            env_id = %route.env_id,
            backend_process_type = %route.backend_process_type,
            "Route matched"
        );

        // Get backend pool and connect
        let pool = self.backend_selector.get_or_create_pool(&route.id).await;

        let (mut backend, backend_info) = match pool.select_and_connect().await {
            Some((stream, backend)) => {
                self.stats.backend_connected.fetch_add(1, Ordering::Relaxed);
                (stream, backend)
            }
            None => {
                self.stats.backend_failed.fetch_add(1, Ordering::Relaxed);
                warn!(route_id = %route.id, "No available backends");
                return Ok(());
            }
        };

        debug!(
            backend_addr = %backend_info.socket_addr(),
            instance_id = %backend_info.instance_id,
            "Connected to backend"
        );

        // Send PROXY v2 header if enabled
        if route.proxy_protocol == ProxyProtocol::V2 {
            let proxy_header = ProxyProtocolV2::new(peer_addr, local_addr);
            let header_bytes = proxy_header.encode()?;
            backend.write_all(&header_bytes).await?;
            debug!("PROXY v2 header sent");
        }

        // Forward any buffered data from SNI inspection
        if !sniff_buffer.is_empty() {
            backend.write_all(&sniff_buffer).await?;
        }

        // Proxy the connection bidirectionally
        let (bytes_to_backend, bytes_from_backend) =
            proxy_bidirectional(&mut client, &mut backend, self.config.idle_timeout).await?;

        self.stats
            .bytes_to_backend
            .fetch_add(bytes_to_backend, Ordering::Relaxed);
        self.stats
            .bytes_from_backend
            .fetch_add(bytes_from_backend, Ordering::Relaxed);

        debug!(
            bytes_to_backend = bytes_to_backend,
            bytes_from_backend = bytes_from_backend,
            "Connection closed"
        );

        Ok(())
    }
}

/// Proxy data bidirectionally between two streams.
///
/// Returns (bytes_to_b, bytes_from_b).
async fn proxy_bidirectional(
    a: &mut TcpStream,
    b: &mut TcpStream,
    idle_timeout: Option<Duration>,
) -> io::Result<(u64, u64)> {
    let (mut a_read, mut a_write) = a.split();
    let (mut b_read, mut b_write) = b.split();

    let a_to_b = async {
        let mut total = 0u64;
        let mut buf = vec![0u8; 8192];
        loop {
            let read_result = if let Some(timeout) = idle_timeout {
                match tokio::time::timeout(timeout, a_read.read(&mut buf)).await {
                    Ok(result) => result,
                    Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout")),
                }
            } else {
                a_read.read(&mut buf).await
            };

            match read_result {
                Ok(0) => break,
                Ok(n) => {
                    b_write.write_all(&buf[..n]).await?;
                    total += n as u64;
                }
                Err(e) => return Err(e),
            }
        }
        b_write.shutdown().await?;
        Ok(total)
    };

    let b_to_a = async {
        let mut total = 0u64;
        let mut buf = vec![0u8; 8192];
        loop {
            let read_result = if let Some(timeout) = idle_timeout {
                match tokio::time::timeout(timeout, b_read.read(&mut buf)).await {
                    Ok(result) => result,
                    Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "idle timeout")),
                }
            } else {
                b_read.read(&mut buf).await
            };

            match read_result {
                Ok(0) => break,
                Ok(n) => {
                    a_write.write_all(&buf[..n]).await?;
                    total += n as u64;
                }
                Err(e) => return Err(e),
            }
        }
        a_write.shutdown().await?;
        Ok(total)
    };

    let (a_result, b_result) = tokio::join!(a_to_b, b_to_a);

    // Return bytes transferred even if one direction errored
    let bytes_to_b = a_result.unwrap_or(0);
    let bytes_from_b = b_result.unwrap_or(0);

    Ok((bytes_to_b, bytes_from_b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listener_config_default() {
        let config = ListenerConfig::new("[::]:443".parse().unwrap());
        assert_eq!(config.max_connections, DEFAULT_MAX_CONNECTIONS);
        assert!(config.idle_timeout.is_none());
    }

    #[tokio::test]
    async fn test_listener_stats() {
        let stats = ListenerStats::default();
        stats.connections_accepted.fetch_add(1, Ordering::Relaxed);
        assert_eq!(stats.connections_accepted.load(Ordering::Relaxed), 1);
    }
}
