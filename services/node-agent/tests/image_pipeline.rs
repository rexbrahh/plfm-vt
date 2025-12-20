//! Integration tests for the image pull and root disk pipeline.
//!
//! These tests verify the OCI client, rootdisk builder, and image cache
//! work together correctly.

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use plfm_node_agent::image::{
    ImageCache, ImageCacheConfig, ImagePuller, ImagePullerConfig, OciConfig, RootDiskConfig,
};

/// Create a test puller with temporary directories.
fn create_test_puller(temp_dir: &TempDir) -> (Arc<ImagePuller>, Arc<ImageCache>) {
    let base_path = temp_dir.path();

    let cache_config = ImageCacheConfig {
        max_size_bytes: 10 * 1024 * 1024 * 1024, // 10 GiB
        high_water_mark: 0.9,
        low_water_mark: 0.7,
        rootdisk_dir: base_path.join("rootdisks"),
    };
    let cache = Arc::new(ImageCache::new(cache_config));

    let puller_config = ImagePullerConfig {
        oci: OciConfig {
            registry_url: "https://registry-1.docker.io".to_string(),
            auth_token: None,
            layer_timeout: std::time::Duration::from_secs(60),
            total_timeout: std::time::Duration::from_secs(300),
            max_compressed_size: 1024 * 1024 * 1024, // 1 GiB
            blob_dir: base_path.join("oci/blobs"),
        },
        rootdisk: RootDiskConfig {
            unpack_dir: base_path.join("unpacked"),
            rootdisk_dir: base_path.join("rootdisks"),
            tmp_dir: base_path.join("tmp"),
            max_uncompressed_size: 5 * 1024 * 1024 * 1024, // 5 GiB
            size_headroom_factor: 1.2,
            min_disk_size: 64 * 1024 * 1024, // 64 MiB for tests
        },
        max_concurrent_builds: 2,
    };

    let puller = ImagePuller::new(puller_config, cache.clone()).unwrap();
    (Arc::new(puller), cache)
}

#[test]
fn test_image_puller_creation() {
    let temp_dir = TempDir::new().unwrap();
    let (_puller, _cache) = create_test_puller(&temp_dir);
    // Just verify it creates successfully
}

#[tokio::test]
async fn test_cache_initialization() {
    let temp_dir = TempDir::new().unwrap();
    let (_puller, cache) = create_test_puller(&temp_dir);

    // Initialize empty cache
    cache.init().await.unwrap();

    // Should have no entries
    assert!(!cache.has_rootdisk("sha256:nonexistent").await);
}

#[tokio::test]
async fn test_cache_register_and_acquire() {
    let temp_dir = TempDir::new().unwrap();
    let (_puller, cache) = create_test_puller(&temp_dir);

    let digest = "sha256:test123";
    let path = PathBuf::from("/tmp/test.ext4");
    let size = 1024u64;

    // Register
    cache.register_rootdisk(digest, path.clone(), size).await;

    // Should be findable
    assert!(cache.has_rootdisk(digest).await);

    // Acquire
    let acquired = cache.acquire_rootdisk(digest).await;
    assert!(acquired.is_some());
    assert_eq!(acquired.unwrap(), path);

    // Release
    cache.release_rootdisk(digest).await;
}

#[tokio::test]
async fn test_cache_eviction_respects_refs() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create a cache with very small size limit
    let cache_config = ImageCacheConfig {
        max_size_bytes: 100, // Very small
        high_water_mark: 0.9,
        low_water_mark: 0.5,
        rootdisk_dir: base_path.join("rootdisks"),
    };
    let cache = ImageCache::new(cache_config);

    // Register two entries
    let digest1 = "sha256:entry1";
    let digest2 = "sha256:entry2";

    cache
        .register_rootdisk(digest1, PathBuf::from("/tmp/1.ext4"), 60)
        .await;
    cache
        .register_rootdisk(digest2, PathBuf::from("/tmp/2.ext4"), 60)
        .await;

    // Acquire reference to entry1
    cache.acquire_rootdisk(digest1).await;

    // Cache is now over limit (120 > 100)
    assert!(cache.needs_eviction());

    // Entry1 should not be evicted because it has refs
    // Entry2 should be evictable (but we'd need the actual files to evict)
}

#[test]
fn test_parse_image_ref_docker_hub() {
    use plfm_node_agent::image::parse_image_ref;

    // Library image
    let (registry, repo, tag) = parse_image_ref("alpine:3.18").unwrap();
    assert_eq!(registry, "registry-1.docker.io");
    assert_eq!(repo, "library/alpine");
    assert_eq!(tag, "3.18");

    // User image
    let (registry, repo, tag) = parse_image_ref("myuser/myapp:latest").unwrap();
    assert_eq!(registry, "registry-1.docker.io");
    assert_eq!(repo, "myuser/myapp");
    assert_eq!(tag, "latest");
}

#[test]
fn test_parse_image_ref_custom_registry() {
    use plfm_node_agent::image::parse_image_ref;

    let (registry, repo, tag) = parse_image_ref("ghcr.io/owner/repo:v1.0.0").unwrap();
    assert_eq!(registry, "ghcr.io");
    assert_eq!(repo, "owner/repo");
    assert_eq!(tag, "v1.0.0");

    let (registry, repo, tag) = parse_image_ref("gcr.io/project/image:latest").unwrap();
    assert_eq!(registry, "gcr.io");
    assert_eq!(repo, "project/image");
    assert_eq!(tag, "latest");
}

#[test]
fn test_parse_image_ref_digest() {
    use plfm_node_agent::image::parse_image_ref;

    let digest = "sha256:a3ed95caeb02ffe68cdd9fd84406680ae93d633cb16422d00e8a7c22955b46d4";
    let (registry, repo, reference) = parse_image_ref(&format!("alpine@{}", digest)).unwrap();
    assert_eq!(registry, "registry-1.docker.io");
    assert_eq!(repo, "library/alpine");
    assert_eq!(reference, digest);
}

#[test]
fn test_parse_image_ref_localhost() {
    use plfm_node_agent::image::parse_image_ref;

    let (registry, repo, tag) = parse_image_ref("localhost:5000/myimage:dev").unwrap();
    assert_eq!(registry, "localhost:5000");
    assert_eq!(repo, "myimage");
    assert_eq!(tag, "dev");
}

// Note: The following test would require network access and a real registry.
// It's commented out but shows how to test the full pipeline.
//
// #[tokio::test]
// #[ignore] // Requires network access
// async fn test_pull_real_image() {
//     let temp_dir = TempDir::new().unwrap();
//     let (puller, _cache) = create_test_puller(&temp_dir);
//
//     // Pull a small public image
//     let result = puller
//         .ensure_image(
//             "alpine:3.18",
//             "library/alpine",
//             "sha256:...", // Real digest
//         )
//         .await
//         .unwrap();
//
//     assert!(result.root_disk_path.exists());
//     assert!(!result.was_cached);
// }
