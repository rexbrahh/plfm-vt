//! Node API integration tests.
//!
//! Tests node enrollment, heartbeat, and plan delivery endpoints
//! that are used by node-agents.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plfm_control_plane::{
    api,
    db::{Database, DbConfig},
    projections::{worker::WorkerConfig, ProjectionWorker},
    scheduler::SchedulerReconciler,
    state::AppState,
};
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

/// Generate a valid WireGuard public key (base64-encoded 32 bytes = 44 chars).
fn gen_wg_key() -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let bytes: [u8; 32] = rand::random();
    STANDARD.encode(bytes)
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

/// Test harness for node API tests.
struct NodeApiTestHarness {
    base_url: String,
    client: reqwest::Client,
    _postgres: testcontainers::ContainerAsync<GenericImage>,
    shutdown_tx: watch::Sender<bool>,
    scheduler_pool: sqlx::PgPool,
}

impl NodeApiTestHarness {
    async fn new() -> Self {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info,plfm_control_plane=debug,sqlx=warn".into()),
            )
            .with_test_writer()
            .try_init();

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

        let pool = db.pool().clone();
        let scheduler_pool = pool.clone();
        let projection_worker = ProjectionWorker::new(pool, WorkerConfig::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = projection_worker.run(shutdown_rx).await;
        });

        let state = AppState::new(db);
        let app = api::create_router(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();

        Self {
            base_url,
            client,
            _postgres: postgres,
            shutdown_tx,
            scheduler_pool,
        }
    }

    async fn issue_user_token(&self, email: &str) -> String {
        let resp = self
            .client
            .post(format!("{}/v1/auth/device/start", self.base_url))
            .json(&serde_json::json!({ "device_name": "itest-node-agent" }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());

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
            "logs:read"
        ]);

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
        .execute(&self.scheduler_pool)
        .await
        .unwrap();

        let resp = self
            .client
            .post(format!("{}/v1/auth/device/token", self.base_url))
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let tokens: serde_json::Value = resp.json().await.unwrap();
        tokens["access_token"]
            .as_str()
            .expect("missing access_token")
            .to_string()
    }

    /// Create a valid node enrollment payload.
    fn enroll_payload(&self, hostname: &str) -> serde_json::Value {
        serde_json::json!({
            "hostname": hostname,
            "region": "us-west-2",
            "wireguard_public_key": gen_wg_key(),
            "agent_mtls_subject": format!("CN={}", hostname),
            "public_ipv6": "2001:db8::1",
            "cpu_cores": 8,
            "memory_bytes": 17179869184_i64,
            "labels": { "pool": "general" }
        })
    }

    fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for NodeApiTestHarness {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[tokio::test]
async fn test_node_enrollment() {
    let harness = NodeApiTestHarness::new().await;

    // Enroll a new node
    let enroll_url = format!("{}/v1/nodes/enroll", harness.base_url);
    let resp = harness
        .client
        .post(&enroll_url)
        .json(&harness.enroll_payload("node-01.example.com"))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Enrollment should succeed: {} - {:?}",
        status,
        body
    );

    let node_id = body["id"].as_str().expect("missing node id");
    assert!(
        node_id.starts_with("node_"),
        "Node ID should have correct prefix"
    );
    let overlay_ipv6 = body["overlay_ipv6"].as_str().expect("missing overlay_ipv6");
    assert!(!overlay_ipv6.is_empty(), "overlay_ipv6 should be set");

    // Verify the node appears in the list
    tokio::time::sleep(Duration::from_millis(200)).await; // Wait for projection
    let list_url = format!("{}/v1/nodes", harness.base_url);
    let resp = harness.client.get(&list_url).send().await.unwrap();
    assert!(resp.status().is_success());
    let list: serde_json::Value = resp.json().await.unwrap();
    let items = list["items"].as_array().expect("missing items");
    assert!(items.iter().any(|n| n["id"] == node_id));
}

#[tokio::test]
async fn test_node_heartbeat() {
    let harness = NodeApiTestHarness::new().await;

    // Enroll a node first
    let enroll_url = format!("{}/v1/nodes/enroll", harness.base_url);
    let resp = harness
        .client
        .post(&enroll_url)
        .json(&harness.enroll_payload("node-heartbeat.example.com"))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Enrollment failed: {} - {:?}",
        status,
        body
    );
    let node_id = body["id"].as_str().expect("missing node id").to_string();

    // Wait for projection to create node in nodes_view
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send a heartbeat with correct format
    let heartbeat_url = format!("{}/v1/nodes/{}/heartbeat", harness.base_url, node_id);
    let resp = harness
        .client
        .post(&heartbeat_url)
        .json(&serde_json::json!({
            "state": "active",
            "available_cpu_cores": 6,
            "available_memory_bytes": 12884901888_i64,
            "instance_count": 2
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Heartbeat should succeed: {} - {:?}",
        status,
        body
    );
    assert!(
        body["accepted"].as_bool().unwrap_or(false),
        "Heartbeat should be accepted"
    );
    assert!(
        body["next_heartbeat_secs"].as_i64().is_some(),
        "Should have next_heartbeat_secs"
    );
}

#[tokio::test]
async fn test_node_plan_empty() {
    let harness = NodeApiTestHarness::new().await;

    // Enroll a node
    let enroll_url = format!("{}/v1/nodes/enroll", harness.base_url);
    let resp = harness
        .client
        .post(&enroll_url)
        .json(&harness.enroll_payload("node-plan.example.com"))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Enrollment failed: {} - {:?}",
        status,
        body
    );
    let node_id = body["id"].as_str().expect("missing node id").to_string();

    // Wait for projection
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Get plan - should be empty since no instances are scheduled
    let plan_url = format!("{}/v1/nodes/{}/plan", harness.base_url, node_id);
    let resp = harness.client.get(&plan_url).send().await.unwrap();

    let status = resp.status();
    let plan: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Plan request should succeed: {} - {:?}",
        status,
        plan
    );

    let instances = plan["instances"]
        .as_array()
        .expect("missing instances array");
    assert!(instances.is_empty(), "Plan should be empty for new node");
}

#[tokio::test]
async fn test_node_plan_with_scheduled_instance() {
    let harness = NodeApiTestHarness::new().await;

    // Create org, app, env, release, deploy flow
    let access_token = harness.issue_user_token("test@example.com").await;
    let auth_header = format!("Bearer {}", access_token);
    let org_name = format!("org-{}", unique_suffix());

    // Create org
    let resp = harness
        .client
        .post(format!("{}/v1/orgs", harness.base_url))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", format!("org-{}", unique_suffix()))
        .json(&serde_json::json!({ "name": org_name }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "Create org failed");
    let org_id = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create app
    let resp = harness
        .client
        .post(format!("{}/v1/orgs/{}/apps", harness.base_url, org_id))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", format!("app-{}", unique_suffix()))
        .json(&serde_json::json!({ "name": "myapp", "description": "test app" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "Create app failed");
    let app_id = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create env
    let resp = harness
        .client
        .post(format!(
            "{}/v1/orgs/{}/apps/{}/envs",
            harness.base_url, org_id, app_id
        ))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", format!("env-{}", unique_suffix()))
        .json(&serde_json::json!({ "name": "production" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "Create env failed");
    let env_id = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create release
    let resp = harness
        .client
        .post(format!(
            "{}/v1/orgs/{}/apps/{}/releases",
            harness.base_url, org_id, app_id
        ))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", format!("rel-{}", unique_suffix()))
        .json(&serde_json::json!({
            "image_ref": "ghcr.io/example/app:v1",
            "image_digest": "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            "manifest_schema_version": 2,
            "manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "command": ["./start"]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "Create release failed");
    let release_id = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Create deploy
    let resp = harness
        .client
        .post(format!(
            "{}/v1/orgs/{}/apps/{}/envs/{}/deploys",
            harness.base_url, org_id, app_id, env_id
        ))
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", format!("dep-{}", unique_suffix()))
        .json(&serde_json::json!({ "release_id": release_id }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "Create deploy failed");

    // Enroll a node
    let resp = harness
        .client
        .post(format!("{}/v1/nodes/enroll", harness.base_url))
        .json(&harness.enroll_payload("scheduler-node.example.com"))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Enrollment failed: {} - {:?}",
        status,
        body
    );
    let node_id = body["id"].as_str().unwrap().to_string();

    // Wait for projections
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Run scheduler
    let reconciler = SchedulerReconciler::new(harness.scheduler_pool.clone());
    reconciler.reconcile_all().await.unwrap();

    // Wait for instance to be created
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get plan - should have at least one instance
    let plan_url = format!("{}/v1/nodes/{}/plan", harness.base_url, node_id);
    let resp = harness.client.get(&plan_url).send().await.unwrap();

    let status = resp.status();
    let plan: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Plan request should succeed: {} - {:?}",
        status,
        plan
    );

    let instances = plan["instances"]
        .as_array()
        .expect("missing instances array");
    assert!(!instances.is_empty(), "Plan should have scheduled instance");

    let instance = &instances[0];
    assert!(
        instance["instance_id"].as_str().is_some(),
        "missing instance_id"
    );
    let workload = &instance["workload"];
    assert!(
        workload["release_id"].as_str().is_some(),
        "missing workload.release_id"
    );
    assert!(workload["image"].is_object(), "missing workload.image");
}

#[tokio::test]
async fn test_instance_status_reporting() {
    let harness = NodeApiTestHarness::new().await;

    // Enroll a node
    let resp = harness
        .client
        .post(format!("{}/v1/nodes/enroll", harness.base_url))
        .json(&harness.enroll_payload("status-node.example.com"))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Enrollment failed: {} - {:?}",
        status,
        body
    );
    let node_id = body["id"].as_str().unwrap().to_string();

    // Insert an instance directly into the database for testing
    let instance_id = plfm_id::InstanceId::new().to_string();
    let org_id = plfm_id::OrgId::new().to_string();
    let app_id = plfm_id::AppId::new().to_string();
    let env_id = plfm_id::EnvId::new().to_string();
    let release_id = plfm_id::ReleaseId::new().to_string();

    sqlx::query(
        r#"
        INSERT INTO instances_desired_view (
            instance_id, org_id, app_id, env_id, process_type, node_id,
            desired_state, release_id, overlay_ipv6, resources_snapshot,
            spec_hash, generation, resource_version, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, 'web', $5, 'running', $6,
                'fd00:1::1'::inet, '{}'::jsonb, 'abc123', 1, 1, now(), now())
        "#,
    )
    .bind(&instance_id)
    .bind(&org_id)
    .bind(&app_id)
    .bind(&env_id)
    .bind(&node_id)
    .bind(&release_id)
    .execute(&harness.scheduler_pool)
    .await
    .unwrap();

    // Report status - anonymous requests are treated as System actors
    // (this is intentional for node-agent to control-plane communication).
    // The endpoint should accept the status report.
    let status_url = format!(
        "{}/v1/nodes/{}/instances/{}/status",
        harness.base_url, node_id, instance_id
    );
    let resp = harness
        .client
        .post(&status_url)
        .json(&serde_json::json!({
            "status": "ready",
            "boot_id": "boot-12345"
        }))
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        status.is_success(),
        "Status report should succeed: {} - {:?}",
        status,
        body
    );
    assert!(
        body["accepted"].as_bool().unwrap_or(false),
        "Status should be accepted"
    );

    // Wait for projection
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify status was recorded
    let row: Option<(String,)> =
        sqlx::query_as("SELECT status FROM instances_status_view WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&harness.scheduler_pool)
            .await
            .unwrap();

    assert_eq!(row.map(|r| r.0), Some("ready".to_string()));

    let resp_failed = harness
        .client
        .post(&status_url)
        .json(&serde_json::json!({
            "status": "failed",
            "error_message": "OOM killed",
            "exit_code": 137
        }))
        .send()
        .await
        .unwrap();

    let failed_status = resp_failed.status();
    let failed_body: serde_json::Value = resp_failed.json().await.unwrap();
    assert!(
        failed_status.is_success(),
        "Status report should succeed: {} - {:?}",
        failed_status,
        failed_body
    );
    assert!(
        failed_body["accepted"].as_bool().unwrap_or(false),
        "Status should be accepted"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;

    let row: Option<(String,)> =
        sqlx::query_as("SELECT status FROM instances_status_view WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&harness.scheduler_pool)
            .await
            .unwrap();

    assert_eq!(row.map(|r| r.0), Some("failed".to_string()));

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE aggregate_id = $1 AND event_type = 'instance.status_changed' ORDER BY event_id DESC LIMIT 1",
    )
    .bind(&instance_id)
    .fetch_one(&harness.scheduler_pool)
    .await
    .unwrap();

    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["reason_detail"], "OOM killed");
    assert_eq!(payload["exit_code"], 137);
}
