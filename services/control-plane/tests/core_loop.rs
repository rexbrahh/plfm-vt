use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plfm_control_plane::{
    api,
    db::{Database, DbConfig},
    projections::{worker::WorkerConfig, ProjectionWorker},
    state::AppState,
};
use testcontainers::{clients, GenericImage};
use tokio::net::TcpListener;
use tokio::sync::watch;

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos()
        .to_string()
}

async fn wait_for_postgres(database_url: &str) {
    let max_wait = Duration::from_secs(10);
    let start = std::time::Instant::now();

    loop {
        match sqlx::PgPool::connect(database_url).await {
            Ok(pool) => {
                let _ = pool.close().await;
                return;
            }
            Err(_) => {
                if start.elapsed() > max_wait {
                    panic!("postgres did not become ready within {max_wait:?}: {database_url}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[tokio::test]
async fn core_loop_request_id_idempotency_and_ryw() {
    let docker = clients::Cli::default();
    let postgres = docker.run(
        GenericImage::new("postgres", "16-alpine")
            .with_env_var("POSTGRES_USER", "plfm")
            .with_env_var("POSTGRES_PASSWORD", "plfm_test")
            .with_env_var("POSTGRES_DB", "plfm")
            .with_exposed_port(5432),
    );

    let port = postgres.get_host_port_ipv4(5432);
    let database_url = format!("postgres://plfm:plfm_test@127.0.0.1:{port}/plfm");
    wait_for_postgres(&database_url).await;

    let db_config = DbConfig {
        database_url,
        ..Default::default()
    };

    let db = Database::connect(&db_config).await.unwrap();
    db.run_migrations().await.unwrap();

    // The HTTP handlers rely on projections being applied to satisfy RYW semantics.
    let pool = db.pool().clone();
    let projection_worker = ProjectionWorker::new(pool, WorkerConfig::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let projection_handle = tokio::spawn(async move {
        let _ = projection_worker.run(shutdown_rx).await;
    });

    let state = AppState::new(db);
    let app = api::create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let auth_header = "Bearer user:itest@example.com";
    let client = reqwest::Client::new();

    let idem_key = format!("itest-org-{}-key", unique_suffix());
    let org_name = format!("itest-org-{}", unique_suffix());
    let create_url = format!("{base_url}/v1/orgs");

    let resp1 = client
        .post(&create_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_key)
        .json(&serde_json::json!({ "name": org_name }))
        .send()
        .await
        .unwrap();
    assert!(resp1.status().is_success());

    let request_id_1 = header_str(resp1.headers(), "x-request-id").expect("missing x-request-id");
    assert!(!request_id_1.is_empty());

    let body1: serde_json::Value = resp1.json().await.unwrap();
    let org_id = body1["id"].as_str().expect("missing org id").to_string();

    let resp2 = client
        .post(&create_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_key)
        .json(&serde_json::json!({ "name": org_name }))
        .send()
        .await
        .unwrap();
    assert!(resp2.status().is_success());

    let request_id_2 = header_str(resp2.headers(), "x-request-id").expect("missing x-request-id");
    assert!(!request_id_2.is_empty());

    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body2["id"], body1["id"]);

    // RYW proof: the create endpoint waits for projections; GET immediately must succeed.
    let get_url = format!("{base_url}/v1/orgs/{org_id}");
    let resp_get = client
        .get(&get_url)
        .header("Authorization", auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_get.status().is_success());

    let request_id_get =
        header_str(resp_get.headers(), "x-request-id").expect("missing x-request-id");
    assert!(!request_id_get.is_empty());

    let body_get: serde_json::Value = resp_get.json().await.unwrap();
    assert_eq!(body_get["id"], body1["id"]);

    let _ = shutdown_tx.send(true);
    server_handle.abort();
    let _ = server_handle.await;
    projection_handle.abort();
    let _ = projection_handle.await;
}
