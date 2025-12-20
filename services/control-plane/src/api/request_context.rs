//! Request-scoped context extracted from HTTP requests.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use plfm_events::ActorType;
use plfm_id::RequestId;
use sha2::{Digest, Sha256};

use crate::api::error::ApiError;
use crate::api::tokens;
use crate::state::AppState;

pub const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";
pub const AUTHORIZATION_HEADER: &str = "Authorization";

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: String,
    pub idempotency_key: Option<String>,
    pub actor_type: ActorType,
    pub actor_id: String,
    pub actor_email: Option<String>,
    pub scopes: Vec<String>,
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn dev_mode_enabled() -> bool {
    std::env::var("GHOST_DEV")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

async fn actor_from_authorization_header(
    state: &AppState,
    headers: &HeaderMap,
    request_id: &str,
) -> Result<Option<(ActorType, String, Option<String>, Vec<String>)>, ApiError> {
    let Some(auth_value) = header_string(headers, AUTHORIZATION_HEADER) else {
        return Ok(None);
    };

    let auth_value = auth_value.trim();
    let Some(token) = auth_value.strip_prefix("Bearer ") else {
        return Err(ApiError::unauthorized(
            "invalid_authorization",
            "Authorization must be a Bearer token",
        )
        .with_request_id(request_id.to_string()));
    };

    let token = token.trim();
    if token.is_empty() {
        return Err(ApiError::unauthorized(
            "invalid_authorization",
            "Authorization Bearer token cannot be empty",
        )
        .with_request_id(request_id.to_string()));
    }

    // Dev-only fallback tokens for local workflows and tests.
    if dev_mode_enabled() {
        if let Some(email) = token.strip_prefix("user:") {
            let email = email.trim();
            if email.is_empty() || email.len() > 320 || !email.contains('@') {
                return Err(ApiError::unauthorized(
                    "invalid_token",
                    "user token must be in the form 'user:<email>'",
                )
                .with_request_id(request_id.to_string()));
            }

            // Important: never persist or log bearer tokens. Derive a stable, non-secret actor id.
            let digest = Sha256::digest(email.as_bytes());
            let hex = format!("{:x}", digest);
            let short = hex.get(..32).unwrap_or(&hex);

            return Ok(Some((
                ActorType::User,
                format!("usr_{short}"),
                Some(email.to_string()),
                Vec::new(),
            )));
        }

        if let Some(sp_id) = token.strip_prefix("sp:") {
            let sp_id = sp_id.trim();
            if sp_id.is_empty() {
                return Err(ApiError::unauthorized(
                    "invalid_token",
                    "service principal token must be in the form 'sp:<id>'",
                )
                .with_request_id(request_id.to_string()));
            }

            return Ok(Some((
                ActorType::ServicePrincipal,
                sp_id.to_string(),
                None,
                Vec::new(),
            )));
        }
    }

    let token_hash = tokens::hash_token(token);
    if let Some(cached) = tokens::access_token_cache().get(&token_hash).await {
        let actor_type = match cached.subject_type {
            tokens::SubjectType::User => ActorType::User,
            tokens::SubjectType::ServicePrincipal => ActorType::ServicePrincipal,
        };
        return Ok(Some((
            actor_type,
            cached.subject_id,
            cached.subject_email,
            cached.scopes,
        )));
    }

    // Validate access token against DB.
    let validated = tokens::validate_access_token(state.db().pool(), token, request_id).await?;
    let actor_type = match validated.subject_type {
        tokens::SubjectType::User => ActorType::User,
        tokens::SubjectType::ServicePrincipal => ActorType::ServicePrincipal,
    };

    tokens::access_token_cache()
        .insert(token_hash, validated.clone())
        .await;

    Ok(Some((
        actor_type,
        validated.subject_id,
        validated.subject_email,
        validated.scopes,
    )))
}

impl FromRequestParts<AppState> for RequestContext {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = header_string(&parts.headers, "x-request-id")
            .unwrap_or_else(|| RequestId::new().to_string());

        let idempotency_key = header_string(&parts.headers, IDEMPOTENCY_KEY_HEADER);
        if let Some(key) = &idempotency_key {
            if !(8..=128).contains(&key.len()) {
                return Err(ApiError::bad_request(
                    "invalid_idempotency_key",
                    "Idempotency-Key must be between 8 and 128 characters",
                )
                .with_request_id(request_id));
            }
        }

        let (actor_type, actor_id, actor_email, scopes) =
            actor_from_authorization_header(state, &parts.headers, &request_id)
                .await?
                .unwrap_or((ActorType::System, "system".to_string(), None, Vec::new()));

        Ok(Self {
            request_id,
            idempotency_key,
            actor_type,
            actor_id,
            actor_email,
            scopes,
        })
    }
}
