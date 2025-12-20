//! Firecracker jailer configuration and cgroup setup.
//!
//! The jailer provides an additional layer of isolation for Firecracker microVMs
//! by running each VM in a chroot with restricted capabilities.
//!
//! This module handles:
//! - Sandbox directory creation
//! - Cgroup v2 setup (memory limits, CPU weight)
//! - Jailer command line construction
//!
//! Reference: docs/specs/runtime/limits-and-isolation.md

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

/// Errors from jailer operations.
#[derive(Debug, Error)]
pub enum JailerError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Cgroup error: {message}")]
    Cgroup { message: String },

    #[error("Invalid configuration: {0}")]
    Config(String),
}

/// Jailer configuration for a microVM.
#[derive(Debug, Clone)]
pub struct JailerConfig {
    /// Instance ID (used for naming).
    pub instance_id: String,
    /// Path to the jailer binary.
    pub jailer_path: PathBuf,
    /// Path to the firecracker binary.
    pub firecracker_path: PathBuf,
    /// Base directory for jailed environments.
    pub chroot_base: PathBuf,
    /// UID to run the VM as.
    pub uid: u32,
    /// GID to run the VM as.
    pub gid: u32,
    /// Cgroup version (1 or 2).
    pub cgroup_version: u8,
    /// Memory limit in bytes.
    pub memory_limit_bytes: Option<u64>,
    /// CPU weight (1-10000, default 100).
    pub cpu_weight: Option<u32>,
    /// Enable NUMA node pinning.
    pub numa_node: Option<u32>,
}

impl JailerConfig {
    /// Create a new jailer configuration.
    pub fn new(instance_id: &str, chroot_base: PathBuf) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            jailer_path: PathBuf::from("/usr/bin/jailer"),
            firecracker_path: PathBuf::from("/usr/bin/firecracker"),
            chroot_base,
            uid: 1000,
            gid: 1000,
            cgroup_version: 2,
            memory_limit_bytes: None,
            cpu_weight: None,
            numa_node: None,
        }
    }

    /// Get the chroot directory for this instance.
    pub fn chroot_dir(&self) -> PathBuf {
        self.chroot_base
            .join("firecracker")
            .join(&self.instance_id)
            .join("root")
    }

    /// Get the API socket path inside the chroot.
    pub fn api_socket_path(&self) -> PathBuf {
        self.chroot_dir().join("run").join("firecracker.socket")
    }

    /// Get the cgroup path for this instance.
    pub fn cgroup_path(&self) -> PathBuf {
        PathBuf::from("/sys/fs/cgroup")
            .join("firecracker")
            .join(&self.instance_id)
    }

    /// Set memory limit.
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit_bytes = Some(bytes);
        self
    }

    /// Set CPU weight.
    pub fn with_cpu_weight(mut self, weight: u32) -> Self {
        self.cpu_weight = Some(weight.clamp(1, 10000));
        self
    }
}

/// Sandbox manager for Firecracker instances.
pub struct SandboxManager {
    config: JailerConfig,
}

impl SandboxManager {
    /// Create a new sandbox manager.
    pub fn new(config: JailerConfig) -> Self {
        Self { config }
    }

    /// Prepare the sandbox directory structure.
    pub fn prepare_sandbox(&self) -> Result<SandboxPaths, JailerError> {
        let chroot = self.config.chroot_dir();

        // Create directory structure
        let dirs = [chroot.join("dev"), chroot.join("run"), chroot.join("tmp")];

        for dir in &dirs {
            fs::create_dir_all(dir)?;
        }

        debug!(
            chroot = %chroot.display(),
            "Sandbox directory prepared"
        );

        Ok(SandboxPaths {
            chroot,
            socket: self.config.api_socket_path(),
        })
    }

    /// Set up cgroup v2 limits for the instance.
    pub fn setup_cgroups(&self) -> Result<(), JailerError> {
        if self.config.cgroup_version != 2 {
            warn!("Only cgroup v2 is supported");
            return Ok(());
        }

        let cgroup_path = self.config.cgroup_path();

        // Create cgroup directory
        if !cgroup_path.exists() {
            fs::create_dir_all(&cgroup_path)?;
        }

        // Set memory limit
        if let Some(limit) = self.config.memory_limit_bytes {
            let memory_max = cgroup_path.join("memory.max");
            fs::write(&memory_max, limit.to_string())?;
            debug!(limit_bytes = limit, "Set memory.max");
        }

        // Set CPU weight
        if let Some(weight) = self.config.cpu_weight {
            let cpu_weight = cgroup_path.join("cpu.weight");
            fs::write(&cpu_weight, weight.to_string())?;
            debug!(weight = weight, "Set cpu.weight");
        }

        Ok(())
    }

    /// Clean up the sandbox after instance termination.
    pub fn cleanup(&self) -> Result<(), JailerError> {
        let chroot = self.config.chroot_dir();

        // Remove chroot directory
        if chroot.exists() {
            fs::remove_dir_all(&chroot)?;
            debug!(chroot = %chroot.display(), "Cleaned up sandbox");
        }

        // Remove cgroup
        let cgroup_path = self.config.cgroup_path();
        if cgroup_path.exists() {
            // First, ensure no processes are in the cgroup
            let procs = cgroup_path.join("cgroup.procs");
            if procs.exists() {
                let content = fs::read_to_string(&procs)?;
                if !content.trim().is_empty() {
                    warn!("Cgroup still has processes, skipping removal");
                    return Ok(());
                }
            }

            fs::remove_dir(&cgroup_path)?;
            debug!(cgroup = %cgroup_path.display(), "Cleaned up cgroup");
        }

        Ok(())
    }

    /// Build the jailer command line arguments.
    pub fn jailer_args(&self) -> Vec<String> {
        let mut args = vec![
            "--id".to_string(),
            self.config.instance_id.clone(),
            "--exec-file".to_string(),
            self.config.firecracker_path.to_string_lossy().to_string(),
            "--uid".to_string(),
            self.config.uid.to_string(),
            "--gid".to_string(),
            self.config.gid.to_string(),
            "--chroot-base-dir".to_string(),
            self.config.chroot_base.to_string_lossy().to_string(),
        ];

        if self.config.cgroup_version == 2 {
            args.push("--cgroup-version".to_string());
            args.push("2".to_string());
        }

        if let Some(node) = self.config.numa_node {
            args.push("--numa-node".to_string());
            args.push(node.to_string());
        }

        args
    }
}

/// Paths created by sandbox preparation.
#[derive(Debug, Clone)]
pub struct SandboxPaths {
    /// Chroot directory.
    pub chroot: PathBuf,
    /// API socket path.
    pub socket: PathBuf,
}

/// Copy a file into the sandbox chroot.
pub fn copy_to_sandbox<P: AsRef<Path>, Q: AsRef<Path>>(
    source: P,
    sandbox_path: Q,
) -> io::Result<()> {
    let dest = sandbox_path.as_ref();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, dest)?;
    Ok(())
}

/// Create a hard link in the sandbox (more efficient than copy).
pub fn link_to_sandbox<P: AsRef<Path>, Q: AsRef<Path>>(
    source: P,
    sandbox_path: Q,
) -> io::Result<()> {
    let dest = sandbox_path.as_ref();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::hard_link(source, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jailer_config_paths() {
        let config = JailerConfig::new("inst-123", PathBuf::from("/var/lib/firecracker"));

        let chroot = config.chroot_dir();
        assert!(chroot.to_string_lossy().contains("inst-123"));
        assert!(chroot.to_string_lossy().contains("firecracker"));

        let socket = config.api_socket_path();
        assert!(socket.to_string_lossy().contains("firecracker.socket"));
    }

    #[test]
    fn test_jailer_args() {
        let config = JailerConfig::new("inst-123", PathBuf::from("/var/lib/firecracker"))
            .with_memory_limit(512 * 1024 * 1024)
            .with_cpu_weight(100);

        let manager = SandboxManager::new(config);
        let args = manager.jailer_args();

        assert!(args.contains(&"--id".to_string()));
        assert!(args.contains(&"inst-123".to_string()));
        assert!(args.contains(&"--cgroup-version".to_string()));
        assert!(args.contains(&"2".to_string()));
    }

    #[test]
    fn test_cpu_weight_clamping() {
        let config = JailerConfig::new("test", PathBuf::from("/tmp")).with_cpu_weight(50000); // Way too high

        assert_eq!(config.cpu_weight, Some(10000)); // Should be clamped
    }
}
