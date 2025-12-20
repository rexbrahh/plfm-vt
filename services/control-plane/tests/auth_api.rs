use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plfm_control_plane::{api, db::{Database, DbConfig}, state::AppState};
use plfm_id::OrgId;
use testcontainers::{core::IntoContainerPort, runners::AsyncRunner, GenericImage, ImageExt};
use tokio::net::TcpListener;

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

struct ApiFixture {
    base_url: String,
    db: Database,
    _postgres: testcontainers::ContainerAsync<GenericImage>,
}

async fn start_api() -> ApiFixture {
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

    let state = AppState::new(db.clone());
    let app = api::create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    ApiFixture {
        base_url,
        db,
        _postgres: postgres,
    }
}

#[tokio::test]
async fn device_flow_token_refresh_and_revoke() {
    let fixture = start_api().await;
    let base_url = fixture.base_url;
    let db = fixture.db;
    let client = reqwest::Client::new();

    // Start device flow
    let resp = client
        .post(format!("{base_url}/v1/auth/device/start"))
        .json(&serde_json::json!({ "device_name": "itest-cli" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let body: serde_json::Value = resp.json().await.unwrap();
    let device_code = body["device_code"].as_str().expect("missing device_code");
    let user_code = body["user_code"].as_str().expect("missing user_code");

    // Approve device code via DB (simulating user approval UI)
    let subject_id = format!("usr_test_{}", unique_suffix());
    let subject_email = format!("tester-{}@example.com", unique_suffix());
    let scopes = serde_json::json!(["orgs:read", "apps:read", "logs:read"]);

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
    .bind(&subject_email)
    .bind(scopes)
    .bind(user_code)
    .execute(db.pool())
    .await
    .unwrap();

    // Poll for token
    let resp = client
        .post(format!("{base_url}/v1/auth/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let tokens: serde_json::Value = resp.json().await.unwrap();
    let access_token = tokens["access_token"].as_str().expect("missing access_token");
    let refresh_token = tokens["refresh_token"].as_str().expect("missing refresh_token");

    // whoami should succeed with access token
    let resp = client
        .get(format!("{base_url}/v1/auth/whoami"))
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let whoami: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(whoami["subject_type"], "user");
    assert_eq!(whoami["subject_id"], subject_id);
    assert_eq!(whoami["display_name"], subject_email);

    // Refresh token rotation
    let resp = client
        .post(format!("{base_url}/v1/auth/token/refresh"))
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let refreshed: serde_json::Value = resp.json().await.unwrap();
    let new_access = refreshed["access_token"].as_str().expect("missing access_token");
    let new_refresh = refreshed["refresh_token"].as_str().expect("missing refresh_token");
    assert_ne!(new_access, access_token);
    assert_ne!(new_refresh, refresh_token);

    // Revoke access token
    let resp = client
        .post(format!("{base_url}/v1/auth/token/revoke"))
        .json(&serde_json::json!({ "access_token": new_access }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // whoami should reject revoked token
    let resp = client
        .get(format!("{base_url}/v1/auth/whoami"))
        .header("Authorization", format!("Bearer {new_access}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn service_principal_token_flow() {
    let fixture = start_api().await;
    let base_url = fixture.base_url;
    let db = fixture.db;
    let client = reqwest::Client::new();

    let org_id = OrgId::new().to_string();
    let sp_id = format!("sp_{}", unique_suffix());
    let client_id = format!("client_{}", unique_suffix());
    let client_secret = format!("secret_{}", unique_suffix());
    let scopes = serde_json::json!(["orgs:read", "apps:read"]);

    let secret_hash = plfm_control_plane::api::tokens::hash_token(&client_secret);

    sqlx::query(
        r#"
        INSERT INTO service_principals_view (
            service_principal_id,
            org_id,
            name,
            scopes,
            client_id,
            client_secret_hash,
            resource_version
        )
        VALUES ($1, $2, $3, $4, $5, $6, 1)
        "#,
    )
    .bind(&sp_id)
    .bind(&org_id)
    .bind("itest-sp")
    .bind(scopes)
    .bind(&client_id)
    .bind(secret_hash)
    .execute(db.pool())
    .await
    .unwrap();

    let resp = client
        .post(format!("{base_url}/v1/auth/token"))
        .json(&serde_json::json!({
            "grant_type": "client_credentials",
            "client_id": client_id,
            "client_secret": client_secret
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let tokens: serde_json::Value = resp.json().await.unwrap();
    let access_token = tokens["access_token"].as_str().expect("missing access_token");

    let resp = client
        .get(format!("{base_url}/v1/auth/whoami"))
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let whoami: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(whoami["subject_type"], "service_principal");
    assert_eq!(whoami["subject_id"], sp_id);
}
