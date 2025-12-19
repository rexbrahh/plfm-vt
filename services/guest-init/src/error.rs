//! Error types for guest init.

use thiserror::Error;

/// Guest init errors with standardized reason codes.
#[derive(Debug, Error)]
#[allow(dead_code)] // Variants used by future code paths
pub enum InitError {
    /// Could not connect to host agent.
    #[error("handshake failed: {0}")]
    HandshakeFailed(String),

    /// Could not parse config JSON.
    #[error("config_parse_failed: {0}")]
    ConfigParseFailed(String),

    /// Networking configuration failed.
    #[error("net_config_failed: {0}")]
    NetConfigFailed(String),

    /// Volume mount failed.
    #[error("mount_failed: volume {name}: {detail}")]
    MountFailed { name: String, detail: String },

    /// Required secrets not provided.
    #[error("secrets_missing: {0}")]
    SecretsMissing(String),

    /// Could not write secrets file.
    #[error("secrets_write_failed: {0}")]
    SecretsWriteFailed(String),

    /// Could not exec workload command.
    #[error("workload_start_failed: {0}")]
    WorkloadStartFailed(String),

    /// Workload exited immediately (crash loop).
    #[error("workload_crashed: exit_code={exit_code}")]
    WorkloadCrashed { exit_code: i32 },

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Vsock error.
    #[error("vsock error: {0}")]
    Vsock(String),

    /// System call error.
    #[error("syscall error: {0}")]
    Syscall(#[from] nix::Error),
}

impl InitError {
    /// Get the standardized reason code for this error.
    #[allow(dead_code)] // Used by future error reporting
    pub fn reason_code(&self) -> &'static str {
        match self {
            InitError::HandshakeFailed(_) => "handshake_failed",
            InitError::ConfigParseFailed(_) => "config_parse_failed",
            InitError::NetConfigFailed(_) => "net_config_failed",
            InitError::MountFailed { .. } => "mount_failed",
            InitError::SecretsMissing(_) => "secrets_missing",
            InitError::SecretsWriteFailed(_) => "secrets_write_failed",
            InitError::WorkloadStartFailed(_) => "workload_start_failed",
            InitError::WorkloadCrashed { .. } => "workload_crashed",
            InitError::Io(_) => "io_error",
            InitError::Vsock(_) => "vsock_error",
            InitError::Syscall(_) => "syscall_error",
        }
    }
}
