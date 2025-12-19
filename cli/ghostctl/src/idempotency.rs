//! Idempotency key helpers.
//!
//! The API supports `Idempotency-Key` for write endpoints. The CLI generates
//! deterministic keys by default so retrying the same command is safe.

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";

pub fn default_idempotency_key(
    endpoint_name: &str,
    request_scope: &str,
    body: &impl Serialize,
) -> Result<String> {
    // Canonicalize JSON so map key ordering doesn't affect the derived key.
    let json_value = serde_json::to_value(body).context("failed to serialize request body")?;
    let body_json = serde_json::to_vec(&json_value).context("failed to serialize request body")?;

    let mut hasher = Sha256::new();
    hasher.update(endpoint_name.as_bytes());
    hasher.update(b"\n");
    hasher.update(request_scope.as_bytes());
    hasher.update(b"\n");
    hasher.update(&body_json);

    Ok(format!("vt_{:x}", hasher.finalize()))
}

pub fn default_idempotency_key_no_body(endpoint_name: &str, request_scope: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(endpoint_name.as_bytes());
    hasher.update(b"\n");
    hasher.update(request_scope.as_bytes());

    format!("vt_{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_key_is_stable_and_within_limits() {
        #[derive(Serialize)]
        struct Req {
            name: String,
        }

        let req = Req {
            name: "hello".to_string(),
        };

        let a = default_idempotency_key("apps.create", "/v1/orgs/org_123/apps", &req).unwrap();
        let b = default_idempotency_key("apps.create", "/v1/orgs/org_123/apps", &req).unwrap();
        assert_eq!(a, b);
        assert!(a.len() >= 8 && a.len() <= 128);
    }
}
