use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

async fn issue_device_token(
    client: &reqwest::Client,
    base_url: &str,
    db: &Database,
    email: &str,
) -> String {
    let resp = client
        .post(format!("{base_url}/v1/auth/device/start"))
        .json(&serde_json::json!({ "device_name": "itest" }))
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
    .execute(db.pool())
    .await
    .unwrap();

    let resp = client
        .post(format!("{base_url}/v1/auth/device/token"))
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

fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[tokio::test]
async fn core_loop_request_id_idempotency_ryw_scale_and_instances() {
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

    // The HTTP handlers rely on projections being applied to satisfy RYW semantics.
    let pool = db.pool().clone();
    let scheduler_pool = pool.clone();
    let projection_worker = ProjectionWorker::new(pool, WorkerConfig::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let projection_handle = tokio::spawn(async move {
        let _ = projection_worker.run(shutdown_rx).await;
    });

    let state = AppState::new(db.clone());
    let app = api::create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let access_token = issue_device_token(&client, &base_url, &db, "itest@example.com").await;
    let auth_header = format!("Bearer {}", access_token);

    let idem_key = format!("itest-org-{}-key", unique_suffix());
    let org_name = format!("itest-org-{}", unique_suffix());
    let create_url = format!("{base_url}/v1/orgs");

    let resp1 = client
        .post(&create_url)
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
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
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_secrets_get_2.status().is_success());
    let secrets_get_2: serde_json::Value = resp_secrets_get_2.json().await.unwrap();
    assert_eq!(secrets_get_2["bundle_id"], bundle_id);
    assert_eq!(secrets_get_2["current_version_id"], version_id);

    // Volumes: create, attach, snapshot, restore.
    let create_volume_url = format!("{base_url}/v1/orgs/{org_id}/volumes");
    let idem_volume = format!("itest-vol-{}-key", unique_suffix());
    let resp_volume_create = client
        .post(&create_volume_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_volume)
        .json(&serde_json::json!({
            "name": "itest-data",
            "size_bytes": 1073741824,
            "filesystem": "ext4",
            "backup_enabled": true
        }))
        .send()
        .await
        .unwrap();
    assert!(resp_volume_create.status().is_success());
    let volume: serde_json::Value = resp_volume_create.json().await.unwrap();
    let volume_id = volume["id"]
        .as_str()
        .expect("missing volume id")
        .to_string();

    let get_volume_url = format!("{base_url}/v1/orgs/{org_id}/volumes/{volume_id}");
    let resp_volume_get = client
        .get(&get_volume_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_volume_get.status().is_success());
    let volume_get: serde_json::Value = resp_volume_get.json().await.unwrap();
    assert_eq!(
        volume_get["id"].as_str().expect("missing volume id"),
        volume_id.as_str()
    );
    let volume_attachments = volume_get["attachments"]
        .as_array()
        .expect("missing attachments");
    assert!(volume_attachments.is_empty());

    let attach_url =
        format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments");
    let idem_attach = format!("itest-attach-{}-key", unique_suffix());
    let resp_attach = client
        .post(&attach_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_attach)
        .json(&serde_json::json!({
            "volume_id": &volume_id,
            "process_type": "web",
            "mount_path": "/data",
            "read_only": false
        }))
        .send()
        .await
        .unwrap();
    assert!(resp_attach.status().is_success());
    let attachment: serde_json::Value = resp_attach.json().await.unwrap();
    let attachment_id = attachment["id"].clone();

    let resp_volume_get_2 = client
        .get(&get_volume_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_volume_get_2.status().is_success());
    let volume_get_2: serde_json::Value = resp_volume_get_2.json().await.unwrap();
    let attachments = volume_get_2["attachments"]
        .as_array()
        .expect("missing attachments");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["id"], attachment_id);

    let snapshot_url = format!("{base_url}/v1/orgs/{org_id}/volumes/{volume_id}/snapshots");
    let idem_snapshot = format!("itest-snap-{}-key", unique_suffix());
    let resp_snapshot = client
        .post(&snapshot_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_snapshot)
        .json(&serde_json::json!({ "note": "itest" }))
        .send()
        .await
        .unwrap();
    assert!(resp_snapshot.status().is_success());
    let snapshot: serde_json::Value = resp_snapshot.json().await.unwrap();
    let snapshot_id = snapshot["id"]
        .as_str()
        .expect("missing snapshot id")
        .to_string();

    let list_snapshots_url =
        format!("{base_url}/v1/orgs/{org_id}/volumes/{volume_id}/snapshots?limit=200");
    let resp_snapshots_list = client
        .get(&list_snapshots_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_snapshots_list.status().is_success());
    let snapshots_list: serde_json::Value = resp_snapshots_list.json().await.unwrap();
    let items = snapshots_list["items"]
        .as_array()
        .expect("missing snapshot items");
    assert!(!items.is_empty());

    let restore_url = format!("{base_url}/v1/orgs/{org_id}/volumes/{volume_id}/restore");
    let idem_restore = format!("itest-restore-{}-key", unique_suffix());
    let resp_restore = client
        .post(&restore_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_restore)
        .json(
            &serde_json::json!({ "snapshot_id": snapshot_id, "new_volume_name": "itest-restored" }),
        )
        .send()
        .await
        .unwrap();
    assert!(resp_restore.status().is_success());
    let restored: serde_json::Value = resp_restore.json().await.unwrap();
    assert_ne!(
        restored["id"].as_str().expect("missing restored volume id"),
        volume_id.as_str()
    );

    // Create a release.
    let create_release_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/releases");
    let idem_release = format!("itest-release-{}-key", unique_suffix());
    let resp_release = client
        .post(&create_release_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_release)
        .json(&serde_json::json!({
            "image_ref": format!("example.com/{app_name}:demo"),
            "image_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "manifest_schema_version": 1,
            "manifest_hash": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "command": ["./start", "--port", "8080"]
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
        .header("Authorization", &auth_header)
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
    let first = wait_for_instances(&client, &instances_url, &auth_header, 1).await;
    assert_eq!(
        first["items"]
            .as_array()
            .expect("missing items array")
            .len(),
        1
    );

    let first_instance_id = first["items"][0]["id"]
        .as_str()
        .expect("missing instance id")
        .to_string();

    // Report instance status as ready (system actor) so exec grants are allowed.
    let report_status_url =
        format!("{base_url}/v1/nodes/{node_id}/instances/{first_instance_id}/status");
    let resp_status = client
        .post(&report_status_url)
        .json(&serde_json::json!({ "status": "ready" }))
        .send()
        .await
        .unwrap();
    assert!(resp_status.status().is_success());

    // Wait until env-scoped instance view reflects ready status.
    let get_instance_url = format!(
        "{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{first_instance_id}"
    );
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);
    loop {
        let resp = client
            .get(&get_instance_url)
            .header("Authorization", &auth_header)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.unwrap();
        if body["status"].as_str() == Some("ready") {
            break;
        }
        if start.elapsed() > timeout {
            panic!("timed out waiting for instance to become ready; last body: {body}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Exec: create an exec grant (and verify idempotency replay).
    let exec_url = format!(
        "{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/instances/{first_instance_id}/exec"
    );
    let idem_exec = format!("itest-exec-{}-key", unique_suffix());
    let exec_body = serde_json::json!({
        "command": ["sh", "-lc", "uptime"],
        "tty": true
    });

    let resp_exec_1 = client
        .post(&exec_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_exec)
        .json(&exec_body)
        .send()
        .await
        .unwrap();
    assert!(resp_exec_1.status().is_success());
    let exec_1: serde_json::Value = resp_exec_1.json().await.unwrap();
    let exec_session_id = exec_1["session_id"]
        .as_str()
        .expect("missing session_id")
        .to_string();

    let resp_exec_2 = client
        .post(&exec_url)
        .header("Authorization", &auth_header)
        .header("Idempotency-Key", &idem_exec)
        .json(&exec_body)
        .send()
        .await
        .unwrap();
    assert!(resp_exec_2.status().is_success());
    let exec_2: serde_json::Value = resp_exec_2.json().await.unwrap();
    assert_eq!(exec_2["session_id"], exec_1["session_id"]);
    assert_eq!(exec_2["connect_url"], exec_1["connect_url"]);
    assert_eq!(exec_2["session_token"], exec_1["session_token"]);
    assert_eq!(exec_2["expires_in_seconds"], exec_1["expires_in_seconds"]);

    // Verify the exec session is materialized for auditing.
    let exec_row = sqlx::query_as::<_, (String, serde_json::Value, bool)>(
        r#"
        SELECT status, requested_command, tty
        FROM exec_sessions_view
        WHERE exec_session_id = $1
        "#,
    )
    .bind(&exec_session_id)
    .fetch_one(&scheduler_pool)
    .await
    .unwrap();
    assert_eq!(exec_row.0, "granted");
    assert!(exec_row.2);
    assert_eq!(exec_row.1, serde_json::json!(["sh", "-lc", "uptime"]));

    // Scale up to 2 instances using optimistic concurrency (GET then PUT).
    let scale_url = format!("{base_url}/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/scale");
    let resp_scale_get = client
        .get(&scale_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .unwrap();
    assert!(resp_scale_get.status().is_success());
    let scale_state: serde_json::Value = resp_scale_get.json().await.unwrap();
    let current_version = scale_state["resource_version"].as_i64().unwrap_or(0) as i32;

    let idem_scale = format!("itest-scale-{}-key", unique_suffix());
    let resp_scale_put = client
        .put(&scale_url)
        .header("Authorization", &auth_header)
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

    // Reconcile again; volume-backed processes are clamped to a single replica in v1.
    reconciler.reconcile_all().await.unwrap();
    let second = wait_for_instances(&client, &instances_url, &auth_header, 1).await;
    assert_eq!(
        second["items"]
            .as_array()
            .expect("missing items array")
            .len(),
        1
    );

    let _ = shutdown_tx.send(true);
    server_handle.abort();
    let _ = server_handle.await;
    projection_handle.abort();
    let _ = projection_handle.await;
}
