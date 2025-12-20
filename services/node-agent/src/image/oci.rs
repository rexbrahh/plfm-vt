//! OCI registry client for pulling images by digest.
//!
//! This module implements the OCI Distribution Specification for pulling
//! manifests and blobs from container registries.
//!
//! Reference: https://github.com/opencontainers/distribution-spec

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, info};

/// Errors from OCI operations.
#[derive(Debug, Error)]
pub enum OciError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    #[error("Image not found: {0}")]
    NotFound(String),

    #[error("Authentication required")]
    AuthRequired,

    #[error("Image too large: {size} bytes exceeds limit of {limit} bytes")]
    TooLarge { size: u64, limit: u64 },

    #[error("Pull timeout")]
    Timeout,
}

/// Configuration for OCI client.
#[derive(Debug, Clone)]
pub struct OciConfig {
    /// Registry URL (e.g., "https://registry-1.docker.io").
    pub registry_url: String,
    /// Optional auth token.
    pub auth_token: Option<String>,
    /// Per-layer pull timeout.
    pub layer_timeout: Duration,
    /// Total pull timeout.
    pub total_timeout: Duration,
    /// Max compressed image size.
    pub max_compressed_size: u64,
    /// Directory to store blobs.
    pub blob_dir: PathBuf,
}

impl Default for OciConfig {
    fn default() -> Self {
        Self {
            registry_url: "https://registry-1.docker.io".to_string(),
            auth_token: None,
            layer_timeout: Duration::from_secs(300), // 5 minutes
            total_timeout: Duration::from_secs(1800), // 30 minutes
            max_compressed_size: 10 * 1024 * 1024 * 1024, // 10 GiB
            blob_dir: PathBuf::from("/var/lib/plfm-agent/oci/blobs"),
        }
    }
}

/// OCI Distribution client.
pub struct OciClient {
    config: OciConfig,
    client: Client,
}

impl OciClient {
    /// Create a new OCI client.
    pub fn new(config: OciConfig) -> Result<Self, OciError> {
        let client = Client::builder().timeout(config.total_timeout).build()?;

        Ok(Self { config, client })
    }

    /// Pull an image manifest by digest.
    pub async fn pull_manifest(&self, repo: &str, digest: &str) -> Result<Manifest, OciError> {
        let url = format!(
            "{}/v2/{}/manifests/{}",
            self.config.registry_url, repo, digest
        );

        debug!(url = %url, "Pulling manifest");

        let mut request = self.client.get(&url).header(
            "Accept",
            "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json",
        );

        if let Some(token) = &self.config.auth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request.send().await?;

        match response.status() {
            StatusCode::OK => {
                let body = response.bytes().await?;

                // Verify digest
                let computed = format!("sha256:{}", hex::encode(Sha256::digest(&body)));
                if computed != digest {
                    return Err(OciError::DigestMismatch {
                        expected: digest.to_string(),
                        actual: computed,
                    });
                }

                let manifest: Manifest = serde_json::from_slice(&body)?;
                Ok(manifest)
            }
            StatusCode::NOT_FOUND => Err(OciError::NotFound(digest.to_string())),
            StatusCode::UNAUTHORIZED => Err(OciError::AuthRequired),
            _status => Err(OciError::Http(response.error_for_status().unwrap_err())),
        }
    }

    /// Pull a blob by digest to a file.
    pub async fn pull_blob(&self, repo: &str, digest: &str, dest: &Path) -> Result<u64, OciError> {
        let url = format!("{}/v2/{}/blobs/{}", self.config.registry_url, repo, digest);

        debug!(url = %url, dest = %dest.display(), "Pulling blob");

        let mut request = self.client.get(&url);

        if let Some(token) = &self.config.auth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = tokio::time::timeout(self.config.layer_timeout, request.send())
            .await
            .map_err(|_| OciError::Timeout)??;

        match response.status() {
            StatusCode::OK => {
                // Check content length
                if let Some(size) = response.content_length() {
                    if size > self.config.max_compressed_size {
                        return Err(OciError::TooLarge {
                            size,
                            limit: self.config.max_compressed_size,
                        });
                    }
                }

                // Create parent directory
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Download to temporary file, then rename
                let temp_path = dest.with_extension("tmp");
                let mut file = std::fs::File::create(&temp_path)?;
                let mut hasher = Sha256::new();

                // Read the whole response (streaming would be better for large files)
                let bytes = response.bytes().await?;
                let total_bytes = bytes.len() as u64;
                hasher.update(&bytes);
                file.write_all(&bytes)?;
                file.sync_all()?;
                drop(file);

                // Verify digest
                let computed = format!("sha256:{}", hex::encode(hasher.finalize()));
                if computed != digest {
                    std::fs::remove_file(&temp_path).ok();
                    return Err(OciError::DigestMismatch {
                        expected: digest.to_string(),
                        actual: computed,
                    });
                }

                // Rename to final location
                std::fs::rename(&temp_path, dest)?;

                info!(
                    digest = %digest,
                    size = total_bytes,
                    "Blob downloaded"
                );

                Ok(total_bytes)
            }
            StatusCode::NOT_FOUND => Err(OciError::NotFound(digest.to_string())),
            StatusCode::UNAUTHORIZED => Err(OciError::AuthRequired),
            _ => Err(OciError::Http(response.error_for_status().unwrap_err())),
        }
    }

    /// Get the local path for a blob.
    pub fn blob_path(&self, digest: &str) -> PathBuf {
        // digest format: "sha256:abc123..."
        let parts: Vec<&str> = digest.split(':').collect();
        if parts.len() == 2 {
            self.config.blob_dir.join(parts[0]).join(parts[1])
        } else {
            self.config.blob_dir.join(digest)
        }
    }

    /// Check if a blob exists locally.
    pub fn blob_exists(&self, digest: &str) -> bool {
        self.blob_path(digest).exists()
    }
}

/// OCI image manifest.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Schema version.
    pub schema_version: u32,
    /// Media type.
    #[serde(default)]
    pub media_type: Option<String>,
    /// Config descriptor.
    pub config: Descriptor,
    /// Layer descriptors.
    pub layers: Vec<Descriptor>,
}

/// Content descriptor.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    /// Media type of the referenced content.
    pub media_type: String,
    /// Digest of the content.
    pub digest: String,
    /// Size in bytes.
    pub size: u64,
}

impl Manifest {
    /// Get total compressed size of all layers.
    pub fn total_layer_size(&self) -> u64 {
        self.layers.iter().map(|l| l.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_path() {
        let config = OciConfig {
            blob_dir: PathBuf::from("/var/lib/test/blobs"),
            ..Default::default()
        };
        let client = OciClient::new(config).unwrap();

        let path = client.blob_path("sha256:abc123");
        assert_eq!(path, PathBuf::from("/var/lib/test/blobs/sha256/abc123"));
    }

    #[test]
    fn test_manifest_total_size() {
        let manifest = Manifest {
            schema_version: 2,
            media_type: None,
            config: Descriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_string(),
                digest: "sha256:config".to_string(),
                size: 1000,
            },
            layers: vec![
                Descriptor {
                    media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                    digest: "sha256:layer1".to_string(),
                    size: 5000,
                },
                Descriptor {
                    media_type: "application/vnd.oci.image.layer.v1.tar+gzip".to_string(),
                    digest: "sha256:layer2".to_string(),
                    size: 3000,
                },
            ],
        };

        assert_eq!(manifest.total_layer_size(), 8000);
    }
}
