//! OCI image fetching and caching.
//!
//! This module handles:
//! - Pulling OCI images from registries by digest
//! - Verifying layer integrity
//! - Building ext4 root disks from OCI layers
//! - Caching with LRU eviction
//!
//! ## Reference
//!
//! - Image fetch spec: `docs/specs/runtime/image-fetch-and-cache.md`
//! - Boot contract: `docs/specs/runtime/firecracker-boot.md`

mod cache;
mod oci;
mod puller;
mod rootdisk;

pub use cache::{ImageCache, ImageCacheConfig};
pub use oci::{Descriptor, Manifest, OciClient, OciConfig, OciError};
pub use puller::{parse_image_ref, ImagePullError, ImagePuller, ImagePullerConfig, PullResult};
pub use rootdisk::{RootDiskBuilder, RootDiskConfig, RootDiskError};
