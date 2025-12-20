//! Image pull actor - manages image pulls with deduplication.
//!
//! Per `docs/specs/runtime/agent-actors.md`, the ImagePullActor:
//! - Ensures at-most-one concurrent pull per image digest per node
//! - Manages reference counting for garbage collection
//! - Handles disk pressure scenarios

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use super::framework::{Actor, ActorContext, ActorError};
use crate::image::{ImageCache, ImageCacheConfig, ImagePuller, ImagePullerConfig};

// =============================================================================
// Messages
// =============================================================================

/// Messages handled by ImagePullActor.
#[derive(Debug)]
pub enum ImageMessage {
    /// Ensure an image is pulled and available locally.
    EnsurePulled {
        image_ref: String,
        expected_digest: String,
        reply_to: oneshot::Sender<Result<ImagePullResult, String>>,
    },

    /// Release a reference to an image.
    ReleaseRef { instance_id: String, digest: String },

    /// Periodic garbage collection check.
    GCCheck { tick_id: u64 },
}

/// Result of a successful image pull.
#[derive(Debug, Clone)]
pub struct ImagePullResult {
    /// Image digest.
    pub digest: String,

    /// Path to the root disk.
    pub root_disk_path: String,

    /// Image size in bytes.
    pub size_bytes: u64,
}

// =============================================================================
// Actor State
// =============================================================================

/// State for a cached image.
#[derive(Debug, Clone)]
pub struct ImageCacheEntry {
    /// Image digest.
    pub digest: String,

    /// Path to the root disk.
    pub root_disk_path: String,

    /// Image size in bytes.
    pub size_bytes: u64,

    /// When the image was pulled (for cache age tracking).
    #[allow(dead_code)]
    pub pulled_at: Instant,

    /// When the image was last used.
    pub last_used_at: Instant,

    /// Instance IDs referencing this image.
    pub refs: HashSet<String>,
}

/// State of an in-progress pull.
struct PullInProgress {
    /// Waiters for this pull to complete.
    waiters: Vec<oneshot::Sender<Result<ImagePullResult, String>>>,

    /// When the pull started.
    started_at: Instant,
}

// =============================================================================
// Image Pull Actor
// =============================================================================

/// Actor managing image pulls with deduplication.
pub struct ImagePullActor {
    /// Cached images by digest.
    cache: HashMap<String, ImageCacheEntry>,

    /// In-progress pulls by digest.
    in_progress: HashMap<String, PullInProgress>,

    /// Base directory for image storage.
    image_dir: String,

    /// Maximum cache size in bytes.
    max_cache_bytes: u64,

    /// Current cache size in bytes.
    current_cache_bytes: u64,

    /// Image puller (optional - only set after initialization).
    puller: Option<Arc<ImagePuller>>,

    /// Shared image cache.
    image_cache: Option<Arc<ImageCache>>,
}

impl ImagePullActor {
    /// Create a new image pull actor.
    pub fn new(image_dir: String, max_cache_bytes: u64) -> Self {
        Self {
            cache: HashMap::new(),
            in_progress: HashMap::new(),
            image_dir,
            max_cache_bytes,
            current_cache_bytes: 0,
            puller: None,
            image_cache: None,
        }
    }

    /// Create a new image pull actor with a configured puller.
    pub fn with_puller(image_dir: String, max_cache_bytes: u64) -> Result<Self, String> {
        let cache_config = ImageCacheConfig {
            max_size_bytes: max_cache_bytes,
            rootdisk_dir: PathBuf::from(&image_dir).join("rootdisks"),
            ..Default::default()
        };
        let image_cache = Arc::new(ImageCache::new(cache_config));

        let puller_config = ImagePullerConfig {
            oci: crate::image::OciConfig {
                blob_dir: PathBuf::from(&image_dir).join("oci/blobs"),
                ..Default::default()
            },
            rootdisk: crate::image::RootDiskConfig {
                unpack_dir: PathBuf::from(&image_dir).join("unpacked"),
                rootdisk_dir: PathBuf::from(&image_dir).join("rootdisks"),
                tmp_dir: PathBuf::from(&image_dir).join("tmp"),
                ..Default::default()
            },
            ..Default::default()
        };

        let puller = ImagePuller::new(puller_config, image_cache.clone())
            .map_err(|e| format!("Failed to create image puller: {}", e))?;

        Ok(Self {
            cache: HashMap::new(),
            in_progress: HashMap::new(),
            image_dir,
            max_cache_bytes,
            current_cache_bytes: 0,
            puller: Some(Arc::new(puller)),
            image_cache: Some(image_cache),
        })
    }

    /// Get the number of cached images.
    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    /// Get the current cache size in bytes.
    pub fn cache_size_bytes(&self) -> u64 {
        self.current_cache_bytes
    }

    // -------------------------------------------------------------------------
    // Message Handlers
    // -------------------------------------------------------------------------

    async fn handle_ensure_pulled(
        &mut self,
        image_ref: String,
        expected_digest: String,
        reply_to: oneshot::Sender<Result<ImagePullResult, String>>,
    ) -> Result<(), ActorError> {
        // Check if already cached in our local cache
        if let Some(entry) = self.cache.get_mut(&expected_digest) {
            debug!(
                digest = %expected_digest,
                "Image already cached"
            );
            entry.last_used_at = Instant::now();

            let result = ImagePullResult {
                digest: entry.digest.clone(),
                root_disk_path: entry.root_disk_path.clone(),
                size_bytes: entry.size_bytes,
            };
            let _ = reply_to.send(Ok(result));
            return Ok(());
        }

        // Check if already pulling
        if let Some(pull) = self.in_progress.get_mut(&expected_digest) {
            debug!(
                digest = %expected_digest,
                waiters = pull.waiters.len(),
                "Image pull already in progress, adding waiter"
            );
            pull.waiters.push(reply_to);
            return Ok(());
        }

        // Start new pull
        info!(
            image_ref = %image_ref,
            digest = %expected_digest,
            "Starting image pull"
        );

        self.in_progress.insert(
            expected_digest.clone(),
            PullInProgress {
                waiters: vec![reply_to],
                started_at: Instant::now(),
            },
        );

        // If we have a puller, use it
        if let Some(puller) = &self.puller {
            // Parse image reference to get repo
            let (_registry, repo, _) = match crate::image::parse_image_ref(&image_ref) {
                Ok(parsed) => parsed,
                Err(e) => {
                    self.fail_pull(&expected_digest, format!("Invalid image ref: {}", e));
                    return Ok(());
                }
            };

            let puller = puller.clone();
            let digest = expected_digest.clone();
            let image_ref_clone = image_ref.clone();

            // Spawn the actual pull operation
            let pull_result = puller.ensure_image(&image_ref_clone, &repo, &digest).await;

            match pull_result {
                Ok(result) => {
                    self.complete_pull(
                        &digest,
                        result.root_disk_path.to_string_lossy().to_string(),
                        result.root_disk_size,
                    );
                }
                Err(e) => {
                    self.fail_pull(&digest, format!("Pull failed: {}", e));
                }
            }
        } else {
            // Fallback: simulate a successful pull (for testing without real OCI)
            warn!(
                digest = %expected_digest,
                "No puller configured, simulating successful pull"
            );
            let root_disk_path = format!(
                "{}/rootdisks/{}.ext4",
                self.image_dir,
                expected_digest.replace([':', '/'], "_")
            );
            let size_bytes = 512 * 1024 * 1024; // Fake 512MB

            self.complete_pull(&expected_digest, root_disk_path, size_bytes);
        }

        Ok(())
    }

    fn complete_pull(&mut self, digest: &str, root_disk_path: String, size_bytes: u64) {
        let now = Instant::now();

        // Add to cache
        let entry = ImageCacheEntry {
            digest: digest.to_string(),
            root_disk_path: root_disk_path.clone(),
            size_bytes,
            pulled_at: now,
            last_used_at: now,
            refs: HashSet::new(),
        };

        self.cache.insert(digest.to_string(), entry);
        self.current_cache_bytes += size_bytes;

        // Notify waiters
        if let Some(pull) = self.in_progress.remove(digest) {
            let duration = pull.started_at.elapsed();
            info!(
                digest = %digest,
                duration_ms = duration.as_millis(),
                size_bytes,
                waiters = pull.waiters.len(),
                "Image pull completed"
            );

            let result = ImagePullResult {
                digest: digest.to_string(),
                root_disk_path,
                size_bytes,
            };

            for waiter in pull.waiters {
                let _ = waiter.send(Ok(result.clone()));
            }
        }
    }

    fn fail_pull(&mut self, digest: &str, error: String) {
        if let Some(pull) = self.in_progress.remove(digest) {
            warn!(
                digest = %digest,
                error = %error,
                waiters = pull.waiters.len(),
                "Image pull failed"
            );

            for waiter in pull.waiters {
                let _ = waiter.send(Err(error.clone()));
            }
        }
    }

    fn handle_release_ref(&mut self, instance_id: String, digest: String) {
        if let Some(entry) = self.cache.get_mut(&digest) {
            entry.refs.remove(&instance_id);
            debug!(
                digest = %digest,
                instance_id = %instance_id,
                remaining_refs = entry.refs.len(),
                "Released image reference"
            );
        }
    }

    fn handle_gc_check(&mut self, _tick_id: u64) {
        // Check if we're over the cache limit
        if self.current_cache_bytes <= self.max_cache_bytes {
            return;
        }

        debug!(
            current_bytes = self.current_cache_bytes,
            max_bytes = self.max_cache_bytes,
            "Cache over limit, running GC"
        );

        // Find candidates for eviction (no refs, oldest first)
        let mut candidates: Vec<_> = self
            .cache
            .iter()
            .filter(|(_, e)| e.refs.is_empty())
            .map(|(d, e)| (d.clone(), e.last_used_at, e.size_bytes))
            .collect();

        // Sort by last used time (oldest first)
        candidates.sort_by_key(|(_, t, _)| *t);

        // Evict until under limit
        for (digest, _, size_bytes) in candidates {
            if self.current_cache_bytes <= self.max_cache_bytes {
                break;
            }

            info!(
                digest = %digest,
                size_bytes,
                "Evicting image from cache"
            );

            self.cache.remove(&digest);
            self.current_cache_bytes = self.current_cache_bytes.saturating_sub(size_bytes);

            // TODO: Actually delete the files
        }
    }
}

#[async_trait]
impl Actor for ImagePullActor {
    type Message = ImageMessage;

    fn name(&self) -> &str {
        "image_pull"
    }

    async fn handle(
        &mut self,
        msg: ImageMessage,
        _ctx: &mut ActorContext,
    ) -> Result<bool, ActorError> {
        match msg {
            ImageMessage::EnsurePulled {
                image_ref,
                expected_digest,
                reply_to,
            } => {
                self.handle_ensure_pulled(image_ref, expected_digest, reply_to)
                    .await?;
            }

            ImageMessage::ReleaseRef {
                instance_id,
                digest,
            } => {
                self.handle_release_ref(instance_id, digest);
            }

            ImageMessage::GCCheck { tick_id } => {
                self.handle_gc_check(tick_id);
            }
        }

        Ok(true)
    }

    async fn on_start(&mut self, _ctx: &mut ActorContext) -> Result<(), ActorError> {
        info!(
            image_dir = %self.image_dir,
            max_cache_bytes = self.max_cache_bytes,
            "ImagePullActor starting"
        );

        // Initialize the image cache from disk
        if let Some(cache) = &self.image_cache {
            if let Err(e) = cache.init().await {
                warn!(error = %e, "Failed to initialize image cache from disk");
            }
        }

        Ok(())
    }

    async fn on_stop(&mut self, _ctx: &mut ActorContext) {
        info!(
            cached_count = self.cache.len(),
            in_progress = self.in_progress.len(),
            "ImagePullActor stopping"
        );

        // Fail any in-progress pulls
        let digests: Vec<_> = self.in_progress.keys().cloned().collect();
        for digest in digests {
            self.fail_pull(&digest, "Actor stopping".to_string());
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_cache_entry() {
        let entry = ImageCacheEntry {
            digest: "sha256:abc123".to_string(),
            root_disk_path: "/var/lib/images/abc123/rootfs.ext4".to_string(),
            size_bytes: 512 * 1024 * 1024,
            pulled_at: Instant::now(),
            last_used_at: Instant::now(),
            refs: HashSet::new(),
        };

        assert_eq!(entry.digest, "sha256:abc123");
        assert!(entry.refs.is_empty());
    }

    #[test]
    fn test_image_pull_actor_new() {
        let actor = ImagePullActor::new("/var/lib/images".to_string(), 10 * 1024 * 1024 * 1024);
        assert_eq!(actor.cached_count(), 0);
        assert_eq!(actor.cache_size_bytes(), 0);
    }

    #[tokio::test]
    async fn test_image_pull_result() {
        let result = ImagePullResult {
            digest: "sha256:abc".to_string(),
            root_disk_path: "/path/to/rootfs".to_string(),
            size_bytes: 1024,
        };

        assert_eq!(result.digest, "sha256:abc");
    }
}
