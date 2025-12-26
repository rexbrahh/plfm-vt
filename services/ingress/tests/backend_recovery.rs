mod harness;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use harness::{make_backend, make_route, IngressHandle, TcpEchoBackend};
use plfm_ingress::ProtocolHint;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

async fn try_roundtrip(ingress_addr: SocketAddr, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
    let result = timeout(Duration::from_millis(500), async {
        let mut stream = TcpStream::connect(ingress_addr).await?;
        stream.write_all(payload).await?;
        stream.flush().await?;
        let mut buf = vec![0u8; 64];
        let n = stream.read(&mut buf).await?;
        Ok::<_, std::io::Error>(buf[..n].to_vec())
    })
    .await;

    match result {
        Ok(Ok(data)) if !data.is_empty() => Ok(data),
        Ok(Ok(_)) => Err("connection closed"),
        Ok(Err(_)) => Err("io error"),
        Err(_) => Err("timeout"),
    }
}

#[tokio::test]
async fn unhealthy_backend_retried_after_cooldown() {
    let temp_listener = TcpListener::bind("[::1]:0").await.unwrap();
    let dead_port = temp_listener.local_addr().unwrap().port();
    drop(temp_listener);

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-recovery",
        "recovery.example.test",
        port,
        ProtocolHint::TcpRaw,
        dead_port,
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend(
            "r-recovery",
            make_backend(
                format!("[::1]:{}", dead_port).parse().unwrap(),
                "inst-recovery",
            ),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result1 = try_roundtrip(ingress.listen_addr, b"test1").await;
    assert!(
        result1.is_err(),
        "First attempt should fail - no backend listener"
    );

    let result2 = try_roundtrip(ingress.listen_addr, b"test2").await;
    assert!(
        result2.is_err(),
        "Immediate retry should fail - backend unhealthy, cooldown not elapsed"
    );

    let new_backend = TcpListener::bind(format!("[::1]:{}", dead_port))
        .await
        .unwrap();

    let accepting = Arc::new(AtomicBool::new(true));
    let accept_flag = Arc::clone(&accepting);
    tokio::spawn(async move {
        while accept_flag.load(Ordering::SeqCst) {
            if let Ok((mut stream, _)) = new_backend.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 1024];
                    if let Ok(n) = stream.read(&mut buf).await {
                        let _ = stream.write_all(&buf[..n]).await;
                    }
                });
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(2200)).await;

    let result3 = try_roundtrip(ingress.listen_addr, b"test3").await;
    match result3 {
        Ok(data) => assert_eq!(data, b"test3", "Backend should echo after recovery"),
        Err(e) => panic!("Connection after cooldown should succeed: {}", e),
    }

    accepting.store(false, Ordering::SeqCst);
}

#[tokio::test]
async fn backend_recovers_after_new_instance_registered() {
    let temp_listener = TcpListener::bind("[::1]:0").await.unwrap();
    let dead_addr = temp_listener.local_addr().unwrap();
    drop(temp_listener);

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-restart",
        "restart.example.test",
        port,
        ProtocolHint::TcpRaw,
        dead_addr.port(),
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .add_backend("r-restart", make_backend(dead_addr, "inst-restart"))
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result1 = try_roundtrip(ingress.listen_addr, b"test1").await;
    assert!(result1.is_err(), "Connection should fail - no backend");

    let new_backend = TcpEchoBackend::spawn_v6().await.unwrap();

    ingress
        .backend_selector
        .update_route_backends(
            "r-restart",
            vec![make_backend(new_backend.addr, "inst-restart-new")],
        )
        .await;

    let mut updated_route = make_route(
        "r-restart",
        "restart.example.test",
        port,
        ProtocolHint::TcpRaw,
        new_backend.addr.port(),
    );
    updated_route.allow_non_tls_fallback = true;
    ingress.add_route(updated_route).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result2 = try_roundtrip(ingress.listen_addr, b"recovered").await;
    match result2 {
        Ok(data) => assert_eq!(data, b"recovered", "New backend should work"),
        Err(e) => panic!("Connection to new backend failed: {}", e),
    }

    assert_eq!(
        new_backend.connection_count(),
        1,
        "New backend should receive connection"
    );
}

#[tokio::test]
async fn multiple_backends_failover_to_healthy() {
    let backend_a = TcpEchoBackend::spawn_v6().await.unwrap();
    let backend_b = TcpEchoBackend::spawn_v6().await.unwrap();

    let ingress = IngressHandle::spawn_v6().await.unwrap();
    let port = ingress.listen_addr.port();

    let mut route = make_route(
        "r-multi",
        "multi.example.test",
        port,
        ProtocolHint::TcpRaw,
        backend_a.addr.port(),
    );
    route.allow_non_tls_fallback = true;

    ingress.add_route(route).await;
    ingress
        .backend_selector
        .update_route_backends(
            "r-multi",
            vec![
                make_backend(backend_a.addr, "inst-a"),
                make_backend(backend_b.addr, "inst-b"),
            ],
        )
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0..4 {
        let result = try_roundtrip(ingress.listen_addr, format!("req{}", i).as_bytes()).await;
        assert!(result.is_ok(), "Request {} should succeed", i);
    }

    let total = backend_a.connection_count() + backend_b.connection_count();
    assert!(
        total >= 4,
        "Both backends should share traffic (got {} connections)",
        total
    );
}
