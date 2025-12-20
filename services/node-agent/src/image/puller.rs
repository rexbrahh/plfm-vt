//! High-level image puller that orchestrates OCI pull, caching, and root disk building.
//!
//! This module provides the main API for ensuring images are available locally
//! as bootable ext4 root disks for Firecracker VMs.
//!
//! Reference: docs/specs/runtime/image-fetch-and-cache.md

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info};

use super::cache::ImageCache;
use super::oci::{OciClient, OciConfig, OciError};
use super::rootdisk::{RootDiskBuilder, RootDiskConfig, RootDiskError};

/// Errors from image pulling operations.
#[derive(Debug, Error)]
pub enum ImagePullError {
    #[error("OCI error: {0}")]
    Oci(#[from] OciError),

    #[error("Root disk error: {0}")]
    RootDisk(#[from] RootDiskError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Image too large: compressed size {size} bytes exceeds limit {limit} bytes")]
    ImageTooLarge { size: u64, limit: u64 },

    #[error("Image pull timeout")]
    Timeout,

    #[error("Invalid image reference: {0}")]
    InvalidImageRef(String),

    #[error("Build lock acquisition failed")]
    LockFailed,
}

/// Result of a successful image pull.
#[derive(Debug, Clone)]
pub struct PullResult {
    /// The digest of the pulled image.
    pub digest: String,

    /// Path to the root disk ext4 image.
    pub root_disk_path: PathBuf,

    /// Size of the root disk in bytes.
    pub root_disk_size: u64,

    /// Whether the image was already cached (cache hit).
    pub was_cached: bool,

    /// Time taken to pull and build (if not cached).
    pub pull_duration_ms: Option<u64>,
}

/// Configuration for the image puller.
#[derive(Debug, Clone)]
pub struct ImagePullerConfig {
    /// OCI client configuration.
    pub oci: OciConfig,

    /// Root disk builder configuration.
    pub rootdisk: RootDiskConfig,

    /// Maximum concurrent builds per puller.
    pub max_concurrent_builds: usize,
}

impl Default for ImagePullerConfig {
    fn default() -> Self {
        Self {
            oci: OciConfig::default(),
            rootdisk: RootDiskConfig::default(),
            max_concurrent_builds: 4,
        }
    }
}

/// High-level image puller that coordinates OCI pulls and root disk building.
///
/// This is the main entry point for ensuring images are available locally.
/// It handles:
/// - Checking cache for existing root disks
/// - Pulling OCI manifests and layers
/// - Building ext4 root disks
/// - Deduplicating concurrent pulls for the same digest
pub struct ImagePuller {
    rootdisk_builder: RootDiskBuilder,
    cache: Arc<ImageCache>,
    /// Per-digest build locks to prevent concurrent builds of the same image.
    build_locks: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>>>,
    config: ImagePullerConfig,
}

impl ImagePuller {
    /// Create a new image puller.
    pub fn new(config: ImagePullerConfig, cache: Arc<ImageCache>) -> Result<Self, ImagePullError> {
        let rootdisk_builder = RootDiskBuilder::new(config.rootdisk.clone());

        Ok(Self {
            rootdisk_builder,
            cache,
            build_locks: Arc::new(Mutex::new(std::collections::HashMap::new())),
            config,
        })
    }

    /// Ensure an image is pulled and available as a root disk.
    ///
    /// This method is idempotent - if the root disk already exists, it returns immediately.
    /// Concurrent calls for the same digest will wait for the first build to complete.
    ///
    /// # Arguments
    /// * `image_ref` - Human-readable image reference (for logging only)
    /// * `registry` - Registry hostname (e.g., "registry-1.docker.io")
    /// * `repo` - Repository name (e.g., "library/alpine")
    /// * `digest` - Content-addressable digest (e.g., "sha256:abc123...")
    ///
    /// # Returns
    /// Path to the root disk and metadata about the pull operation.
    pub async fn ensure_image(
        &self,
        image_ref: &str,
        registry: &str,
        repo: &str,
        digest: &str,
    ) -> Result<PullResult, ImagePullError> {
        let start = Instant::now();

        // Fast path: check if root disk already exists in cache
        if let Some(path) = self.cache.acquire_rootdisk(digest).await {
            debug!(
                digest = %digest,
                image_ref = %image_ref,
                "Root disk cache hit"
            );

            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            return Ok(PullResult {
                digest: digest.to_string(),
                root_disk_path: path,
                root_disk_size: size,
                was_cached: true,
                pull_duration_ms: None,
            });
        }

        // Also check if the root disk file exists on disk (cache may not be initialized)
        if self.rootdisk_builder.rootdisk_exists(digest) {
            let path = self.rootdisk_builder.rootdisk_path(digest);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            // Register in cache for future lookups
            self.cache
                .register_rootdisk(digest, path.clone(), size)
                .await;
            self.cache.acquire_rootdisk(digest).await;

            debug!(
                digest = %digest,
                image_ref = %image_ref,
                "Root disk exists on disk, registered in cache"
            );

            return Ok(PullResult {
                digest: digest.to_string(),
                root_disk_path: path,
                root_disk_size: size,
                was_cached: true,
                pull_duration_ms: None,
            });
        }

        // Acquire per-digest build lock to prevent concurrent builds
        let build_lock = self.get_build_lock(digest).await;
        let _guard = build_lock.lock().await;

        // Double-check after acquiring lock (another task may have completed the build)
        if self.rootdisk_builder.rootdisk_exists(digest) {
            let path = self.rootdisk_builder.rootdisk_path(digest);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            self.cache
                .register_rootdisk(digest, path.clone(), size)
                .await;
            self.cache.acquire_rootdisk(digest).await;

            return Ok(PullResult {
                digest: digest.to_string(),
                root_disk_path: path,
                root_disk_size: size,
                was_cached: true,
                pull_duration_ms: Some(start.elapsed().as_millis() as u64),
            });
        }

        // Actually pull and build
        info!(
            digest = %digest,
            image_ref = %image_ref,
            repo = %repo,
            "Pulling image and building root disk"
        );

        let result = self.pull_and_build(registry, repo, digest).await?;

        let duration = start.elapsed();
        info!(
            digest = %digest,
            image_ref = %image_ref,
            duration_ms = duration.as_millis(),
            size_bytes = result.root_disk_size,
            "Image pull and build completed"
        );

        // Register in cache
        self.cache
            .register_rootdisk(digest, result.root_disk_path.clone(), result.root_disk_size)
            .await;
        self.cache.acquire_rootdisk(digest).await;

        Ok(PullResult {
            digest: result.digest,
            root_disk_path: result.root_disk_path,
            root_disk_size: result.root_disk_size,
            was_cached: false,
            pull_duration_ms: Some(duration.as_millis() as u64),
        })
    }

    /// Release a reference to an image's root disk.
    ///
    /// This should be called when an instance using this image is stopped.
    pub async fn release_image(&self, digest: &str) {
        self.cache.release_rootdisk(digest).await;
    }

    /// Pull manifest and layers, then build root disk.
    async fn pull_and_build(
        &self,
        registry: &str,
        repo: &str,
        digest: &str,
    ) -> Result<PullResult, ImagePullError> {
        let oci_client = self.oci_client_for_registry(registry)?;
        // 1. Pull manifest
        let manifest = oci_client.pull_manifest(repo, digest).await?;

        // 2. Check total size before pulling
        let total_compressed = manifest.total_layer_size();
        if total_compressed > self.config.oci.max_compressed_size {
            return Err(ImagePullError::ImageTooLarge {
                size: total_compressed,
                limit: self.config.oci.max_compressed_size,
            });
        }

        info!(
            digest = %digest,
            layer_count = manifest.layers.len(),
            total_compressed_bytes = total_compressed,
            "Manifest fetched, pulling layers"
        );

        // 3. Pull all layers
        let mut layer_paths = Vec::with_capacity(manifest.layers.len());
        for (i, layer) in manifest.layers.iter().enumerate() {
            let layer_path = oci_client.blob_path(&layer.digest);

            // Skip if already cached
            if oci_client.blob_exists(&layer.digest) {
                debug!(
                    layer = i,
                    digest = %layer.digest,
                    "Layer already cached"
                );
                layer_paths.push(layer_path);
                continue;
            }

            debug!(
                layer = i,
                digest = %layer.digest,
                size = layer.size,
                "Pulling layer"
            );

            oci_client
                .pull_blob(repo, &layer.digest, &layer_path)
                .await?;

            layer_paths.push(layer_path);
        }

        // 4. Build root disk
        debug!(
            digest = %digest,
            layer_count = layer_paths.len(),
            "Building root disk from layers"
        );

        let rootdisk_path = self.rootdisk_builder.build(digest, &layer_paths)?;

        let size = std::fs::metadata(&rootdisk_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok(PullResult {
            digest: digest.to_string(),
            root_disk_path: rootdisk_path,
            root_disk_size: size,
            was_cached: false,
            pull_duration_ms: None,
        })
    }

    fn oci_client_for_registry(&self, registry: &str) -> Result<OciClient, ImagePullError> {
        let mut config = self.config.oci.clone();
        let registry_url = if registry.starts_with("http://") || registry.starts_with("https://") {
            registry.to_string()
        } else {
            format!("https://{registry}")
        };
        config.registry_url = registry_url;
        Ok(OciClient::new(config)?)
    }

    /// Get or create a build lock for a digest.
    async fn get_build_lock(&self, digest: &str) -> Arc<Mutex<()>> {
        let mut locks = self.build_locks.lock().await;
        locks
            .entry(digest.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Check if eviction is needed and run it.
    pub async fn maybe_evict(&self) -> std::io::Result<u64> {
        if self.cache.needs_eviction() {
            self.cache.evict().await
        } else {
            Ok(0)
        }
    }
}

/// Parse an image reference into registry, repo, and tag/digest components.
///
/// Examples:
/// - `alpine:latest` -> (docker.io, library/alpine, latest)
/// - `ghcr.io/org/repo:v1` -> (ghcr.io, org/repo, v1)
/// - `registry.example.com/foo/bar@sha256:abc...` -> (registry.example.com, foo/bar, sha256:abc...)
pub fn parse_image_ref(image_ref: &str) -> Result<(String, String, String), ImagePullError> {
    // Handle digest reference
    let (name_part, reference) = if let Some((name, digest)) = image_ref.rsplit_once('@') {
        (name, digest.to_string())
    } else if let Some((name, tag)) = image_ref.rsplit_once(':') {
        // Make sure this isn't a port number
        if tag.contains('/') || name.ends_with(']') {
            // It's a port, not a tag
            (image_ref, "latest".to_string())
        } else {
            (name, tag.to_string())
        }
    } else {
        (image_ref, "latest".to_string())
    };

    // Parse registry and repo
    let parts: Vec<&str> = name_part.splitn(2, '/').collect();
    let (registry, repo) = if parts.len() == 1 {
        // No slash - Docker Hub library image
        (
            "registry-1.docker.io".to_string(),
            format!("library/{}", parts[0]),
        )
    } else if parts[0].contains('.') || parts[0].contains(':') || parts[0] == "localhost" {
        // First part looks like a registry
        (parts[0].to_string(), parts[1].to_string())
    } else {
        // Docker Hub user image
        ("registry-1.docker.io".to_string(), name_part.to_string())
    };

    Ok((registry, repo, reference))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_ref_simple() {
        let (registry, repo, tag) = parse_image_ref("alpine:latest").unwrap();
        assert_eq!(registry, "registry-1.docker.io");
        assert_eq!(repo, "library/alpine");
        assert_eq!(tag, "latest");
    }

    #[test]
    fn test_parse_image_ref_no_tag() {
        let (registry, repo, tag) = parse_image_ref("alpine").unwrap();
        assert_eq!(registry, "registry-1.docker.io");
        assert_eq!(repo, "library/alpine");
        assert_eq!(tag, "latest");
    }

    #[test]
    fn test_parse_image_ref_user_repo() {
        let (registry, repo, tag) = parse_image_ref("myuser/myapp:v1").unwrap();
        assert_eq!(registry, "registry-1.docker.io");
        assert_eq!(repo, "myuser/myapp");
        assert_eq!(tag, "v1");
    }

    #[test]
    fn test_parse_image_ref_custom_registry() {
        let (registry, repo, tag) = parse_image_ref("ghcr.io/org/repo:v2").unwrap();
        assert_eq!(registry, "ghcr.io");
        assert_eq!(repo, "org/repo");
        assert_eq!(tag, "v2");
    }

    #[test]
    fn test_parse_image_ref_digest() {
        let (registry, repo, tag) = parse_image_ref("alpine@sha256:abc123").unwrap();
        assert_eq!(registry, "registry-1.docker.io");
        assert_eq!(repo, "library/alpine");
        assert_eq!(tag, "sha256:abc123");
    }

    #[test]
    fn test_parse_image_ref_full_digest() {
        let (registry, repo, digest) =
            parse_image_ref("ghcr.io/org/app@sha256:abcdef1234567890").unwrap();
        assert_eq!(registry, "ghcr.io");
        assert_eq!(repo, "org/app");
        assert_eq!(digest, "sha256:abcdef1234567890");
    }

    #[test]
    fn test_parse_image_ref_localhost() {
        let (registry, repo, tag) = parse_image_ref("localhost:5000/myapp:test").unwrap();
        assert_eq!(registry, "localhost:5000");
        assert_eq!(repo, "myapp");
        assert_eq!(tag, "test");
    }
}
