//! Volume mounting.
//!
//! Mounts volumes according to the configuration from the host agent.
//! Note: This module is Linux-only and uses direct libc calls.

#[cfg(target_os = "linux")]
use std::ffi::CString;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::ptr;

use anyhow::Result;
#[cfg(target_os = "linux")]
use tracing::info;

use crate::config::MountConfig;
use crate::error::InitError;

/// Reserved paths that cannot be mount targets.
const RESERVED_PATHS: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run/secrets",
    "/tmp",
    "/run",
];

/// Mount flags for read-only.
#[cfg(target_os = "linux")]
const MS_RDONLY: libc::c_ulong = 1;

/// Mount a volume according to configuration.
pub fn mount_volume(config: &MountConfig) -> Result<()> {
    // Validate mount point is not reserved
    for reserved in RESERVED_PATHS {
        if config.mountpoint == *reserved || config.mountpoint.starts_with(&format!("{}/", reserved)) {
            return Err(InitError::MountFailed {
                name: config.name.clone(),
                detail: format!("mountpoint '{}' is reserved", config.mountpoint),
            }
            .into());
        }
    }

    match config.kind.as_str() {
        "volume" => mount_block_volume(config),
        "tmpfs" => mount_tmpfs(config),
        other => Err(InitError::MountFailed {
            name: config.name.clone(),
            detail: format!("unknown mount kind: {}", other),
        }
        .into()),
    }
}

/// Mount a block device volume using libc.
#[cfg(target_os = "linux")]
fn mount_block_volume(config: &MountConfig) -> Result<()> {
    let device = config.device.as_ref().ok_or_else(|| InitError::MountFailed {
        name: config.name.clone(),
        detail: "device path required for volume mount".to_string(),
    })?;

    let mountpoint = Path::new(&config.mountpoint);

    // Ensure mountpoint directory exists
    if !mountpoint.exists() {
        fs::create_dir_all(mountpoint).map_err(|e| InitError::MountFailed {
            name: config.name.clone(),
            detail: format!("failed to create mountpoint: {}", e),
        })?;
    }

    // Prepare C strings
    let source = CString::new(device.as_str()).map_err(|e| InitError::MountFailed {
        name: config.name.clone(),
        detail: format!("invalid device path: {}", e),
    })?;

    let target = CString::new(config.mountpoint.as_str()).map_err(|e| InitError::MountFailed {
        name: config.name.clone(),
        detail: format!("invalid mountpoint: {}", e),
    })?;

    let fstype = CString::new(config.fs_type.as_str()).map_err(|e| InitError::MountFailed {
        name: config.name.clone(),
        detail: format!("invalid fstype: {}", e),
    })?;

    // Determine mount flags
    let flags: libc::c_ulong = if config.mode == "ro" { MS_RDONLY } else { 0 };

    // Call mount syscall
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            flags,
            ptr::null(),
        )
    };

    if result != 0 {
        let err = std::io::Error::last_os_error();
        return Err(InitError::MountFailed {
            name: config.name.clone(),
            detail: format!("{} mount failed: {}", config.fs_type, err),
        }
        .into());
    }

    info!(
        name = %config.name,
        device = %device,
        mountpoint = %config.mountpoint,
        fs_type = %config.fs_type,
        mode = %config.mode,
        "volume mounted"
    );

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
fn mount_block_volume(config: &MountConfig) -> Result<()> {
    Err(InitError::MountFailed {
        name: config.name.clone(),
        detail: "block volume mounts only supported on Linux".to_string(),
    }
    .into())
}

/// Mount a tmpfs volume using libc.
#[cfg(target_os = "linux")]
fn mount_tmpfs(config: &MountConfig) -> Result<()> {
    let mountpoint = Path::new(&config.mountpoint);

    // Ensure mountpoint directory exists
    if !mountpoint.exists() {
        fs::create_dir_all(mountpoint).map_err(|e| InitError::MountFailed {
            name: config.name.clone(),
            detail: format!("failed to create mountpoint: {}", e),
        })?;
    }

    // Prepare C strings
    let source = CString::new("tmpfs").unwrap();
    let target = CString::new(config.mountpoint.as_str()).map_err(|e| InitError::MountFailed {
        name: config.name.clone(),
        detail: format!("invalid mountpoint: {}", e),
    })?;
    let fstype = CString::new("tmpfs").unwrap();

    // Call mount syscall
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            0,
            ptr::null(),
        )
    };

    if result != 0 {
        let err = std::io::Error::last_os_error();
        return Err(InitError::MountFailed {
            name: config.name.clone(),
            detail: format!("tmpfs mount failed: {}", err),
        }
        .into());
    }

    info!(
        name = %config.name,
        mountpoint = %config.mountpoint,
        "tmpfs mounted"
    );

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
fn mount_tmpfs(config: &MountConfig) -> Result<()> {
    Err(InitError::MountFailed {
        name: config.name.clone(),
        detail: "tmpfs mounts only supported on Linux".to_string(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reserved_paths() {
        assert!(RESERVED_PATHS.contains(&"/proc"));
        assert!(RESERVED_PATHS.contains(&"/run/secrets"));
    }

    #[test]
    fn test_reserved_path_check() {
        let config = MountConfig {
            kind: "volume".to_string(),
            name: "test".to_string(),
            device: Some("/dev/vdc".to_string()),
            mountpoint: "/proc/foo".to_string(),
            fs_type: "ext4".to_string(),
            mode: "rw".to_string(),
        };

        let result = mount_volume(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("reserved"));
    }
}
