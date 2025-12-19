//! Secrets materialization.
//!
//! Writes secrets to a file with atomic writes and correct permissions.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::Result;
use nix::unistd::{chown, Gid, Uid};
use tracing::info;

use crate::config::SecretsConfig;
use crate::error::InitError;

/// Materialize secrets to the configured path.
pub async fn materialize(config: &SecretsConfig) -> Result<()> {
    let data = match &config.data {
        Some(data) => data.clone(),
        None => {
            if config.required {
                return Err(InitError::SecretsMissing(
                    "secrets.required is true but no data provided".to_string(),
                )
                .into());
            }
            // No secrets to write
            return Ok(());
        }
    };

    let path = Path::new(&config.path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            InitError::SecretsWriteFailed(format!("failed to create directory: {}", e))
        })?;
    }

    // Parse permissions mode (octal string like "0400")
    let mode = parse_mode(&config.mode)?;

    // Write atomically
    let tmp_path = path.with_extension("tmp");
    write_with_permissions(&tmp_path, &data, mode)?;

    // Set ownership before rename
    let uid = Uid::from_raw(config.owner_uid);
    let gid = Gid::from_raw(config.owner_gid);
    chown(&tmp_path, Some(uid), Some(gid)).map_err(|e| {
        InitError::SecretsWriteFailed(format!("chown failed: {}", e))
    })?;

    // Sync to disk
    {
        let file = File::open(&tmp_path)?;
        file.sync_all()?;
    }

    // Rename to final path
    fs::rename(&tmp_path, path).map_err(|e| {
        InitError::SecretsWriteFailed(format!("rename failed: {}", e))
    })?;

    info!(
        path = %config.path,
        mode = %config.mode,
        uid = config.owner_uid,
        gid = config.owner_gid,
        "secrets materialized"
    );

    Ok(())
}

/// Parse octal mode string (e.g., "0400") to u32.
fn parse_mode(mode_str: &str) -> Result<u32> {
    let mode_str = mode_str.trim_start_matches('0');
    u32::from_str_radix(mode_str, 8).map_err(|e| {
        InitError::SecretsWriteFailed(format!("invalid mode '{}': {}", mode_str, e)).into()
    })
}

/// Write data to file with specific permissions.
fn write_with_permissions(path: &Path, data: &str, mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| InitError::SecretsWriteFailed(format!("open failed: {}", e)))?;

    file.write_all(data.as_bytes())
        .map_err(|e| InitError::SecretsWriteFailed(format!("write failed: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn test_parse_mode() {
        assert_eq!(parse_mode("0400").unwrap(), 0o400);
        assert_eq!(parse_mode("0644").unwrap(), 0o644);
        assert_eq!(parse_mode("400").unwrap(), 0o400);
    }

    #[tokio::test]
    async fn test_materialize_secrets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secrets").join("platform.env");

        let config = SecretsConfig {
            required: true,
            path: path.to_string_lossy().to_string(),
            mode: "0400".to_string(),
            owner_uid: unsafe { libc::getuid() },
            owner_gid: unsafe { libc::getgid() },
            format: "dotenv".to_string(),
            bundle_version_id: None,
            data: Some("API_KEY=secret123\nDB_URL=postgres://...".to_string()),
        };

        materialize(&config).await.unwrap();

        // Check file exists and has correct content
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("API_KEY=secret123"));

        // Check permissions
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o400);
    }

    #[tokio::test]
    async fn test_missing_required_secrets() {
        let config = SecretsConfig {
            required: true,
            path: "/tmp/test-secrets.env".to_string(),
            mode: "0400".to_string(),
            owner_uid: 0,
            owner_gid: 0,
            format: "dotenv".to_string(),
            bundle_version_id: None,
            data: None, // No data!
        };

        let result = materialize(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("secrets_missing"));
    }
}
