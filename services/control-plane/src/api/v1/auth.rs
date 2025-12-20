//! Authentication and identity endpoints.
//!
//! Endpoints:
//! - GET  /v1/auth/whoami - Get current identity and org memberships
//! - POST /v1/auth/device/start - Start device authorization flow
//! - POST /v1/auth/device/token - Poll for token after user approval
//! - POST /v1/auth/token - Service principal token (client credentials)
//! - POST /v1/auth/token/refresh - Refresh an access token
//! - POST /v1/auth/token/revoke - Revoke tokens

use std::collections::BTreeSet;

use axum::{
    extract::State,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use plfm_events::{ActorType, MemberRole};
use serde::{Deserialize, Serialize};

use crate::api::authz;
use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::api::tokens::{
    self, create_access_token, create_refresh_token, hash_token, revoke_access_token,
    revoke_refresh_token, validate_refresh_token, SubjectType, ACCESS_TOKEN_LIFETIME_MINUTES,
    DEVICE_CODE_LIFETIME_MINUTES, DEVICE_POLL_INTERVAL_SECONDS,
};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/whoami", get(whoami))
        .route("/device/start", post(device_start))
        .route("/device/token", post(device_token))
        .route("/token", post(token))
        .route("/token/refresh", post(token_refresh))
        .route("/token/revoke", post(token_revoke))
}

// ============================================================================
// whoami endpoint
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum WhoAmISubjectType {
    User,
    ServicePrincipal,
}

#[derive(Debug, Serialize)]
struct OrgMembership {
    org_id: String,
    role: String,
}

#[derive(Debug, Serialize)]
struct WhoAmIResponse {
    subject_type: WhoAmISubjectType,
    subject_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    org_memberships: Vec<OrgMembership>,
    scopes: Vec<String>,
}

fn scopes_for_role(role: MemberRole) -> &'static [&'static str] {
    match role {
        MemberRole::Owner | MemberRole::Admin => &[
            "orgs:read",
            "orgs:admin",
            "apps:read",
            "apps:write",
            "envs:read",
            "envs:write",
            "releases:read",
            "releases:write",
            "deploys:write",
            "rollbacks:write",
            "routes:read",
            "routes:write",
            "volumes:read",
            "volumes:write",
            "secrets:read-metadata",
            "secrets:write",
            "logs:read",
        ],
        MemberRole::Developer => &[
            "orgs:read",
            "apps:read",
            "apps:write",
            "envs:read",
            "envs:write",
            "releases:read",
            "releases:write",
            "deploys:write",
            "rollbacks:write",
            "routes:read",
            "routes:write",
            "volumes:read",
            "volumes:write",
            "secrets:read-metadata",
            "secrets:write",
            "logs:read",
        ],
        MemberRole::Readonly => &[
            "orgs:read",
            "apps:read",
            "envs:read",
            "releases:read",
            "routes:read",
            "volumes:read",
            "secrets:read-metadata",
            "logs:read",
        ],
    }
}

async fn whoami(
    State(state): State<AppState>,
    ctx: RequestContext,
) -> Result<impl IntoResponse, ApiError> {
    let RequestContext {
        request_id,
        actor_type,
        actor_id,
        actor_email,
        ..
    } = ctx;

    let subject_type = match actor_type {
        ActorType::User => WhoAmISubjectType::User,
        ActorType::ServicePrincipal => WhoAmISubjectType::ServicePrincipal,
        ActorType::System => {
            return Err(ApiError::unauthorized(
                "unauthorized",
                "Missing or invalid Authorization token",
            )
            .with_request_id(request_id));
        }
    };

    let mut org_memberships: Vec<OrgMembership> = Vec::new();
    let mut scopes: BTreeSet<String> = BTreeSet::new();
    let mut display_name: Option<String> = None;

    if actor_type == ActorType::User {
        let Some(email) = actor_email.as_deref() else {
            return Err(ApiError::unauthorized(
                "unauthorized",
                "Token subject email is required for org-scoped APIs",
            )
            .with_request_id(request_id));
        };

        display_name = Some(email.to_string());

        let rows = sqlx::query_as::<_, OrgMembershipRow>(
            r#"
            SELECT org_id, role
            FROM org_members_view
            WHERE email = $1 AND NOT is_deleted
            ORDER BY org_id ASC
            "#,
        )
        .bind(email)
        .fetch_all(state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                request_id = %request_id,
                email = %email,
                "Failed to load org memberships"
            );
            ApiError::internal("internal_error", "Failed to load identity")
                .with_request_id(request_id.clone())
        })?;

        for row in rows {
            if let Some(role) = authz::parse_member_role(&row.role) {
                for scope in scopes_for_role(role) {
                    scopes.insert(scope.to_string());
                }
            }

            org_memberships.push(OrgMembership {
                org_id: row.org_id,
                role: row.role,
            });
        }
    }

    Ok(Json(WhoAmIResponse {
        subject_type,
        subject_id: actor_id,
        display_name,
        org_memberships,
        scopes: scopes.into_iter().collect(),
    }))
}

// ============================================================================
// device/start endpoint - Start device authorization flow
// ============================================================================

#[derive(Debug, Deserialize)]
struct DeviceStartRequest {
    /// Optional device name for display in approval UI.
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeviceStartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_uri_complete: Option<String>,
    expires_in_seconds: i64,
    poll_interval_seconds: u32,
}

async fn device_start(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<DeviceStartRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Generate device code and user code
    let device_code = tokens::generate_device_code();
    let user_code = tokens::generate_user_code();
    let device_code_hash = hash_token(&device_code);
    let device_code_id = format!("dc_{}", plfm_id::RequestId::new());

    let expires_at = Utc::now() + Duration::minutes(DEVICE_CODE_LIFETIME_MINUTES);

    // Insert into database
    sqlx::query(
        r#"
        INSERT INTO device_codes (
            device_code_id, device_code_hash, user_code, status,
            device_name, expires_at
        )
        VALUES ($1, $2, $3, 'pending', $4, $5)
        "#,
    )
    .bind(&device_code_id)
    .bind(&device_code_hash)
    .bind(&user_code)
    .bind(&req.device_name)
    .bind(expires_at)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create device code");
        ApiError::internal("internal_error", "Failed to start device authorization")
            .with_request_id(request_id.clone())
    })?;

    // TODO: Make this configurable
    let verification_uri = "https://auth.plfm.dev/device".to_string();
    let verification_uri_complete = Some(format!("{}?code={}", verification_uri, user_code));

    Ok(Json(DeviceStartResponse {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in_seconds: DEVICE_CODE_LIFETIME_MINUTES * 60,
        poll_interval_seconds: DEVICE_POLL_INTERVAL_SECONDS,
    }))
}

// ============================================================================
// device/token endpoint - Poll for token after user approval
// ============================================================================

#[derive(Debug, Deserialize)]
struct DeviceTokenRequest {
    device_code: String,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    token_type: &'static str,
    expires_in_seconds: i64,
}

async fn device_token(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<DeviceTokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate device code format
    if !req.device_code.starts_with(tokens::DEVICE_CODE_PREFIX) {
        return Err(ApiError::bad_request("invalid_request", "Invalid device code format")
            .with_request_id(request_id));
    }

    let device_code_hash = hash_token(&req.device_code);

    // Look up device code
    let row = sqlx::query_as::<_, DeviceCodeRow>(
        r#"
        SELECT device_code_id, status, approved_subject_type, approved_subject_id,
               approved_subject_email, approved_scopes, expires_at, last_polled_at, poll_count
        FROM device_codes
        WHERE device_code_hash = $1
        "#,
    )
    .bind(&device_code_hash)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to query device code");
        ApiError::internal("internal_error", "Failed to check device code")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(ApiError::bad_request("invalid_grant", "Invalid device code")
            .with_request_id(request_id));
    };

    // Check expiry
    if row.expires_at < Utc::now() {
        return Err(ApiError::bad_request("expired_token", "Device code has expired")
            .with_request_id(request_id));
    }

    // Rate limit polling
    if let Some(last_polled) = row.last_polled_at {
        let elapsed = Utc::now().signed_duration_since(last_polled);
        if elapsed.num_seconds() < DEVICE_POLL_INTERVAL_SECONDS as i64 {
            // Update poll tracking even for slow_down response
            let _ = sqlx::query(
                r#"
                UPDATE device_codes
                SET last_polled_at = now(), poll_count = poll_count + 1
                WHERE device_code_id = $1
                "#,
            )
            .bind(&row.device_code_id)
            .execute(state.db().pool())
            .await;

            return Err(ApiError::bad_request(
                "slow_down",
                "Polling too frequently, please wait before retrying",
            )
            .with_request_id(request_id));
        }
    }

    // Update poll tracking
    sqlx::query(
        r#"
        UPDATE device_codes
        SET last_polled_at = now(), poll_count = poll_count + 1
        WHERE device_code_id = $1
        "#,
    )
    .bind(&row.device_code_id)
    .execute(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to update poll tracking");
        ApiError::internal("internal_error", "Failed to check device code")
            .with_request_id(request_id.clone())
    })?;

    match row.status.as_str() {
        "pending" => {
            Err(ApiError::bad_request(
                "authorization_pending",
                "User has not yet authorized this device",
            )
            .with_request_id(request_id))
        }
        "denied" => {
            Err(ApiError::bad_request("access_denied", "User denied authorization")
                .with_request_id(request_id))
        }
        "expired" => {
            Err(ApiError::bad_request("expired_token", "Device code has expired")
                .with_request_id(request_id))
        }
        "consumed" => {
            Err(ApiError::bad_request("invalid_grant", "Device code has already been used")
                .with_request_id(request_id))
        }
        "approved" => {
            // User has approved! Generate tokens
            let subject_type_str = row.approved_subject_type.ok_or_else(|| {
                ApiError::internal("internal_error", "Approved device code missing subject type")
                    .with_request_id(request_id.clone())
            })?;
            let subject_type = SubjectType::from_str(&subject_type_str).ok_or_else(|| {
                ApiError::internal("internal_error", "Invalid subject type")
                    .with_request_id(request_id.clone())
            })?;
            let subject_id = row.approved_subject_id.ok_or_else(|| {
                ApiError::internal("internal_error", "Approved device code missing subject id")
                    .with_request_id(request_id.clone())
            })?;
            let subject_email = row.approved_subject_email;
            let scopes: Vec<String> = row
                .approved_scopes
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();

            // Mark device code as consumed
            sqlx::query(
                r#"
                UPDATE device_codes SET status = 'consumed' WHERE device_code_id = $1
                "#,
            )
            .bind(&row.device_code_id)
            .execute(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to consume device code");
                ApiError::internal("internal_error", "Failed to complete authorization")
                    .with_request_id(request_id.clone())
            })?;

            // Create refresh token first
            let (refresh_token, refresh_token_id, _) = create_refresh_token(
                state.db().pool(),
                subject_type,
                &subject_id,
                subject_email.as_deref(),
                &scopes,
                Some(&row.device_code_id),
                None,
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to create refresh token");
                ApiError::internal("internal_error", "Failed to create tokens")
                    .with_request_id(request_id.clone())
            })?;

            // Create access token
            let (access_token, _) = create_access_token(
                state.db().pool(),
                subject_type,
                &subject_id,
                subject_email.as_deref(),
                &scopes,
                Some(&refresh_token_id),
                Some(&row.device_code_id),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to create access token");
                ApiError::internal("internal_error", "Failed to create tokens")
                    .with_request_id(request_id.clone())
            })?;

            Ok(Json(TokenResponse {
                access_token,
                refresh_token: Some(refresh_token),
                token_type: "Bearer",
                expires_in_seconds: ACCESS_TOKEN_LIFETIME_MINUTES * 60,
            }))
        }
        _ => Err(ApiError::internal("internal_error", "Invalid device code status")
            .with_request_id(request_id)),
    }
}

// ============================================================================
// token endpoint - Service principal client credentials
// ============================================================================

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    client_secret: String,
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

async fn token(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<TokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Only client_credentials grant type supported
    if req.grant_type != "client_credentials" {
        return Err(ApiError::bad_request(
            "unsupported_grant_type",
            "Only client_credentials grant type is supported",
        )
        .with_request_id(request_id));
    }

    // Look up service principal by client_id
    let sp = sqlx::query_as::<_, ServicePrincipalRow>(
        r#"
        SELECT service_principal_id, org_id, name, scopes, client_secret_hash
        FROM service_principals_view
        WHERE client_id = $1 AND NOT is_deleted
        "#,
    )
    .bind(&req.client_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to query service principal");
        ApiError::internal("internal_error", "Failed to authenticate")
            .with_request_id(request_id.clone())
    })?;

    let Some(sp) = sp else {
        return Err(
            ApiError::unauthorized("invalid_client", "Invalid client credentials")
                .with_request_id(request_id),
        );
    };

    // Verify client secret
    let Some(stored_hash) = sp.client_secret_hash else {
        return Err(
            ApiError::unauthorized("invalid_client", "Client credentials not configured")
                .with_request_id(request_id),
        );
    };

    let secret_hash = hash_token(&req.client_secret);
    if secret_hash != stored_hash {
        return Err(
            ApiError::unauthorized("invalid_client", "Invalid client credentials")
                .with_request_id(request_id),
        );
    }

    // Get allowed scopes
    let allowed_scopes: Vec<String> =
        serde_json::from_value(sp.scopes.clone()).unwrap_or_default();

    // If scopes requested, validate they're a subset
    let granted_scopes = if let Some(requested_scopes) = req.scopes {
        let allowed_set: std::collections::HashSet<_> = allowed_scopes.iter().collect();
        for scope in &requested_scopes {
            if !allowed_set.contains(scope) {
                return Err(ApiError::bad_request(
                    "invalid_scope",
                    format!("Scope '{}' is not allowed for this client", scope),
                )
                .with_request_id(request_id));
            }
        }
        requested_scopes
    } else {
        allowed_scopes
    };

    // Create access token (no refresh token for service principals by default)
    let (access_token, _) = create_access_token(
        state.db().pool(),
        SubjectType::ServicePrincipal,
        &sp.service_principal_id,
        None,
        &granted_scopes,
        None,
        None,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create access token");
        ApiError::internal("internal_error", "Failed to create token")
            .with_request_id(request_id.clone())
    })?;

    Ok(Json(TokenResponse {
        access_token,
        refresh_token: None,
        token_type: "Bearer",
        expires_in_seconds: ACCESS_TOKEN_LIFETIME_MINUTES * 60,
    }))
}

// ============================================================================
// token/refresh endpoint - Refresh an access token
// ============================================================================

#[derive(Debug, Deserialize)]
struct TokenRefreshRequest {
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct TokenRefreshResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in_seconds: i64,
}

async fn token_refresh(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<TokenRefreshRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate refresh token
    let validated =
        validate_refresh_token(state.db().pool(), &req.refresh_token, &request_id).await?;

    // Revoke the old refresh token (rotation)
    revoke_refresh_token(state.db().pool(), &req.refresh_token)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to revoke old refresh token");
            ApiError::internal("internal_error", "Failed to refresh token")
                .with_request_id(request_id.clone())
        })?;

    // Create new refresh token
    let (new_refresh_token, new_refresh_token_id, _) = create_refresh_token(
        state.db().pool(),
        validated.subject_type,
        &validated.subject_id,
        validated.subject_email.as_deref(),
        &validated.scopes,
        None,
        Some(&validated.token_id),
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create new refresh token");
        ApiError::internal("internal_error", "Failed to refresh token")
            .with_request_id(request_id.clone())
    })?;

    // Create new access token
    let (access_token, _) = create_access_token(
        state.db().pool(),
        validated.subject_type,
        &validated.subject_id,
        validated.subject_email.as_deref(),
        &validated.scopes,
        Some(&new_refresh_token_id),
        None,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to create access token");
        ApiError::internal("internal_error", "Failed to refresh token")
            .with_request_id(request_id.clone())
    })?;

    Ok(Json(TokenRefreshResponse {
        access_token,
        refresh_token: new_refresh_token,
        token_type: "Bearer",
        expires_in_seconds: ACCESS_TOKEN_LIFETIME_MINUTES * 60,
    }))
}

// ============================================================================
// token/revoke endpoint - Revoke tokens
// ============================================================================

#[derive(Debug, Deserialize)]
struct TokenRevokeRequest {
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenRevokeResponse {
    revoked: bool,
}

async fn token_revoke(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<TokenRevokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Must provide at least one token
    if req.refresh_token.is_none() && req.access_token.is_none() {
        return Err(ApiError::bad_request(
            "invalid_request",
            "Must provide either refresh_token or access_token",
        )
        .with_request_id(request_id));
    }

    let mut revoked = false;

    // Revoke refresh token (and associated access tokens)
    if let Some(refresh_token) = &req.refresh_token {
        let result = revoke_refresh_token(state.db().pool(), refresh_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to revoke refresh token");
                ApiError::internal("internal_error", "Failed to revoke token")
                    .with_request_id(request_id.clone())
            })?;
        revoked = revoked || result;
    }

    // Revoke access token
    if let Some(access_token) = &req.access_token {
        let result = revoke_access_token(state.db().pool(), access_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to revoke access token");
                ApiError::internal("internal_error", "Failed to revoke token")
                    .with_request_id(request_id.clone())
            })?;
        revoked = revoked || result;
    }

    // Per spec, revocation is idempotent - return success even if token not found
    Ok(Json(TokenRevokeResponse { revoked }))
}

// ============================================================================
// Database row types
// ============================================================================

#[derive(Debug)]
struct OrgMembershipRow {
    org_id: String,
    role: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for OrgMembershipRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            role: row.try_get("role")?,
        })
    }
}

#[derive(Debug)]
struct DeviceCodeRow {
    device_code_id: String,
    status: String,
    approved_subject_type: Option<String>,
    approved_subject_id: Option<String>,
    approved_subject_email: Option<String>,
    approved_scopes: Option<serde_json::Value>,
    expires_at: DateTime<Utc>,
    last_polled_at: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    poll_count: i32,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for DeviceCodeRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            device_code_id: row.try_get("device_code_id")?,
            status: row.try_get("status")?,
            approved_subject_type: row.try_get("approved_subject_type")?,
            approved_subject_id: row.try_get("approved_subject_id")?,
            approved_subject_email: row.try_get("approved_subject_email")?,
            approved_scopes: row.try_get("approved_scopes")?,
            expires_at: row.try_get("expires_at")?,
            last_polled_at: row.try_get("last_polled_at")?,
            poll_count: row.try_get("poll_count")?,
        })
    }
}

#[derive(Debug)]
struct ServicePrincipalRow {
    service_principal_id: String,
    #[allow(dead_code)]
    org_id: String,
    #[allow(dead_code)]
    name: String,
    scopes: serde_json::Value,
    client_secret_hash: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ServicePrincipalRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            service_principal_id: row.try_get("service_principal_id")?,
            org_id: row.try_get("org_id")?,
            name: row.try_get("name")?,
            scopes: row.try_get("scopes")?,
            client_secret_hash: row.try_get("client_secret_hash")?,
        })
    }
}
