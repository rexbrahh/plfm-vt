//! Test harness for M4 ingress integration tests.
//!
//! Provides helpers to spawn TCP/TLS backends, ingress listeners, and verify
//! PROXY protocol v2 headers in a test environment.

use std::io;
use std::net::{SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

static INIT_CRYPTO: Once = Once::new();

fn init_crypto_provider() {
    INIT_CRYPTO.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
    });
}

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use plfm_ingress::{
    Backend, BackendSelector, Listener, ListenerConfig, ProtocolHint, ProxyProtocol, Route,
    RouteTable,
};

#[allow(dead_code)]
pub struct TcpEchoBackend {
    pub addr: SocketAddr,
    pub connections: Arc<AtomicU64>,
    pub bytes_received: Arc<AtomicU64>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TcpEchoBackend {
    pub async fn spawn_v6() -> io::Result<Self> {
        let listener = TcpListener::bind("[::1]:0").await?;
        let addr = listener.local_addr()?;
        let connections = Arc::new(AtomicU64::new(0));
        let bytes_received = Arc::new(AtomicU64::new(0));

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let conn_clone = Arc::clone(&connections);
        let bytes_clone = Arc::clone(&bytes_received);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((mut stream, _)) => {
                                conn_clone.fetch_add(1, Ordering::Relaxed);
                                let bytes = Arc::clone(&bytes_clone);
                                tokio::spawn(async move {
                                    let mut buf = vec![0u8; 8192];
                                    loop {
                                        match stream.read(&mut buf).await {
                                            Ok(0) => break,
                                            Ok(n) => {
                                                bytes.fetch_add(n as u64, Ordering::Relaxed);
                                                if stream.write_all(&buf[..n]).await.is_err() {
                                                    break;
                                                }
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        Ok(Self {
            addr,
            connections,
            bytes_received,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn connection_count(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for TcpEchoBackend {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[allow(dead_code)]
pub struct TlsBackend {
    pub addr: SocketAddr,
    pub cert_der: Vec<u8>,
    pub connections: Arc<AtomicU64>,
    pub marker: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TlsBackend {
    pub async fn spawn_v6(server_name: &str, marker: &str) -> io::Result<Self> {
        init_crypto_provider();

        let cert = rcgen::generate_simple_self_signed(vec![server_name.to_string()])
            .map_err(io::Error::other)?;

        let cert_der = cert.cert.der().to_vec();
        let key_der = cert.key_pair.serialize_der();

        let certs = vec![CertificateDer::from(cert_der.clone())];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(io::Error::other)?;

        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("[::1]:0").await?;
        let addr = listener.local_addr()?;

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let connections = Arc::new(AtomicU64::new(0));
        let conn_clone = Arc::clone(&connections);
        let marker_bytes = marker.as_bytes().to_vec();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, _)) => {
                                conn_clone.fetch_add(1, Ordering::Relaxed);
                                let acceptor = acceptor.clone();
                                let response = marker_bytes.clone();
                                tokio::spawn(async move {
                                    if let Ok(mut tls_stream) = acceptor.accept(stream).await {
                                        let mut buf = vec![0u8; 1024];
                                        if tls_stream.read(&mut buf).await.is_ok() {
                                            let _ = tls_stream.write_all(&response).await;
                                        }
                                    }
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        Ok(Self {
            addr,
            cert_der,
            connections,
            marker: marker.to_string(),
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn connection_count(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for TlsBackend {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[allow(dead_code)]
pub struct ProxyV2Backend {
    pub addr: SocketAddr,
    pub connections: Arc<AtomicU64>,
    pub last_proxy_header: Arc<tokio::sync::RwLock<Option<ParsedProxyHeader>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

#[derive(Debug, Clone)]
pub struct ParsedProxyHeader {
    pub src_addr: SocketAddr,
    pub dst_addr: SocketAddr,
    pub payload: Vec<u8>,
}

const PROXY_V2_SIGNATURE: [u8; 12] = [
    0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
];

impl ProxyV2Backend {
    pub async fn spawn_v6() -> io::Result<Self> {
        let listener = TcpListener::bind("[::1]:0").await?;
        let addr = listener.local_addr()?;
        let connections = Arc::new(AtomicU64::new(0));
        let last_proxy_header = Arc::new(tokio::sync::RwLock::new(None));

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let conn_clone = Arc::clone(&connections);
        let header_clone = Arc::clone(&last_proxy_header);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((mut stream, _)) => {
                                conn_clone.fetch_add(1, Ordering::Relaxed);
                                let header_store = Arc::clone(&header_clone);
                                tokio::spawn(async move {
                                    let mut header_base = [0u8; 16];
                                    if tokio::time::timeout(
                                        Duration::from_secs(1),
                                        stream.read_exact(&mut header_base),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        return;
                                    }

                                    if header_base[..12] != PROXY_V2_SIGNATURE {
                                        let _ = stream.write_all(b"ack").await;
                                        return;
                                    }

                                    let addr_len = u16::from_be_bytes([header_base[14], header_base[15]]) as usize;
                                    let mut addr_data = vec![0u8; addr_len];

                                    if addr_len > 0 {
                                        if tokio::time::timeout(
                                            Duration::from_secs(1),
                                            stream.read_exact(&mut addr_data),
                                        )
                                        .await
                                        .is_err()
                                        {
                                            return;
                                        }
                                    }

                                    let mut payload = vec![0u8; 256];
                                    let payload_len = match tokio::time::timeout(
                                        Duration::from_millis(100),
                                        stream.read(&mut payload),
                                    )
                                    .await
                                    {
                                        Ok(Ok(n)) => n,
                                        _ => 0,
                                    };

                                    let mut full_data = Vec::with_capacity(16 + addr_len + payload_len);
                                    full_data.extend_from_slice(&header_base);
                                    full_data.extend_from_slice(&addr_data);
                                    full_data.extend_from_slice(&payload[..payload_len]);

                                    if let Some(parsed) = parse_proxy_v2_header(&full_data) {
                                        *header_store.write().await = Some(parsed);
                                    }
                                    let _ = stream.write_all(b"ack").await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        Ok(Self {
            addr,
            connections,
            last_proxy_header,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub async fn get_last_header(&self) -> Option<ParsedProxyHeader> {
        self.last_proxy_header.read().await.clone()
    }
}

impl Drop for ProxyV2Backend {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn parse_proxy_v2_header(data: &[u8]) -> Option<ParsedProxyHeader> {
    if data.len() < 16 {
        return None;
    }

    if data[..12] != PROXY_V2_SIGNATURE {
        return None;
    }

    let family_protocol = data[13];
    let addr_len = u16::from_be_bytes([data[14], data[15]]) as usize;

    if data.len() < 16 + addr_len {
        return None;
    }

    let (src_addr, dst_addr) = if family_protocol == 0x21 {
        // AF_INET6 + STREAM
        if addr_len < 36 {
            return None;
        }
        let src_octets: [u8; 16] = data[16..32].try_into().ok()?;
        let dst_octets: [u8; 16] = data[32..48].try_into().ok()?;
        let src_port = u16::from_be_bytes([data[48], data[49]]);
        let dst_port = u16::from_be_bytes([data[50], data[51]]);
        (
            SocketAddr::V6(SocketAddrV6::new(src_octets.into(), src_port, 0, 0)),
            SocketAddr::V6(SocketAddrV6::new(dst_octets.into(), dst_port, 0, 0)),
        )
    } else if family_protocol == 0x11 {
        // AF_INET + STREAM
        if addr_len < 12 {
            return None;
        }
        let src_ip = std::net::Ipv4Addr::new(data[16], data[17], data[18], data[19]);
        let dst_ip = std::net::Ipv4Addr::new(data[20], data[21], data[22], data[23]);
        let src_port = u16::from_be_bytes([data[24], data[25]]);
        let dst_port = u16::from_be_bytes([data[26], data[27]]);
        (
            SocketAddr::new(src_ip.into(), src_port),
            SocketAddr::new(dst_ip.into(), dst_port),
        )
    } else {
        return None;
    };

    let header_len = 16 + addr_len;
    let payload = data[header_len..].to_vec();

    Some(ParsedProxyHeader {
        src_addr,
        dst_addr,
        payload,
    })
}

pub struct IngressHandle {
    pub listen_addr: SocketAddr,
    pub route_table: Arc<RouteTable>,
    pub backend_selector: Arc<BackendSelector>,
}

impl IngressHandle {
    pub async fn spawn_v6() -> io::Result<Self> {
        let route_table = Arc::new(RouteTable::new());
        let backend_selector = Arc::new(BackendSelector::new());

        let config = ListenerConfig::new("[::1]:0".parse().unwrap());
        let listener = Listener::bind(
            config,
            Arc::clone(&route_table),
            Arc::clone(&backend_selector),
        )
        .await?;

        let listen_addr = listener.local_addr()?;
        let listener = Arc::new(listener);

        tokio::spawn(async move {
            let _ = listener.run().await;
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        Ok(Self {
            listen_addr,
            route_table,
            backend_selector,
        })
    }

    pub async fn add_route(&self, route: Route) {
        self.route_table.upsert(route).await;
    }

    pub async fn add_backend(&self, route_id: &str, backend: Backend) {
        self.backend_selector
            .update_route_backends(route_id, vec![backend])
            .await;
    }
}

pub fn make_route(
    id: &str,
    hostname: &str,
    port: u16,
    protocol: ProtocolHint,
    backend_port: u16,
) -> Route {
    Route {
        id: id.to_string(),
        hostname: Route::normalize_hostname(hostname),
        port,
        protocol,
        proxy_protocol: ProxyProtocol::Off,
        app_id: "test-app".to_string(),
        env_id: "test-env".to_string(),
        backend_process_type: "web".to_string(),
        backend_port,
        allow_non_tls_fallback: false,
        env_ipv4_address: None,
    }
}

pub fn make_backend(addr: SocketAddr, instance_id: &str) -> Backend {
    match addr {
        SocketAddr::V6(v6) => Backend::new(*v6.ip(), v6.port(), instance_id.to_string()),
        SocketAddr::V4(_) => panic!("IPv6 address required for M4 tests"),
    }
}

pub async fn tls_client_connect(
    addr: SocketAddr,
    server_name: &str,
    cert_der: &[u8],
) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    init_crypto_provider();

    let mut root_store = rustls::RootCertStore::empty();
    root_store
        .add(CertificateDer::from(cert_der.to_vec()))
        .map_err(io::Error::other)?;

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let stream = TcpStream::connect(addr).await?;
    let server_name = ServerName::try_from(server_name.to_string())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    connector.connect(server_name, stream).await
}
