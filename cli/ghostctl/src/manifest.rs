//! Manifest parsing and hashing helpers.
//!
//! v1 contract: releases pin an OCI image digest plus a manifest content hash.
//! We compute the hash from a canonicalized representation of the TOML.

use anyhow::{Context, Result};
use jsonschema::Draft;
use sha2::{Digest, Sha256};

const MANIFEST_SCHEMA_V1_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../api/schemas/manifest.json"
));

#[derive(Debug, Clone)]
pub struct ManifestValidationError {
    pub instance_path: String,
    pub schema_path: String,
}

pub fn manifest_json_from_toml_str(contents: &str) -> Result<serde_json::Value> {
    let value: toml::Value = toml::from_str(contents).context("invalid manifest TOML")?;
    if !value.is_table() {
        anyhow::bail!("manifest must be a TOML table (key/value pairs at top-level)");
    }

    serde_json::to_value(&value).context("failed to convert manifest TOML to JSON")
}

pub fn manifest_hash_from_toml_str(contents: &str) -> Result<String> {
    let json_value = manifest_json_from_toml_str(contents)?;
    let canonical_json =
        serde_json::to_vec(&json_value).context("failed to serialize manifest for hashing")?;

    let mut hasher = Sha256::new();
    hasher.update(&canonical_json);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn validate_manifest_toml_str(contents: &str) -> Result<Vec<ManifestValidationError>> {
    let schema: serde_json::Value = serde_json::from_str(MANIFEST_SCHEMA_V1_JSON)
        .context("failed to parse embedded manifest schema")?;
    let compiled = jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .map_err(|e| anyhow::anyhow!("failed to compile embedded manifest schema: {e}"))?;

    let instance = manifest_json_from_toml_str(contents)?;

    if compiled.is_valid(&instance) {
        return Ok(Vec::new());
    };

    let mut out: Vec<ManifestValidationError> = compiled
        .iter_errors(&instance)
        .map(|e| ManifestValidationError {
            instance_path: e.instance_path().to_string(),
            schema_path: e.schema_path().to_string(),
        })
        .collect();

    out.sort_by(|a, b| {
        (a.instance_path.as_str(), a.schema_path.as_str())
            .cmp(&(b.instance_path.as_str(), b.schema_path.as_str()))
    });

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_hash_is_deterministic_across_formatting() {
        let a = r#"
schema_version = "v1"

[app]
name = "hello"

[image]
ref = "ghcr.io/acme/hello@sha256:deadbeef"

[env]
workdir = "/app"
"#;

        let b = r#"
schema_version="v1"
[env]
workdir="/app"
[image]
ref="ghcr.io/acme/hello@sha256:deadbeef"
[app]
name="hello"
"#;

        let ha = manifest_hash_from_toml_str(a).unwrap();
        let hb = manifest_hash_from_toml_str(b).unwrap();
        assert_eq!(ha, hb);
        assert!(ha.starts_with("sha256:"));
        assert_eq!(ha.len(), "sha256:".len() + 64);
    }

    #[test]
    fn manifest_validation_accepts_minimal_valid_manifest() {
        let manifest = r#"
schema_version = "v1"

[processes.web]
command = ["sh", "-lc", "echo ok"]

[processes.web.resources]
memory = "256Mi"
"#;

        let errors = validate_manifest_toml_str(manifest).unwrap();
        assert!(errors.is_empty());
    }
}
