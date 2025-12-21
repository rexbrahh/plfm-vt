//! Route state persistence.
//!
//! This module handles saving and loading route state to disk for:
//! - Atomic config reload (write to temp, rename)
//! - Fast startup with last known state
//! - Control plane outage resilience
//!
//! Per docs/specs/networking/ingress-l4.md:
//! - "Control plane outage behavior: edge continues operating on last applied config"
//! - "config updates must be applied atomically"

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use plfm_events::{RouteProtocolHint, RouteProxyProtocol};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Persisted route state file format version.
/// v2: Added protocol_hint field for raw TCP support.
const STATE_VERSION: u32 = 2;

/// Persisted route state.
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedState {
    /// Format version.
    pub version: u32,
    /// Last applied event ID (cursor).
    pub cursor: i64,
    /// Routes by route_id.
    pub routes: BTreeMap<String, PersistedRoute>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            cursor: 0,
            routes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRoute {
    pub route_id: String,
    pub hostname: String,
    pub listen_port: i32,
    pub app_id: String,
    pub env_id: String,
    pub backend_process_type: String,
    pub backend_port: i32,
    pub protocol_hint: String,
    pub proxy_protocol: String,
    pub backend_expects_proxy_protocol: bool,
    pub ipv4_required: bool,
    #[serde(default)]
    pub env_ipv4_address: Option<String>,
}

impl PersistedRoute {
    pub fn protocol_hint_to_string(p: RouteProtocolHint) -> String {
        match p {
            RouteProtocolHint::TlsPassthrough => "tls_passthrough".to_string(),
            RouteProtocolHint::TcpRaw => "tcp_raw".to_string(),
        }
    }

    pub fn protocol_hint_from_string(s: &str) -> RouteProtocolHint {
        match s {
            "tcp_raw" => RouteProtocolHint::TcpRaw,
            _ => RouteProtocolHint::TlsPassthrough,
        }
    }

    pub fn proxy_protocol_to_string(p: RouteProxyProtocol) -> String {
        match p {
            RouteProxyProtocol::Off => "off".to_string(),
            RouteProxyProtocol::V2 => "v2".to_string(),
        }
    }

    pub fn proxy_protocol_from_string(s: &str) -> RouteProxyProtocol {
        match s {
            "v2" => RouteProxyProtocol::V2,
            _ => RouteProxyProtocol::Off,
        }
    }
}

/// State persistence manager.
pub struct StatePersistence {
    /// Path to the state file.
    state_path: PathBuf,
}

impl StatePersistence {
    /// Create a new state persistence manager.
    pub fn new(state_path: PathBuf) -> Self {
        Self { state_path }
    }

    /// Load state from disk.
    ///
    /// Returns default state if file doesn't exist.
    /// Returns error if file exists but is invalid.
    pub fn load(&self) -> Result<PersistedState> {
        if !self.state_path.exists() {
            debug!(path = %self.state_path.display(), "No state file, starting fresh");
            return Ok(PersistedState::default());
        }

        let content = fs::read_to_string(&self.state_path)
            .with_context(|| format!("Failed to read state file: {}", self.state_path.display()))?;

        let state: PersistedState = serde_json::from_str(&content).with_context(|| {
            format!("Failed to parse state file: {}", self.state_path.display())
        })?;

        // Check version compatibility
        if state.version != STATE_VERSION {
            warn!(
                file_version = state.version,
                current_version = STATE_VERSION,
                "State file version mismatch, starting fresh"
            );
            return Ok(PersistedState::default());
        }

        info!(
            path = %self.state_path.display(),
            cursor = state.cursor,
            route_count = state.routes.len(),
            "Loaded state from disk"
        );

        Ok(state)
    }

    /// Save state to disk atomically.
    ///
    /// Uses write-to-temp + rename pattern for atomicity.
    pub fn save(&self, state: &PersistedState) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        // Write to temp file
        let tmp_path = self.state_path.with_extension("tmp");
        let content = serde_json::to_string_pretty(state).context("Failed to serialize state")?;

        fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;

        // Atomic rename
        fs::rename(&tmp_path, &self.state_path).with_context(|| {
            format!(
                "Failed to rename {} -> {}",
                tmp_path.display(),
                self.state_path.display()
            )
        })?;

        debug!(
            path = %self.state_path.display(),
            cursor = state.cursor,
            route_count = state.routes.len(),
            "Saved state to disk"
        );

        Ok(())
    }

    /// Save state with cursor update.
    pub fn save_with_cursor(
        &self,
        routes: &BTreeMap<String, PersistedRoute>,
        cursor: i64,
    ) -> Result<()> {
        let state = PersistedState {
            version: STATE_VERSION,
            cursor,
            routes: routes.clone(),
        };
        self.save(&state)
    }
}

// PersistenceConfig is not currently used - config is handled in Config::from_env()

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn test_persisted_state_serialization() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "route_123".to_string(),
            PersistedRoute {
                route_id: "route_123".to_string(),
                hostname: "example.com".to_string(),
                listen_port: 443,
                app_id: "app_1".to_string(),
                env_id: "env_1".to_string(),
                backend_process_type: "web".to_string(),
                backend_port: 8080,
                protocol_hint: "tls_passthrough".to_string(),
                proxy_protocol: "off".to_string(),
                backend_expects_proxy_protocol: false,
                ipv4_required: false,
                env_ipv4_address: None,
            },
        );

        let state = PersistedState {
            version: STATE_VERSION,
            cursor: 12345,
            routes,
        };

        let json = serde_json::to_string(&state).unwrap();
        let parsed: PersistedState = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, STATE_VERSION);
        assert_eq!(parsed.cursor, 12345);
        assert_eq!(parsed.routes.len(), 1);
    }

    #[test]
    fn test_state_persistence_roundtrip() {
        let tmp = temp_dir().join(format!("ingress-test-{}.json", std::process::id()));
        let persistence = StatePersistence::new(tmp.clone());

        // Should start empty
        let initial = persistence.load().unwrap();
        assert_eq!(initial.cursor, 0);
        assert!(initial.routes.is_empty());

        // Save some state
        let mut routes = BTreeMap::new();
        routes.insert(
            "r1".to_string(),
            PersistedRoute {
                route_id: "r1".to_string(),
                hostname: "test.example.com".to_string(),
                listen_port: 443,
                app_id: "app_1".to_string(),
                env_id: "env_1".to_string(),
                backend_process_type: "web".to_string(),
                backend_port: 8080,
                protocol_hint: "tls_passthrough".to_string(),
                proxy_protocol: "v2".to_string(),
                backend_expects_proxy_protocol: true,
                ipv4_required: false,
                env_ipv4_address: None,
            },
        );

        persistence.save_with_cursor(&routes, 999).unwrap();

        // Load and verify
        let loaded = persistence.load().unwrap();
        assert_eq!(loaded.cursor, 999);
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(
            loaded.routes.get("r1").unwrap().hostname,
            "test.example.com"
        );

        // Cleanup
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_proxy_protocol_conversion() {
        assert_eq!(
            PersistedRoute::proxy_protocol_to_string(RouteProxyProtocol::Off),
            "off"
        );
        assert_eq!(
            PersistedRoute::proxy_protocol_to_string(RouteProxyProtocol::V2),
            "v2"
        );

        assert_eq!(
            PersistedRoute::proxy_protocol_from_string("v2"),
            RouteProxyProtocol::V2
        );
        assert_eq!(
            PersistedRoute::proxy_protocol_from_string("off"),
            RouteProxyProtocol::Off
        );
        assert_eq!(
            PersistedRoute::proxy_protocol_from_string("invalid"),
            RouteProxyProtocol::Off
        );
    }

    #[test]
    fn test_protocol_hint_conversion() {
        assert_eq!(
            PersistedRoute::protocol_hint_to_string(RouteProtocolHint::TlsPassthrough),
            "tls_passthrough"
        );
        assert_eq!(
            PersistedRoute::protocol_hint_to_string(RouteProtocolHint::TcpRaw),
            "tcp_raw"
        );

        assert_eq!(
            PersistedRoute::protocol_hint_from_string("tls_passthrough"),
            RouteProtocolHint::TlsPassthrough
        );
        assert_eq!(
            PersistedRoute::protocol_hint_from_string("tcp_raw"),
            RouteProtocolHint::TcpRaw
        );
        assert_eq!(
            PersistedRoute::protocol_hint_from_string("invalid"),
            RouteProtocolHint::TlsPassthrough
        );
    }
}
