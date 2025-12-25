mod harness;

use std::time::Duration;

use harness::{
    make_backend, make_route, IngressHandle, ProxyV2Backend, TcpEchoBackend, TlsBackend,
};
use plfm_ingress::{ProtocolHint, ProxyProtocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn tls_passthrough_sni_routes_to_correct_backend_ipv6() {
    let backend_a = TlsBackend::spawn_v6("a.example.test", "A").await.unwrap();
    let backend_b = TlsBackend::spawn_v6("b.example.test", "B").await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let route_a = make_route(
        "r-a",
        "a.example.test",
        port,
        ProtocolHint::TlsPassthrough,
        backend_a.addr.port(),
    );
    let route_b = make_route(
        "r-b",
        "b.example.test",
        port,
        ProtocolHint::TlsPassthrough,
        backend_b.addr.port(),
    );

    ingress.add_route(route_a).await;
    ingress.add_route(route_b).await;
    ingress
        .add_backend("r-a", make_backend(backend_a.addr, "inst-a"))
        .await;
    ingress
        .add_backend("r-b", make_backend(backend_b.addr, "inst-b"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result_a = timeout(TEST_TIMEOUT, async {
        let mut stream =
            harness::tls_client_connect(ingress.listen_addr, "a.example.test", &backend_a.cert_der)
                .await?;

        stream.write_all(b"whoami").await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 16];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf[..n]).to_string())
    })
    .await;

    match result_a {
        Ok(Ok(response)) => assert_eq!(response, "A", "Expected response from backend A"),
        Ok(Err(e)) => panic!("TLS connection to A failed: {}", e),
        Err(_) => panic!("TLS connection to A timed out"),
    }

    let result_b = timeout(TEST_TIMEOUT, async {
        let mut stream =
            harness::tls_client_connect(ingress.listen_addr, "b.example.test", &backend_b.cert_der)
                .await?;

        stream.write_all(b"whoami").await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 16];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf[..n]).to_string())
    })
    .await;

    match result_b {
        Ok(Ok(response)) => assert_eq!(response, "B", "Expected response from backend B"),
        Ok(Err(e)) => panic!("TLS connection to B failed: {}", e),
        Err(_) => panic!("TLS connection to B timed out"),
    }

    assert_eq!(backend_a.connection_count(), 1);
    assert_eq!(backend_b.connection_count(), 1);
}

#[tokio::test]
async fn no_sni_rejected_when_ambiguous_ipv6() {
    let backend_a = TcpEchoBackend::spawn_v6().await.unwrap();
    let backend_b = TcpEchoBackend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let route_a = make_route(
        "r-a",
        "a.example.test",
        port,
        ProtocolHint::TlsPassthrough,
        backend_a.addr.port(),
    );
    let route_b = make_route(
        "r-b",
        "b.example.test",
        port,
        ProtocolHint::TlsPassthrough,
        backend_b.addr.port(),
    );

    ingress.add_route(route_a).await;
    ingress.add_route(route_b).await;
    ingress
        .add_backend("r-a", make_backend(backend_a.addr, "inst-a"))
        .await;
    ingress
        .add_backend("r-b", make_backend(backend_b.addr, "inst-b"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result = timeout(Duration::from_millis(500), async {
        let mut stream = TcpStream::connect(ingress.listen_addr).await?;
        stream.write_all(b"not-tls-data").await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 64];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(n)
    })
    .await;

    match result {
        Ok(Ok(0)) | Err(_) => {}
        Ok(Ok(n)) => panic!("Expected connection close, got {} bytes", n),
        Ok(Err(_)) => {}
    }

    assert_eq!(
        backend_a.connection_count(),
        0,
        "Backend A should not receive connection"
    );
    assert_eq!(
        backend_b.connection_count(),
        0,
        "Backend B should not receive connection"
    );
}

#[tokio::test]
async fn no_sni_accepted_when_unambiguous_ipv6() {
    let backend = TcpEchoBackend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-single",
        "single.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend.addr.port(),
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend("r-single", make_backend(backend.addr, "inst-single"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result = timeout(TEST_TIMEOUT, async {
        let mut stream = TcpStream::connect(ingress.listen_addr).await?;
        stream.write_all(b"hello").await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 64];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(buf[..n].to_vec())
    })
    .await;

    match result {
        Ok(Ok(data)) => assert_eq!(data, b"hello", "Backend should echo data"),
        Ok(Err(e)) => panic!("Connection failed: {}", e),
        Err(_) => panic!("Connection timed out"),
    }

    assert_eq!(backend.connection_count(), 1);
}

#[tokio::test]
async fn raw_tcp_route_proxies_bytes_unchanged_ipv6() {
    let backend = TcpEchoBackend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-raw",
        "raw.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend.addr.port(),
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend("r-raw", make_backend(backend.addr, "inst-raw"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let test_payload: Vec<u8> = (0..256).map(|i| i as u8).collect();

    let result = timeout(TEST_TIMEOUT, async {
        let mut stream = TcpStream::connect(ingress.listen_addr).await?;
        stream.write_all(&test_payload).await?;
        stream.flush().await?;

        let mut received = Vec::new();
        let mut buf = vec![0u8; 1024];

        loop {
            match timeout(Duration::from_millis(100), stream.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => received.extend_from_slice(&buf[..n]),
                Ok(Err(e)) => return Err(e),
                Err(_) => break,
            }
            if received.len() >= test_payload.len() {
                break;
            }
        }

        Ok::<_, std::io::Error>(received)
    })
    .await;

    match result {
        Ok(Ok(data)) => {
            assert_eq!(data.len(), test_payload.len(), "Payload length mismatch");
            assert_eq!(data, test_payload, "Payload content mismatch");
        }
        Ok(Err(e)) => panic!("Connection failed: {}", e),
        Err(_) => panic!("Connection timed out"),
    }
}

#[tokio::test]
async fn proxy_v2_injected_and_contains_correct_client_addr_ipv6() {
    let backend = ProxyV2Backend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-proxy",
        "proxy.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend.addr.port(),
    );
    route.proxy_protocol = ProxyProtocol::V2;
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend("r-proxy", make_backend(backend.addr, "inst-proxy"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result = timeout(TEST_TIMEOUT, async {
        let mut stream = TcpStream::connect(ingress.listen_addr).await?;
        let client_addr = stream.local_addr()?;

        stream.write_all(b"ping").await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 64];
        let _ = stream.read(&mut buf).await?;

        Ok::<_, std::io::Error>(client_addr)
    })
    .await;

    let client_addr = match result {
        Ok(Ok(addr)) => addr,
        Ok(Err(e)) => panic!("Connection failed: {}", e),
        Err(_) => panic!("Connection timed out"),
    };

    tokio::time::sleep(Duration::from_millis(50)).await;

    let header = backend.get_last_header().await;
    let header = header.expect("Backend should have received PROXY v2 header");

    assert!(header.src_addr.is_ipv6(), "Source address should be IPv6");
    assert_eq!(
        header.src_addr.port(),
        client_addr.port(),
        "Client port should match"
    );
    assert_eq!(
        header.dst_addr.port(),
        port,
        "Destination port should match ingress listener"
    );
    assert_eq!(header.payload, b"ping", "Payload should follow header");
}

#[tokio::test]
async fn health_gating_excludes_not_ready_instances_ipv6() {
    let backend_a = TcpEchoBackend::spawn_v6().await.unwrap();
    let backend_b = TcpEchoBackend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-health",
        "health.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend_a.addr.port(),
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend("r-health", make_backend(backend_a.addr, "inst-a"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    for _ in 0..3 {
        let _ = timeout(Duration::from_millis(500), async {
            let mut stream = TcpStream::connect(ingress.listen_addr).await?;
            stream.write_all(b"test").await?;
            stream.flush().await?;
            let mut buf = vec![0u8; 64];
            let _ = stream.read(&mut buf).await?;
            Ok::<_, std::io::Error>(())
        })
        .await;
    }

    let a_count_before = backend_a.connection_count();
    let b_count_before = backend_b.connection_count();

    assert!(
        a_count_before >= 1,
        "Backend A should have received connections"
    );
    assert_eq!(
        b_count_before, 0,
        "Backend B should not have received connections yet"
    );

    let mut updated_route = make_route(
        "r-health",
        "health.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend_b.addr.port(),
    );
    updated_route.allow_non_tls_fallback = true;

    ingress.add_route(updated_route).await;
    ingress
        .backend_selector
        .update_route_backends("r-health", vec![make_backend(backend_b.addr, "inst-b")])
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    for _ in 0..3 {
        let _ = timeout(Duration::from_millis(500), async {
            let mut stream = TcpStream::connect(ingress.listen_addr).await?;
            stream.write_all(b"test").await?;
            stream.flush().await?;
            let mut buf = vec![0u8; 64];
            let _ = stream.read(&mut buf).await?;
            Ok::<_, std::io::Error>(())
        })
        .await;
    }

    let a_count_after = backend_a.connection_count();
    let b_count_after = backend_b.connection_count();

    assert_eq!(
        a_count_after, a_count_before,
        "Backend A should not receive new connections"
    );
    assert!(
        b_count_after >= 1,
        "Backend B should now receive connections"
    );
}
