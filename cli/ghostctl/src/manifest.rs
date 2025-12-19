//! Manifest parsing and hashing helpers.
//!
//! v1 contract: releases pin an OCI image digest plus a manifest content hash.
//! We compute the hash from a canonicalized representation of the TOML.

use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn manifest_hash_from_toml_str(contents: &str) -> Result<String> {
    let value: toml::Value = toml::from_str(contents).context("invalid manifest TOML")?;
    if !value.is_table() {
        anyhow::bail!("manifest must be a TOML table (key/value pairs at top-level)");
    }

    let json_value = serde_json::to_value(&value).context("failed to canonicalize manifest")?;
    let canonical_json =
        serde_json::to_vec(&json_value).context("failed to serialize manifest for hashing")?;

    let mut hasher = Sha256::new();
    hasher.update(&canonical_json);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub fn manifest_hash_from_path(path: &Path) -> Result<String> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest: {}", path.display()))?;
    manifest_hash_from_toml_str(&contents)
        .with_context(|| format!("failed to compute manifest hash: {}", path.display()))
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
}
