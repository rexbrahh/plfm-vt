//! Configuration and context management.
//!
//! Handles:
//! - API endpoint configuration
//! - Authentication token storage
//! - Current context (org, app, env)

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Configuration file name.
const CONFIG_FILE: &str = "config.json";

/// Credentials file name.
const CREDENTIALS_FILE: &str = "credentials.json";

/// Get the config directory path.
fn config_dir() -> Result<PathBuf> {
    ProjectDirs::from("com", "plfm", "vt")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))
}

/// CLI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// API endpoint URL.
    #[serde(default = "default_api_url")]
    pub api_url: String,

    /// Current context.
    #[serde(default)]
    pub context: CliContext,
}

fn default_api_url() -> String {
    std::env::var("VT_API_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            context: CliContext::default(),
        }
    }
}

impl Config {
    /// Load config from disk, or return default.
    pub fn load() -> Result<Self> {
        let path = config_dir()?.join(CONFIG_FILE);

        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;

        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {:?}", path))
    }

    /// Get the API URL.
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    /// Save config to disk.
    pub fn save(&self) -> Result<()> {
        let dir = config_dir()?;
        fs::create_dir_all(&dir)?;

        let path = dir.join(CONFIG_FILE);
        let contents = serde_json::to_string_pretty(self)?;

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;

            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(contents.as_bytes())?;
        }

        #[cfg(not(unix))]
        {
            fs::write(&path, contents)
                .with_context(|| format!("Failed to write config to {:?}", path))
                .map(|_| ())?;
        }

        Ok(())
    }
}

/// Current CLI context (selected org, app, env).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliContext {
    /// Current organization ID or name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,

    /// Current application ID or name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,

    /// Current environment ID or name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
}

/// Stored credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Access token.
    pub token: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,

    /// Token expiration time (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,

    /// User ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// User email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl Credentials {
    /// Create new credentials.
    pub fn new(token: String) -> Self {
        Self {
            token,
            refresh_token: None,
            expires_at: None,
            user_id: None,
            email: None,
        }
    }

    /// Load credentials from disk.
    pub fn load() -> Result<Option<Self>> {
        let path = config_dir()?.join(CREDENTIALS_FILE);

        if !path.exists() {
            return Ok(None);
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read credentials from {:?}", path))?;

        let creds: Self = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse credentials from {:?}", path))?;

        Ok(Some(creds))
    }

    /// Save credentials to disk.
    pub fn save(&self) -> Result<()> {
        let dir = config_dir()?;
        fs::create_dir_all(&dir)?;

        let path = dir.join(CREDENTIALS_FILE);
        let contents = serde_json::to_string_pretty(self)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            use std::io::Write;
            file.write_all(contents.as_bytes())?;
        }

        #[cfg(not(unix))]
        {
            fs::write(&path, contents)
                .with_context(|| format!("Failed to write credentials to {:?}", path))
                .map(|_| ())?;
        }

        Ok(())
    }

    /// Delete credentials from disk.
    pub fn delete() -> Result<()> {
        let path = config_dir()?.join(CREDENTIALS_FILE);

        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to delete credentials at {:?}", path))?;
        }

        Ok(())
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            chrono::Utc::now() >= expires_at
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(!config.api_url.is_empty());
    }

    #[test]
    fn test_credentials_new() {
        let creds = Credentials::new("test-token".to_string());
        assert_eq!(creds.token, "test-token");
        assert!(!creds.is_expired());
    }
}
