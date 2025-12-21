//! End-to-end happy path test.
//!
//! This test validates the complete user flow from authentication through
//! deployment, verifying:
//!
//! 1. Authentication (device flow)
//! 2. Create org, app, env
//! 3. Configure secrets (confirm none)
//! 4. Create release
//! 5. Create deploy
//! 6. Verify instances are scheduled
//! 7. Query events and logs
//!
//! ## Running
//!
//! ```bash
//! cargo test -p plfm-e2e --test happy_path
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use plfm_control_plane::{
    api,
    db::{Database, DbConfig},
    projections::{worker::WorkerConfig, ProjectionWorker},
    scheduler::SchedulerReconciler,
    state::AppState,
};
use plfm_id::NodeId;
use testcontainers::{core::IntoContainerPort, runners::AsyncRunner, GenericImage, ImageExt};
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
    let max_wait = Duration::from_secs(15);
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

/// Issue an access token via device flow (simulating browser approval).
async fn issue_device_token(
    client: &reqwest::Client,
    base_url: &str,
    db: &Database,
    email: &str,
) -> String {
    let resp = client
        .post(format!("{base_url}/v1/auth/device/start"))
        .json(&serde_json::json!({ "device_name": "e2e-test" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "device/start failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let device_code = body["device_code"].as_str().expect("missing device_code");
    let user_code = body["user_code"].as_str().expect("missing user_code");

    let subject_id = format!("usr_{}", unique_suffix());
    let scopes = serde_json::json!([
        "orgs:admin",
        "apps:write",
        "envs:write",
        "releases:write",
        "deploys:write",
        "routes:write",
        "volumes:write",
        "secrets:write",
        "logs:read",
        "events:read"
    ]);

    // Simulate browser approval by directly updating the device code.
    sqlx::query(
        r#"
        UPDATE device_codes
        SET status = 'approved',
            approved_subject_type = 'user',
            approved_subject_id = $1,
            approved_subject_email = $2,
            approved_scopes = $3,
            approved_at = now()
        WHERE user_code = $4
        "#,
    )
    .bind(&subject_id)
    .bind(email)
    .bind(scopes)
    .bind(user_code)
    .execute(db.pool())
    .await
    .unwrap();

    // Exchange device code for access token.
    let resp = client
        .post(format!("{base_url}/v1/auth/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "device/token failed");
    let tokens: serde_json::Value = resp.json().await.unwrap();
    tokens["access_token"]
        .as_str()
        .expect("missing access_token")
        .to_string()
}

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// E2E happy path test covering the complete user flow.
///
/// This test validates:
/// - Authentication via device flow
/// - Organization creation
/// - Application creation
/// - Environment creation
/// - Secrets confirmation (none)
/// - Release creation
/// - Deploy creation
/// - Instance scheduling
/// - Event querying
/// - Receipt headers (x-request-id)
#[tokio::test]
async fn e2e_happy_path_org_to_instances() {
    // Set required environment variables.
    std::env::set_var(
        "PLFM_SECRETS_MASTER_KEY",
        "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
    );
    std::env::set_var("PLFM_PROJECTION_WAIT_TIMEOUT_SECS", "15");

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,plfm_control_plane=debug,sqlx=warn".into()),
        )
        .with_test_writer()
        .try_init();

    // Start PostgreSQL via testcontainer.
    let postgres = GenericImage::new("postgres", "16-alpine")
        .with_exposed_port(5432.tcp())
        .with_env_var("POSTGRES_USER", "plfm")
        .with_env_var("POSTGRES_PASSWORD", "plfm_test")
        .with_env_var("POSTGRES_DB", "plfm")
        .start()
        .await
        .expect("failed to start postgres container");

    let port = postgres
        .get_host_port_ipv4(5432.tcp())
        .await
        .expect("failed to resolve postgres host port");
    let database_url = format!("postgres://plfm:plfm_test@127.0.0.1:{port}/plfm");
    wait_for_postgres(&database_url).await;

    let db_config = DbConfig {
        database_url,
        ..Default::default()
    };

    let db = Database::connect(&db_config).await.unwrap();
    db.run_migrations().await.unwrap();

    // Start projection worker for RYW semantics.
    let pool = db.pool().clone();
    let scheduler_pool = pool.clone();
    let projection_worker = ProjectionWorker::new(pool, WorkerConfig::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let projection_handle = tokio::spawn(async move {
        let _ = projection_worker.run(shutdown_rx).await;
    });

    // Start HTTP server.
    let state = AppState::new(db.clone());
    let app = api::create_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let access_token = issue_device_token(&client, &base_url, &db, "e2e@example.com").await;
    let auth_header = format!("Bearer {}", access_token);

    // ===========================================================================
    // Step 1: Create Organization
    // ===========================================================================
    let org_name = format!("e2e-org-{}", unique_suffix());
    let idem_org = format!("e2e-org-{}-key", unique_suffix());
    let resp = client
        .post(format!("{base_url}/v1/orgs"))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_org)
        .json(&serde_json::json!({ "name": org_name }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "org create failed");

    let request_id = header_str(resp.headers(), "x-request-id").expect("missing x-request-id");
    assert!(!request_id.is_empty(), "x-request-id should not be empty");

    let org: serde_json::Value = resp.json().await.unwrap();
    let org_id = org["id"].as_str().expect("missing org id").to_string();

    // ===========================================================================
    // Step 2: Create Application
    // ===========================================================================
    let app_name = format!("e2e-app-{}", unique_suffix());
    let idem_app = format!("e2e-app-{}-key", unique_suffix());
    let resp = client
        .post(format!("{base_url}/v1/orgs/{org_id}/apps"))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_app)
        .json(&serde_json::json!({
            "name": app_name,
            "description": "E2E test application"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "app create failed");
    let app: serde_json::Value = resp.json().await.unwrap();
    let app_id = app["id"].as_str().expect("missing app id").to_string();

    // ===========================================================================
    // Step 3: Create Environment
    // ===========================================================================
    let idem_env = format!("e2e-env-{}-key", unique_suffix());
    let resp = client
        .post(format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs"))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_env)
        .json(&serde_json::json!({ "name": "prod" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "env create failed");
    let env: serde_json::Value = resp.json().await.unwrap();
    let env_id = env["id"].as_str().expect("missing env id").to_string();

    // ===========================================================================
    // Step 4: Confirm secrets (none for this test)
    // ===========================================================================
    let secrets_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets");
    let idem_secrets = format!("e2e-secrets-{}-key", unique_suffix());
    let resp = client
        .put(&secrets_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_secrets)
        .json(&serde_json::json!({ "values": {} }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "secrets set failed");

    // ===========================================================================
    // Step 5: Create Release
    // ===========================================================================
    let idem_release = format!("e2e-release-{}-key", unique_suffix());
    let resp = client
        .post(format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/releases"))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_release)
        .json(&serde_json::json!({
            "image_ref": format!("example.com/{app_name}:latest"),
            "image_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "manifest_schema_version": 1,
            "manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "command": ["./start", "--port", "8080"]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "release create failed");
    let release: serde_json::Value = resp.json().await.unwrap();
    let release_id = release["id"]
        .as_str()
        .expect("missing release id")
        .to_string();

    // ===========================================================================
    // Step 6: Create Deploy
    // ===========================================================================
    let idem_deploy = format!("e2e-deploy-{}-key", unique_suffix());
    let resp = client
        .post(format!(
            "{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/deploys"
        ))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_deploy)
        .json(&serde_json::json!({ "release_id": release_id }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "deploy create failed");
    let deploy: serde_json::Value = resp.json().await.unwrap();
    let deploy_id = deploy["id"]
        .as_str()
        .expect("missing deploy id")
        .to_string();

    // ===========================================================================
    // Step 7: Register a node for scheduling
    // ===========================================================================
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
    .bind("e2e-wireguard-key")
    .bind("CN=e2e-node")
    .bind(serde_json::json!({ "cpu_cores": 4, "memory_bytes": 1073741824 }))
    .execute(&scheduler_pool)
    .await
    .unwrap();

    // Run scheduler reconciliation.
    let reconciler = SchedulerReconciler::new(scheduler_pool.clone());
    reconciler.reconcile_all().await.unwrap();

    // ===========================================================================
    // Step 8: Verify instances are created
    // ===========================================================================
    let instances_url =
        format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances?limit=200");

    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);
    let mut instance_count = 0;
    let mut first_instance_id: Option<String> = None;

    loop {
        let resp = client
            .get(&instances_url)
            .header("Authorization", &auth_header)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "instances list failed");

        let body: serde_json::Value = resp.json().await.unwrap();
        if let Some(items) = body["items"].as_array() {
            instance_count = items.len();
            if let Some(first) = items.first() {
                first_instance_id = first["id"].as_str().map(|s| s.to_string());
            }
        }

        if instance_count >= 1 {
            break;
        }

        if start.elapsed() > timeout {
            panic!("timed out waiting for instances; last count: {instance_count}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(instance_count >= 1, "expected at least 1 instance");
    let instance_id = first_instance_id.expect("missing instance id");

    // ===========================================================================
    // Step 9: Query events
    // ===========================================================================
    let events_url = format!("{base_url}/v1/orgs/{org_id}/events?limit=50");
    let resp = client
        .get(&events_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "events list failed");
    let events: serde_json::Value = resp.json().await.unwrap();
    let event_count = events["items"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    assert!(event_count > 0, "expected events to be recorded");

    let next_after = events["next_after_event_id"].as_i64().unwrap_or(0);
    let tail_url =
        format!("{base_url}/v1/orgs/{org_id}/events?after_event_id={next_after}&limit=50");
    let tail_resp = client
        .get(&tail_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(tail_resp.status().is_success(), "events tail failed");
    let tail_events: serde_json::Value = tail_resp.json().await.unwrap();
    let tail_count = tail_events["items"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    assert_eq!(tail_count, 0, "expected no new events on tail");

    let log_line = "hello from e2e";
    sqlx::query(
        r#"
        INSERT INTO workload_logs (
            org_id,
            app_id,
            env_id,
            process_type,
            instance_id,
            node_id,
            ts,
            stream,
            line,
            truncated
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, false)
        "#,
    )
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .bind("web")
    .bind(&instance_id)
    .bind(&node_id)
    .bind(Utc::now())
    .bind("stdout")
    .bind(log_line)
    .execute(db.pool())
    .await
    .unwrap();

    let logs_url =
        format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/logs?tail_lines=50");
    let logs_resp = client
        .get(&logs_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(logs_resp.status().is_success(), "logs query failed");
    let logs: serde_json::Value = logs_resp.json().await.unwrap();
    let logs_items = logs["items"].as_array().cloned().unwrap_or_default();
    assert!(
        logs_items
            .iter()
            .any(|item| item["line"].as_str() == Some(log_line)),
        "expected inserted log line in logs response"
    );

    // ===========================================================================
    // Verify key invariants
    // ===========================================================================

    // All responses should have x-request-id
    let final_resp = client
        .get(format!("{base_url}/v1/orgs/{org_id}"))
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(
        header_str(final_resp.headers(), "x-request-id").is_some(),
        "all responses should have x-request-id"
    );

    // Idempotency: replay org create should return same ID
    let replay_resp = client
        .post(format!("{base_url}/v1/orgs"))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_org)
        .json(&serde_json::json!({ "name": org_name }))
        .send()
        .await
        .unwrap();
    assert!(
        replay_resp.status().is_success(),
        "idempotent replay failed"
    );
    let replay_org: serde_json::Value = replay_resp.json().await.unwrap();
    assert_eq!(
        replay_org["id"].as_str(),
        Some(org_id.as_str()),
        "idempotent replay should return same ID"
    );

    // ===========================================================================
    // Cleanup
    // ===========================================================================
    let _ = shutdown_tx.send(true);
    server_handle.abort();
    let _ = server_handle.await;
    projection_handle.abort();
    let _ = projection_handle.await;

    println!("E2E happy path test completed successfully!");
    println!("  Org: {} ({})", org_name, org_id);
    println!("  App: {} ({})", app_name, app_id);
    println!("  Env: prod ({})", env_id);
    println!("  Release: {}", release_id);
    println!("  Deploy: {}", deploy_id);
    println!("  Instances: {}", instance_count);
    println!("  Events: {}", event_count);
}
