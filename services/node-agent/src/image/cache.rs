//! Image cache with LRU eviction and reference counting.
//!
//! This module manages cached OCI artifacts and root disks,
//! ensuring in-use artifacts are never evicted.
//!
//! Reference: docs/specs/runtime/image-fetch-and-cache.md

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::{debug, info};

/// Configuration for the image cache.
#[derive(Debug, Clone)]
pub struct ImageCacheConfig {
    /// Maximum cache size in bytes.
    pub max_size_bytes: u64,
    /// High water mark that triggers eviction (percentage of max).
    pub high_water_mark: f64,
    /// Low water mark target after eviction (percentage of max).
    pub low_water_mark: f64,
    /// Root disk directory.
    pub rootdisk_dir: PathBuf,
}

impl Default for ImageCacheConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 50 * 1024 * 1024 * 1024, // 50 GiB
            high_water_mark: 0.9,
            low_water_mark: 0.7,
            rootdisk_dir: PathBuf::from("/var/lib/plfm-agent/rootdisks"),
        }
    }
}

/// A cached artifact entry.
#[derive(Debug)]
struct CacheEntry {
    /// Digest of the artifact.
    digest: String,
    /// Path to the artifact.
    path: PathBuf,
    /// Size in bytes.
    size_bytes: u64,
    /// Last access time.
    last_accessed: Instant,
    /// Reference count (number of instances using this).
    ref_count: u32,
}

/// Image cache manager.
pub struct ImageCache {
    config: ImageCacheConfig,
    /// Cached root disks keyed by digest.
    rootdisks: RwLock<HashMap<String, CacheEntry>>,
    /// Statistics.
    stats: CacheStats,
}

/// Cache statistics.
#[derive(Debug, Default)]
pub struct CacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: AtomicU64,
    pub current_size_bytes: AtomicU64,
}

impl ImageCache {
    /// Create a new image cache.
    pub fn new(config: ImageCacheConfig) -> Self {
        Self {
            config,
            rootdisks: RwLock::new(HashMap::new()),
            stats: CacheStats::default(),
        }
    }

    /// Initialize cache from disk.
    pub async fn init(&self) -> std::io::Result<()> {
        // Scan root disk directory
        if self.config.rootdisk_dir.exists() {
            let mut rootdisks = self.rootdisks.write().await;
            for entry in fs::read_dir(&self.config.rootdisk_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "ext4").unwrap_or(false) {
                    let digest = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.replace('_', ":"))
                        .unwrap_or_default();

                    let metadata = entry.metadata()?;
                    let size = metadata.len();

                    rootdisks.insert(
                        digest.clone(),
                        CacheEntry {
                            digest,
                            path,
                            size_bytes: size,
                            last_accessed: Instant::now(),
                            ref_count: 0,
                        },
                    );

                    self.stats
                        .current_size_bytes
                        .fetch_add(size, Ordering::Relaxed);
                }
            }
            info!(count = rootdisks.len(), "Loaded root disks from cache");
        }

        Ok(())
    }

    /// Register a root disk in the cache.
    pub async fn register_rootdisk(&self, digest: &str, path: PathBuf, size_bytes: u64) {
        let mut rootdisks = self.rootdisks.write().await;

        if !rootdisks.contains_key(digest) {
            rootdisks.insert(
                digest.to_string(),
                CacheEntry {
                    digest: digest.to_string(),
                    path,
                    size_bytes,
                    last_accessed: Instant::now(),
                    ref_count: 0,
                },
            );

            self.stats
                .current_size_bytes
                .fetch_add(size_bytes, Ordering::Relaxed);

            debug!(digest = %digest, size = size_bytes, "Registered root disk");
        }
    }

    /// Acquire a reference to a root disk (prevents eviction).
    pub async fn acquire_rootdisk(&self, digest: &str) -> Option<PathBuf> {
        let mut rootdisks = self.rootdisks.write().await;

        if let Some(entry) = rootdisks.get_mut(digest) {
            entry.ref_count += 1;
            entry.last_accessed = Instant::now();
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.path.clone())
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Release a reference to a root disk.
    pub async fn release_rootdisk(&self, digest: &str) {
        let mut rootdisks = self.rootdisks.write().await;

        if let Some(entry) = rootdisks.get_mut(digest) {
            entry.ref_count = entry.ref_count.saturating_sub(1);
            debug!(
                digest = %digest,
                ref_count = entry.ref_count,
                "Released root disk reference"
            );
        }
    }

    /// Check if a root disk exists in cache.
    pub async fn has_rootdisk(&self, digest: &str) -> bool {
        let rootdisks = self.rootdisks.read().await;
        rootdisks.contains_key(digest)
    }

    /// Get current cache size.
    pub fn current_size(&self) -> u64 {
        self.stats.current_size_bytes.load(Ordering::Relaxed)
    }

    /// Check if eviction is needed.
    pub fn needs_eviction(&self) -> bool {
        let current = self.current_size();
        let threshold = (self.config.max_size_bytes as f64 * self.config.high_water_mark) as u64;
        current > threshold
    }

    /// Run eviction to free space.
    pub async fn evict(&self) -> std::io::Result<u64> {
        let target = (self.config.max_size_bytes as f64 * self.config.low_water_mark) as u64;
        let mut freed = 0u64;

        // Collect eviction candidates (ref_count == 0)
        let candidates: Vec<(String, PathBuf, u64, Instant)> = {
            let rootdisks = self.rootdisks.read().await;
            rootdisks
                .values()
                .filter(|e| e.ref_count == 0)
                .map(|e| {
                    (
                        e.digest.clone(),
                        e.path.clone(),
                        e.size_bytes,
                        e.last_accessed,
                    )
                })
                .collect()
        };

        // Sort by last accessed (oldest first)
        let mut candidates = candidates;
        candidates.sort_by_key(|(_, _, _, accessed)| *accessed);

        // Evict until we reach target
        for (digest, path, size, _) in candidates {
            if self.current_size() <= target {
                break;
            }

            // Remove from cache
            {
                let mut rootdisks = self.rootdisks.write().await;
                if let Some(entry) = rootdisks.get(&digest) {
                    // Double-check ref_count
                    if entry.ref_count > 0 {
                        continue;
                    }
                }
                rootdisks.remove(&digest);
            }

            // Delete file
            if path.exists() {
                fs::remove_file(&path)?;
                // Also remove metadata
                let meta_path = path.with_extension("meta.json");
                fs::remove_file(&meta_path).ok();
            }

            self.stats
                .current_size_bytes
                .fetch_sub(size, Ordering::Relaxed);
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            freed += size;

            info!(digest = %digest, size = size, "Evicted root disk");
        }

        Ok(freed)
    }

    /// Get cache statistics.
    pub fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.stats.hits.load(Ordering::Relaxed),
            self.stats.misses.load(Ordering::Relaxed),
            self.stats.evictions.load(Ordering::Relaxed),
            self.stats.current_size_bytes.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_register_and_acquire() {
        let config = ImageCacheConfig {
            rootdisk_dir: PathBuf::from("/tmp/test-cache"),
            ..Default::default()
        };
        let cache = ImageCache::new(config);

        // Register a root disk
        cache
            .register_rootdisk("sha256:abc123", PathBuf::from("/tmp/test.ext4"), 1024)
            .await;

        // Acquire should succeed
        let path = cache.acquire_rootdisk("sha256:abc123").await;
        assert!(path.is_some());

        // Cache should have the entry
        assert!(cache.has_rootdisk("sha256:abc123").await);

        // Release
        cache.release_rootdisk("sha256:abc123").await;
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let config = ImageCacheConfig::default();
        let cache = ImageCache::new(config);

        let path = cache.acquire_rootdisk("sha256:notexist").await;
        assert!(path.is_none());

        let (hits, misses, _, _) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 1);
    }

    #[test]
    fn test_needs_eviction() {
        let config = ImageCacheConfig {
            max_size_bytes: 1000,
            high_water_mark: 0.9,
            ..Default::default()
        };
        let cache = ImageCache::new(config);

        // Under threshold
        cache.stats.current_size_bytes.store(800, Ordering::Relaxed);
        assert!(!cache.needs_eviction());

        // Over threshold
        cache.stats.current_size_bytes.store(950, Ordering::Relaxed);
        assert!(cache.needs_eviction());
    }
}
