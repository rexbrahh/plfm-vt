use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plfm_control_plane::{
    api,
    db::{Database, DbConfig},
    projections::{worker::WorkerConfig, ProjectionWorker},
    scheduler::SchedulerReconciler,
    state::AppState,
};
use plfm_id::NodeId;
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
async fn core_loop_request_id_idempotency_ryw_scale_and_instances() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,plfm_control_plane=debug,sqlx=warn".into()),
        )
        .with_test_writer()
        .try_init();

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
    let scheduler_pool = pool.clone();
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

    // Create an app in the org.
    let app_name = format!("itest-app-{}", unique_suffix());
    let create_app_url = format!("{base_url}/v1/orgs/{org_id}/apps");
    let idem_app = format!("itest-app-{}-key", unique_suffix());
    let resp_app = client
        .post(&create_app_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_app)
        .json(&serde_json::json!({
            "name": app_name,
            "description": "itest app"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp_app.status().is_success());
    let app_id = resp_app.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("missing app id")
        .to_string();

    // Create an env.
    let create_env_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs");
    let idem_env = format!("itest-env-{}-key", unique_suffix());
    let resp_env = client
        .post(&create_env_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_env)
        .json(&serde_json::json!({ "name": "prod" }))
        .send()
        .await
        .unwrap();
    assert!(resp_env.status().is_success());
    let env_id = resp_env.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("missing env id")
        .to_string();

    // Secrets: initially not configured.
    let secrets_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets");
    let resp_secrets_get = client
        .get(&secrets_url)
        .header("Authorization", auth_header)
        .send()
        .await
        .unwrap();
    assert_eq!(resp_secrets_get.status().as_u16(), 404);

    // Set secrets (and verify idempotency replay).
    let idem_secrets = format!("itest-secrets-{}-key", unique_suffix());
    let secrets_body = serde_json::json!({
        "values": { "DB_PASSWORD": "supersecret" }
    });

    let resp_secrets_put_1 = client
        .put(&secrets_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_secrets)
        .json(&secrets_body)
        .send()
        .await
        .unwrap();
    assert!(resp_secrets_put_1.status().is_success());
    let secrets_put_1: serde_json::Value = resp_secrets_put_1.json().await.unwrap();
    let bundle_id = secrets_put_1["bundle_id"].clone();
    let version_id = secrets_put_1["current_version_id"].clone();

    let resp_secrets_put_2 = client
        .put(&secrets_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_secrets)
        .json(&secrets_body)
        .send()
        .await
        .unwrap();
    assert!(resp_secrets_put_2.status().is_success());
    let secrets_put_2: serde_json::Value = resp_secrets_put_2.json().await.unwrap();
    assert_eq!(secrets_put_2["bundle_id"], bundle_id);
    assert_eq!(secrets_put_2["current_version_id"], version_id);

    let resp_secrets_get_2 = client
        .get(&secrets_url)
        .header("Authorization", auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_secrets_get_2.status().is_success());
    let secrets_get_2: serde_json::Value = resp_secrets_get_2.json().await.unwrap();
    assert_eq!(secrets_get_2["bundle_id"], bundle_id);
    assert_eq!(secrets_get_2["current_version_id"], version_id);

    // Create a release.
    let create_release_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/releases");
    let idem_release = format!("itest-release-{}-key", unique_suffix());
    let resp_release = client
        .post(&create_release_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_release)
        .json(&serde_json::json!({
            "image_ref": format!("example.com/{app_name}:demo"),
            "image_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "manifest_schema_version": 1,
            "manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp_release.status().is_success());
    let release_id = resp_release.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("missing release id")
        .to_string();

    // Create a deploy (deploys projection also sets desired releases + default scale).
    let create_deploy_url =
        format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys");
    let idem_deploy = format!("itest-deploy-{}-key", unique_suffix());
    let resp_deploy = client
        .post(&create_deploy_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_deploy)
        .json(&serde_json::json!({ "release_id": release_id }))
        .send()
        .await
        .unwrap();
    assert!(resp_deploy.status().is_success());

    // Insert a node directly (scheduler reads nodes_view).
    let node_id = NodeId::new().to_string();
    sqlx::query(
        r#"
        INSERT INTO nodes_view (
            node_id, state, wireguard_public_key, agent_mtls_subject, labels, allocatable,
            resource_version, created_at, updated_at
        )
        VALUES ($1, 'active', $2, $3, '{}'::jsonb, $4::jsonb, 1, now(), now())
        ON CONFLICT (node_id) DO NOTHING
        "#,
    )
    .bind(&node_id)
    .bind("itest-wireguard-key")
    .bind("CN=itest-node")
    .bind(serde_json::json!({ "cpu_cores": 4, "memory_bytes": 1073741824 }))
    .execute(&scheduler_pool)
    .await
    .unwrap();

    let instances_url =
        format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances?limit=200");

    async fn wait_for_instances(
        client: &reqwest::Client,
        instances_url: &str,
        auth_header: &str,
        expected: usize,
    ) -> serde_json::Value {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(10);

        loop {
            let resp = client
                .get(instances_url)
                .header("Authorization", auth_header)
                .send()
                .await
                .unwrap();
            assert!(resp.status().is_success());

            let request_id =
                header_str(resp.headers(), "x-request-id").expect("missing x-request-id");
            assert!(!request_id.is_empty());

            let body: serde_json::Value = resp.json().await.unwrap();
            let count = body["items"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(0);
            if count >= expected {
                return body;
            }

            if start.elapsed() > timeout {
                panic!("timed out waiting for {expected} instances; last body: {body}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Run scheduler once and wait for the first instance to materialize.
    let reconciler = SchedulerReconciler::new(scheduler_pool.clone());
    reconciler.reconcile_all().await.unwrap();
    let first = wait_for_instances(&client, &instances_url, auth_header, 1).await;
    assert_eq!(
        first["items"]
            .as_array()
            .expect("missing items array")
            .len(),
        1
    );

    // Scale up to 2 instances using optimistic concurrency (GET then PUT).
    let scale_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale");
    let resp_scale_get = client
        .get(&scale_url)
        .header("Authorization", auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_scale_get.status().is_success());
    let scale_state: serde_json::Value = resp_scale_get.json().await.unwrap();
    let current_version = scale_state["resource_version"].as_i64().unwrap_or(0) as i32;

    let idem_scale = format!("itest-scale-{}-key", unique_suffix());
    let resp_scale_put = client
        .put(&scale_url)
        .header("Authorization", auth_header)
        .header("Idempotency-Key", &idem_scale)
        .json(&serde_json::json!({
            "expected_version": current_version,
            "processes": [{ "process_type": "web", "desired": 2 }]
        }))
        .send()
        .await
        .unwrap();
    let scale_put_status = resp_scale_put.status();
    let scale_put_request_id =
        header_str(resp_scale_put.headers(), "x-request-id").unwrap_or_default();
    let scale_put_body = resp_scale_put.text().await.unwrap_or_default();
    if !scale_put_status.is_success() {
        panic!(
            "scale PUT failed: status={scale_put_status} request_id={scale_put_request_id} body={scale_put_body}"
        );
    }

    let updated_scale: serde_json::Value = serde_json::from_str(&scale_put_body).unwrap();
    assert_eq!(
        updated_scale["processes"]
            .as_array()
            .expect("missing processes array")
            .len(),
        1
    );

    // Reconcile again and wait for the second instance.
    reconciler.reconcile_all().await.unwrap();
    let second = wait_for_instances(&client, &instances_url, auth_header, 2).await;
    assert_eq!(
        second["items"]
            .as_array()
            .expect("missing items array")
            .len(),
        2
    );

    let _ = shutdown_tx.send(true);
    server_handle.abort();
    let _ = server_handle.await;
    projection_handle.abort();
    let _ = projection_handle.await;
}
