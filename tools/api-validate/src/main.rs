use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn read_file_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn validate_yaml_file(path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let _: serde_yaml::Value = serde_yaml::from_str(&contents)
        .with_context(|| format!("invalid YAML: {}", path.display()))?;
    Ok(())
}

fn validate_json_file(path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("invalid JSON: {}", path.display()))?;
    if !value.is_object() {
        return Err(anyhow!(
            "expected JSON object at top-level: {}",
            path.display()
        ));
    }
    Ok(())
}

fn require_file(path: &Path) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(anyhow!("missing required file: {}", path.display()))
    }
}

fn main() -> Result<()> {
    let repo_root = std::env::current_dir().context("failed to determine current directory")?;

    let api_openapi = repo_root.join("api/openapi/openapi.yaml");
    let docs_openapi = repo_root.join("docs/specs/api/openapi.yaml");
    require_file(&api_openapi)?;
    require_file(&docs_openapi)?;

    // YAML parsing (syntax) validation
    validate_yaml_file(&api_openapi)?;
    validate_yaml_file(&docs_openapi)?;

    // Drift check: keep these copies identical to avoid “which one is authoritative?” confusion.
    let api_bytes = read_file_bytes(&api_openapi)?;
    let docs_bytes = read_file_bytes(&docs_openapi)?;
    if api_bytes != docs_bytes {
        return Err(anyhow!(
            "OpenAPI copies differ:\n  api:  {} (sha256={})\n  docs: {} (sha256={})",
            api_openapi.display(),
            sha256_hex(&api_bytes),
            docs_openapi.display(),
            sha256_hex(&docs_bytes),
        ));
    }

    // JSON schema syntax validation
    let schemas_dir = repo_root.join("api/schemas");
    require_file(&schemas_dir)?;

    let mut schema_files: Vec<PathBuf> = WalkDir::new(&schemas_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();

    schema_files.sort();
    if schema_files.is_empty() {
        return Err(anyhow!(
            "no JSON schema files found under {}",
            schemas_dir.display()
        ));
    }

    for path in &schema_files {
        validate_json_file(path)?;
    }

    println!(
        "OK: OpenAPI YAML parsed and copies match; {} JSON schemas parsed",
        schema_files.len()
    );
    Ok(())
}
