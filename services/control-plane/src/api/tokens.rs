//! Token generation, hashing, and validation utilities.
//!
//! This module provides:
//! - Token generation with type-specific prefixes
//! - Secure token hashing for storage
//! - Token validation against the database
//!
//! Token format:
//! - Access token: `trc_at_<32 random bytes base64>`
//! - Refresh token: `trc_rt_<32 random bytes base64>`
//! - Device code: `trc_dc_<32 random bytes base64>`
//!
//! All tokens are stored hashed (SHA-256) in the database.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::api::error::ApiError;

/// Token type prefixes per spec.
pub const ACCESS_TOKEN_PREFIX: &str = "trc_at_";
pub const REFRESH_TOKEN_PREFIX: &str = "trc_rt_";
pub const DEVICE_CODE_PREFIX: &str = "trc_dc_";

/// Default token lifetimes per spec.
pub const ACCESS_TOKEN_LIFETIME_MINUTES: i64 = 15;
pub const REFRESH_TOKEN_LIFETIME_DAYS: i64 = 30;
pub const DEVICE_CODE_LIFETIME_MINUTES: i64 = 10;

/// Minimum poll interval for device flow (seconds).
pub const DEVICE_POLL_INTERVAL_SECONDS: u32 = 5;

/// Token bytes (32 bytes = 256 bits of entropy).
const TOKEN_BYTES: usize = 32;

/// Generate a new access token.
pub fn generate_access_token() -> String {
    generate_token_with_prefix(ACCESS_TOKEN_PREFIX)
}

/// Generate a new refresh token.
pub fn generate_refresh_token() -> String {
    generate_token_with_prefix(REFRESH_TOKEN_PREFIX)
}

/// Generate a new device code.
pub fn generate_device_code() -> String {
    generate_token_with_prefix(DEVICE_CODE_PREFIX)
}

/// Generate a user-friendly user code for device flow (e.g., "ABCD-1234").
/// Format: 4 uppercase letters + hyphen + 4 digits = 9 characters.
pub fn generate_user_code() -> String {
    let mut rng = rand::rng();

    let letters: String = (0..4)
        .map(|_| {
            // A-Z, excluding confusing chars I, L, O
            let chars = b"ABCDEFGHJKMNPQRSTUVWXYZ";
            chars[rng.random_range(0..chars.len())] as char
        })
        .collect();

    let digits: String = (0..4)
        .map(|_| {
            // 0-9, excluding confusing 0, 1
            let chars = b"23456789";
            chars[rng.random_range(0..chars.len())] as char
        })
        .collect();

    format!("{}-{}", letters, digits)
}

/// Generate a token with the given prefix.
fn generate_token_with_prefix(prefix: &str) -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill(&mut bytes);
    let encoded = URL_SAFE_NO_PAD.encode(bytes);
    format!("{}{}", prefix, encoded)
}

/// Hash a token for storage using SHA-256.
/// The hash is returned as a hex string.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("{:x}", digest)
}

/// Token subject type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectType {
    User,
    ServicePrincipal,
}

impl SubjectType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubjectType::User => "user",
            SubjectType::ServicePrincipal => "service_principal",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(SubjectType::User),
            "service_principal" => Some(SubjectType::ServicePrincipal),
            _ => None,
        }
    }
}

/// Validated access token info.
#[derive(Debug, Clone)]
pub struct ValidatedAccessToken {
    pub token_id: String,
    pub subject_type: SubjectType,
    pub subject_id: String,
    pub subject_email: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

/// Validated refresh token info.
#[derive(Debug, Clone)]
pub struct ValidatedRefreshToken {
    pub token_id: String,
    pub subject_type: SubjectType,
    pub subject_id: String,
    pub subject_email: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

/// Look up and validate an access token.
///
/// Returns the token info if valid, or an error if:
/// - Token not found
/// - Token expired
/// - Token revoked
pub async fn validate_access_token(
    pool: &PgPool,
    token: &str,
    request_id: &str,
) -> Result<ValidatedAccessToken, ApiError> {
    // Must have correct prefix
    if !token.starts_with(ACCESS_TOKEN_PREFIX) {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid token format")
                .with_request_id(request_id.to_string()),
        );
    }

    let token_hash = hash_token(token);

    let row = sqlx::query_as::<_, AccessTokenRow>(
        r#"
        SELECT token_id, subject_type, subject_id, subject_email, scopes, expires_at, revoked_at
        FROM access_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to query access token");
        ApiError::internal("internal_error", "Failed to validate token")
            .with_request_id(request_id.to_string())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid or expired token")
                .with_request_id(request_id.to_string()),
        );
    };

    // Check if revoked
    if row.revoked_at.is_some() {
        return Err(
            ApiError::unauthorized("token_revoked", "Token has been revoked")
                .with_request_id(request_id.to_string()),
        );
    }

    // Check if expired
    if row.expires_at < Utc::now() {
        return Err(ApiError::unauthorized("token_expired", "Token has expired")
            .with_request_id(request_id.to_string()));
    }

    let subject_type = SubjectType::from_str(&row.subject_type).ok_or_else(|| {
        ApiError::internal("internal_error", "Invalid subject type in token")
            .with_request_id(request_id.to_string())
    })?;

    let scopes: Vec<String> = serde_json::from_value(row.scopes).unwrap_or_default();

    Ok(ValidatedAccessToken {
        token_id: row.token_id,
        subject_type,
        subject_id: row.subject_id,
        subject_email: row.subject_email,
        scopes,
        expires_at: row.expires_at,
    })
}

/// Look up and validate a refresh token.
pub async fn validate_refresh_token(
    pool: &PgPool,
    token: &str,
    request_id: &str,
) -> Result<ValidatedRefreshToken, ApiError> {
    // Must have correct prefix
    if !token.starts_with(REFRESH_TOKEN_PREFIX) {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid token format")
                .with_request_id(request_id.to_string()),
        );
    }

    let token_hash = hash_token(token);

    let row = sqlx::query_as::<_, RefreshTokenRow>(
        r#"
        SELECT token_id, subject_type, subject_id, subject_email, scopes, expires_at, revoked_at
        FROM refresh_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to query refresh token");
        ApiError::internal("internal_error", "Failed to validate token")
            .with_request_id(request_id.to_string())
    })?;

    let Some(row) = row else {
        return Err(
            ApiError::unauthorized("invalid_token", "Invalid or expired token")
                .with_request_id(request_id.to_string()),
        );
    };

    // Check if revoked
    if row.revoked_at.is_some() {
        return Err(
            ApiError::unauthorized("token_revoked", "Token has been revoked")
                .with_request_id(request_id.to_string()),
        );
    }

    // Check if expired
    if row.expires_at < Utc::now() {
        return Err(ApiError::unauthorized("token_expired", "Token has expired")
            .with_request_id(request_id.to_string()));
    }

    let subject_type = SubjectType::from_str(&row.subject_type).ok_or_else(|| {
        ApiError::internal("internal_error", "Invalid subject type in token")
            .with_request_id(request_id.to_string())
    })?;

    let scopes: Vec<String> = serde_json::from_value(row.scopes).unwrap_or_default();

    Ok(ValidatedRefreshToken {
        token_id: row.token_id,
        subject_type,
        subject_id: row.subject_id,
        subject_email: row.subject_email,
        scopes,
        expires_at: row.expires_at,
    })
}

/// Create a new access token in the database.
pub async fn create_access_token(
    pool: &PgPool,
    subject_type: SubjectType,
    subject_id: &str,
    subject_email: Option<&str>,
    scopes: &[String],
    refresh_token_id: Option<&str>,
    device_code_id: Option<&str>,
) -> Result<(String, DateTime<Utc>), sqlx::Error> {
    let token = generate_access_token();
    let token_hash = hash_token(&token);
    let token_id = format!("at_{}", plfm_id::RequestId::new());
    let expires_at = Utc::now() + Duration::minutes(ACCESS_TOKEN_LIFETIME_MINUTES);
    let scopes_json = serde_json::to_value(scopes).unwrap_or_default();

    sqlx::query(
        r#"
        INSERT INTO access_tokens (
            token_id, token_hash, subject_type, subject_id, subject_email,
            scopes, expires_at, refresh_token_id, device_code_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(&token_id)
    .bind(&token_hash)
    .bind(subject_type.as_str())
    .bind(subject_id)
    .bind(subject_email)
    .bind(&scopes_json)
    .bind(expires_at)
    .bind(refresh_token_id)
    .bind(device_code_id)
    .execute(pool)
    .await?;

    Ok((token, expires_at))
}

/// Create a new refresh token in the database.
pub async fn create_refresh_token(
    pool: &PgPool,
    subject_type: SubjectType,
    subject_id: &str,
    subject_email: Option<&str>,
    scopes: &[String],
    device_code_id: Option<&str>,
    previous_token_id: Option<&str>,
) -> Result<(String, String, DateTime<Utc>), sqlx::Error> {
    let token = generate_refresh_token();
    let token_hash = hash_token(&token);
    let token_id = format!("rt_{}", plfm_id::RequestId::new());
    let expires_at = Utc::now() + Duration::days(REFRESH_TOKEN_LIFETIME_DAYS);
    let scopes_json = serde_json::to_value(scopes).unwrap_or_default();

    sqlx::query(
        r#"
        INSERT INTO refresh_tokens (
            token_id, token_hash, subject_type, subject_id, subject_email,
            scopes, expires_at, device_code_id, previous_token_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(&token_id)
    .bind(&token_hash)
    .bind(subject_type.as_str())
    .bind(subject_id)
    .bind(subject_email)
    .bind(&scopes_json)
    .bind(expires_at)
    .bind(device_code_id)
    .bind(previous_token_id)
    .execute(pool)
    .await?;

    Ok((token, token_id, expires_at))
}

/// Revoke an access token.
pub async fn revoke_access_token(pool: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    let token_hash = hash_token(token);
    let result = sqlx::query(
        r#"
        UPDATE access_tokens
        SET revoked_at = now()
        WHERE token_hash = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(&token_hash)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Revoke a refresh token and all access tokens created from it.
pub async fn revoke_refresh_token(pool: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    let token_hash = hash_token(token);

    // First, get the refresh token ID
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT token_id
        FROM refresh_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;

    let Some((token_id,)) = row else {
        return Ok(false);
    };

    // Revoke all access tokens created from this refresh token
    sqlx::query(
        r#"
        UPDATE access_tokens
        SET revoked_at = now()
        WHERE refresh_token_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(&token_id)
    .execute(pool)
    .await?;

    // Revoke the refresh token itself
    let result = sqlx::query(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = now()
        WHERE token_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(&token_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

// Database row types

#[derive(Debug)]
struct AccessTokenRow {
    token_id: String,
    subject_type: String,
    subject_id: String,
    subject_email: Option<String>,
    scopes: serde_json::Value,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AccessTokenRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            token_id: row.try_get("token_id")?,
            subject_type: row.try_get("subject_type")?,
            subject_id: row.try_get("subject_id")?,
            subject_email: row.try_get("subject_email")?,
            scopes: row.try_get("scopes")?,
            expires_at: row.try_get("expires_at")?,
            revoked_at: row.try_get("revoked_at")?,
        })
    }
}

#[derive(Debug)]
struct RefreshTokenRow {
    token_id: String,
    subject_type: String,
    subject_id: String,
    subject_email: Option<String>,
    scopes: serde_json::Value,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for RefreshTokenRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            token_id: row.try_get("token_id")?,
            subject_type: row.try_get("subject_type")?,
            subject_id: row.try_get("subject_id")?,
            subject_email: row.try_get("subject_email")?,
            scopes: row.try_get("scopes")?,
            expires_at: row.try_get("expires_at")?,
            revoked_at: row.try_get("revoked_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_token_format() {
        let token = generate_access_token();
        assert!(token.starts_with(ACCESS_TOKEN_PREFIX));
        assert!(token.len() > ACCESS_TOKEN_PREFIX.len() + 40); // base64 of 32 bytes
    }

    #[test]
    fn test_refresh_token_format() {
        let token = generate_refresh_token();
        assert!(token.starts_with(REFRESH_TOKEN_PREFIX));
        assert!(token.len() > REFRESH_TOKEN_PREFIX.len() + 40);
    }

    #[test]
    fn test_device_code_format() {
        let code = generate_device_code();
        assert!(code.starts_with(DEVICE_CODE_PREFIX));
        assert!(code.len() > DEVICE_CODE_PREFIX.len() + 40);
    }

    #[test]
    fn test_user_code_format() {
        let code = generate_user_code();
        // Format: XXXX-XXXX
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");

        // All letters in first part
        for c in code[0..4].chars() {
            assert!(c.is_ascii_uppercase());
            // Should not contain confusing chars
            assert!(c != 'I' && c != 'L' && c != 'O');
        }

        // All digits in second part
        for c in code[5..9].chars() {
            assert!(c.is_ascii_digit());
            // Should not contain confusing chars
            assert!(c != '0' && c != '1');
        }
    }

    #[test]
    fn test_hash_token_deterministic() {
        let token = "test_token_123";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_token_different_for_different_tokens() {
        let hash1 = hash_token("token1");
        let hash2 = hash_token("token2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_tokens_are_unique() {
        let token1 = generate_access_token();
        let token2 = generate_access_token();
        assert_ne!(token1, token2);
    }
}
