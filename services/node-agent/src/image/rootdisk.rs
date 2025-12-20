//! Root disk builder from OCI layers.
//!
//! This module unpacks OCI image layers and builds ext4 root disk images
//! suitable for Firecracker microVMs.
//!
//! Reference: docs/specs/runtime/image-fetch-and-cache.md

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tar::Archive;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors from root disk building.
#[derive(Debug, Error)]
pub enum RootDiskError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Layer unpack failed: {0}")]
    UnpackFailed(String),

    #[error("Filesystem creation failed: {0}")]
    FsCreationFailed(String),

    #[error("Image too large: {size} bytes exceeds limit of {limit} bytes")]
    TooLarge { size: u64, limit: u64 },

    #[error("Invalid layer: {0}")]
    InvalidLayer(String),
}

/// Configuration for root disk building.
#[derive(Debug, Clone)]
pub struct RootDiskConfig {
    /// Directory for unpacked filesystem trees.
    pub unpack_dir: PathBuf,
    /// Directory for final root disk images.
    pub rootdisk_dir: PathBuf,
    /// Temporary build directory.
    pub tmp_dir: PathBuf,
    /// Maximum uncompressed size.
    pub max_uncompressed_size: u64,
    /// Headroom factor for ext4 image sizing.
    pub size_headroom_factor: f64,
    /// Minimum ext4 image size.
    pub min_disk_size: u64,
}

impl Default for RootDiskConfig {
    fn default() -> Self {
        Self {
            unpack_dir: PathBuf::from("/var/lib/plfm-agent/unpacked"),
            rootdisk_dir: PathBuf::from("/var/lib/plfm-agent/rootdisks"),
            tmp_dir: PathBuf::from("/var/lib/plfm-agent/tmp"),
            max_uncompressed_size: 50 * 1024 * 1024 * 1024, // 50 GiB
            size_headroom_factor: 1.2,
            min_disk_size: 512 * 1024 * 1024, // 512 MiB
        }
    }
}

/// Root disk builder.
pub struct RootDiskBuilder {
    config: RootDiskConfig,
}

impl RootDiskBuilder {
    /// Create a new root disk builder.
    pub fn new(config: RootDiskConfig) -> Self {
        Self { config }
    }

    /// Build a root disk from OCI layers.
    ///
    /// Returns the path to the created ext4 image.
    pub fn build(&self, digest: &str, layer_paths: &[PathBuf]) -> Result<PathBuf, RootDiskError> {
        let sanitized_digest = sanitize_digest(digest);
        let unpack_path = self.config.unpack_dir.join(&sanitized_digest);
        let rootdisk_path = self
            .config
            .rootdisk_dir
            .join(format!("{}.ext4", sanitized_digest));

        // Check if already built
        if rootdisk_path.exists() {
            info!(digest = %digest, "Root disk already exists");
            return Ok(rootdisk_path);
        }

        // Create directories
        fs::create_dir_all(&unpack_path)?;
        fs::create_dir_all(&self.config.rootdisk_dir)?;
        fs::create_dir_all(&self.config.tmp_dir)?;

        // Unpack layers in order
        info!(
            digest = %digest,
            layer_count = layer_paths.len(),
            "Unpacking OCI layers"
        );

        for (i, layer_path) in layer_paths.iter().enumerate() {
            debug!(
                layer = i,
                path = %layer_path.display(),
                "Unpacking layer"
            );
            self.unpack_layer(layer_path, &unpack_path)?;
        }

        // Calculate size
        let used_bytes = dir_size(&unpack_path)?;
        if used_bytes > self.config.max_uncompressed_size {
            return Err(RootDiskError::TooLarge {
                size: used_bytes,
                limit: self.config.max_uncompressed_size,
            });
        }

        let disk_size = self.calculate_disk_size(used_bytes);
        info!(
            digest = %digest,
            used_bytes = used_bytes,
            disk_size = disk_size,
            "Creating ext4 image"
        );

        // Create ext4 image
        self.create_ext4_image(&unpack_path, &rootdisk_path, disk_size)?;

        // Clean up unpacked directory
        fs::remove_dir_all(&unpack_path).ok();

        // Write metadata
        let meta_path = rootdisk_path.with_extension("meta.json");
        let meta = RootDiskMeta {
            digest: digest.to_string(),
            size_bytes: disk_size,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;

        info!(
            digest = %digest,
            path = %rootdisk_path.display(),
            "Root disk created"
        );

        Ok(rootdisk_path)
    }

    /// Unpack a single gzipped tar layer.
    fn unpack_layer(&self, layer_path: &Path, dest: &Path) -> Result<(), RootDiskError> {
        let file = File::open(layer_path)?;
        let reader = BufReader::new(file);

        // Try gzip first, fall back to raw tar
        if is_gzip(layer_path)? {
            let decoder = GzDecoder::new(reader);
            let mut archive = Archive::new(decoder);
            self.extract_archive(&mut archive, dest)
        } else {
            let mut archive = Archive::new(reader);
            self.extract_archive(&mut archive, dest)
        }
    }

    /// Extract a tar archive handling whiteouts.
    fn extract_archive<R: Read>(
        &self,
        archive: &mut Archive<R>,
        dest: &Path,
    ) -> Result<(), RootDiskError> {
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;

            // Check for path traversal
            if path
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            {
                warn!(path = %path.display(), "Skipping path with parent directory");
                continue;
            }

            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Handle whiteout files
            if let Some(target_name) = file_name.strip_prefix(".wh.") {
                if target_name == ".wh..opq" {
                    // Opaque whiteout - remove entire directory contents
                    if let Some(parent) = path.parent() {
                        let full_parent = dest.join(parent);
                        if full_parent.exists() {
                            for entry in fs::read_dir(&full_parent)? {
                                let entry = entry?;
                                let _ = fs::remove_dir_all(entry.path());
                            }
                        }
                    }
                } else {
                    // Regular whiteout - remove specific file
                    if let Some(parent) = path.parent() {
                        let target = dest.join(parent).join(target_name);
                        let _ = fs::remove_file(&target);
                        let _ = fs::remove_dir_all(&target);
                    }
                }
                continue;
            }

            // Extract normally
            let full_path = dest.join(&path);
            entry.unpack(&full_path)?;
        }

        Ok(())
    }

    /// Calculate ext4 image size with headroom.
    fn calculate_disk_size(&self, used_bytes: u64) -> u64 {
        let with_headroom = (used_bytes as f64 * self.config.size_headroom_factor) as u64;
        with_headroom.max(self.config.min_disk_size)
    }

    /// Create an ext4 image from a directory tree.
    fn create_ext4_image(
        &self,
        source: &Path,
        dest: &Path,
        size: u64,
    ) -> Result<(), RootDiskError> {
        let temp_path = self
            .config
            .tmp_dir
            .join(format!("rootdisk-{}.ext4", std::process::id()));

        // Create sparse file
        let file = File::create(&temp_path)?;
        file.set_len(size)?;
        drop(file);

        // Format as ext4
        let status = Command::new("mkfs.ext4")
            .args(["-F", "-q"])
            .arg(&temp_path)
            .status()
            .map_err(|e| RootDiskError::FsCreationFailed(e.to_string()))?;

        if !status.success() {
            return Err(RootDiskError::FsCreationFailed(
                "mkfs.ext4 failed".to_string(),
            ));
        }

        // Mount and copy
        let mount_dir = self
            .config
            .tmp_dir
            .join(format!("mount-{}", std::process::id()));
        fs::create_dir_all(&mount_dir)?;

        let status = Command::new("mount")
            .args(["-o", "loop"])
            .arg(&temp_path)
            .arg(&mount_dir)
            .status()
            .map_err(|e| RootDiskError::FsCreationFailed(e.to_string()))?;

        if !status.success() {
            return Err(RootDiskError::FsCreationFailed("mount failed".to_string()));
        }

        // Copy files
        let copy_result = Command::new("cp")
            .args(["-a", "--reflink=auto"])
            .arg(format!("{}/.", source.display()))
            .arg(&mount_dir)
            .status();

        // Always unmount
        let _ = Command::new("umount").arg(&mount_dir).status();
        let _ = fs::remove_dir(&mount_dir);

        copy_result
            .map_err(|e| RootDiskError::FsCreationFailed(e.to_string()))?
            .success()
            .then_some(())
            .ok_or_else(|| RootDiskError::FsCreationFailed("cp failed".to_string()))?;

        // Move to final location
        fs::rename(&temp_path, dest)?;

        Ok(())
    }

    /// Get the root disk path for a digest.
    pub fn rootdisk_path(&self, digest: &str) -> PathBuf {
        let sanitized = sanitize_digest(digest);
        self.config.rootdisk_dir.join(format!("{}.ext4", sanitized))
    }

    /// Check if a root disk exists.
    pub fn rootdisk_exists(&self, digest: &str) -> bool {
        self.rootdisk_path(digest).exists()
    }
}

/// Root disk metadata.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RootDiskMeta {
    pub digest: String,
    pub size_bytes: u64,
    pub created_at: String,
}

/// Sanitize a digest for use in file paths.
fn sanitize_digest(digest: &str) -> String {
    digest.replace([':', '/'], "_")
}

/// Check if a file is gzip compressed.
fn is_gzip(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    if file.read_exact(&mut magic).is_ok() {
        Ok(magic == [0x1f, 0x8b])
    } else {
        Ok(false)
    }
}

/// Calculate directory size recursively.
fn dir_size(path: &Path) -> io::Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                total += dir_size(&path)?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_digest() {
        assert_eq!(sanitize_digest("sha256:abc123"), "sha256_abc123");
    }

    #[test]
    fn test_calculate_disk_size() {
        let config = RootDiskConfig::default();
        let builder = RootDiskBuilder::new(config);

        // Small size should use minimum
        let size = builder.calculate_disk_size(100 * 1024 * 1024); // 100 MiB
        assert!(size >= 512 * 1024 * 1024);

        // Large size should use headroom
        let size = builder.calculate_disk_size(1024 * 1024 * 1024); // 1 GiB
        assert!(size > 1024 * 1024 * 1024);
        assert!(size < 2 * 1024 * 1024 * 1024);
    }
}
