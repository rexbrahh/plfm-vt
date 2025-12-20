//! Secrets file format library.
//!
//! Implements the v1 secrets delivery format (dotenv-style) per ADR-0010.
//! Secrets are delivered as `/run/secrets/platform.env` with mode 0400.
//!
//! # Format
//!
//! ```text
//! # plfm-secrets v1
//! KEY=value
//! ANOTHER_KEY=another value
//! ```
//!
//! Keys must match `[A-Za-z_][A-Za-z0-9_]*` and be <= 256 bytes.
//! Values are UTF-8 strings; newlines and special chars are escaped.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Default secrets file path.
pub const DEFAULT_SECRETS_PATH: &str = "/run/secrets/platform.env";

/// Maximum key length in bytes.
pub const MAX_KEY_LENGTH: usize = 256;

/// Maximum value length in bytes.
pub const MAX_VALUE_LENGTH: usize = 64 * 1024; // 64 KiB

/// Format version header.
const FORMAT_HEADER: &str = "# plfm-secrets v1";

/// Secrets format errors.
#[derive(Debug, Error)]
pub enum SecretsError {
    /// Invalid key format.
    #[error("invalid key '{key}': {reason}")]
    InvalidKey { key: String, reason: String },

    /// Invalid value format.
    #[error("invalid value for key '{key}': {reason}")]
    InvalidValue { key: String, reason: String },

    /// Parse error.
    #[error("parse error at line {line}: {reason}")]
    ParseError { line: usize, reason: String },

    /// Unsupported format version.
    #[error("unsupported format version: {version}")]
    UnsupportedVersion { version: String },

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// A collection of secrets (key-value pairs).
///
/// Keys are stored in sorted order for deterministic serialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    /// Secrets stored in sorted order.
    inner: BTreeMap<String, String>,
}

impl Secrets {
    /// Create an empty secrets collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create from an iterator of key-value pairs.
    pub fn try_from_iter<I, K, V>(iter: I) -> Result<Self, SecretsError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut secrets = Self::new();
        for (k, v) in iter {
            secrets.set(k, v)?;
        }
        Ok(secrets)
    }

    /// Set a secret value.
    ///
    /// Returns the previous value if the key existed.
    pub fn set<K: Into<String>, V: Into<String>>(
        &mut self,
        key: K,
        value: V,
    ) -> Result<Option<String>, SecretsError> {
        let key = key.into();
        let value = value.into();

        validate_key(&key)?;
        validate_value(&key, &value)?;

        Ok(self.inner.insert(key, value))
    }

    /// Get a secret value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(|s| s.as_str())
    }

    /// Remove a secret.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.inner.remove(key)
    }

    /// Check if a key exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    /// Get the number of secrets.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over key-value pairs in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.inner.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Get all keys in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.inner.keys().map(|k| k.as_str())
    }

    /// Serialize to canonical dotenv format.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str(FORMAT_HEADER);
        out.push('\n');

        for (key, value) in &self.inner {
            out.push_str(key);
            out.push('=');
            out.push_str(&escape_value(value));
            out.push('\n');
        }

        out
    }

    /// Compute the SHA-256 hash of the canonical representation.
    pub fn data_hash(&self) -> String {
        let content = self.serialize();
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let result = hasher.finalize();
        format!("sha256:{}", hex::encode(result))
    }

    /// Parse from dotenv format.
    pub fn parse(content: &str) -> Result<Self, SecretsError> {
        let mut secrets = Self::new();
        let mut lines = content.lines().enumerate();

        // Check header
        if let Some((line_num, first_line)) = lines.next() {
            let first_line = first_line.trim();
            if first_line.starts_with("# plfm-secrets") {
                // Validate version
                if !first_line.starts_with("# plfm-secrets v1") {
                    let version = first_line
                        .strip_prefix("# plfm-secrets ")
                        .unwrap_or("unknown");
                    return Err(SecretsError::UnsupportedVersion {
                        version: version.to_string(),
                    });
                }
            } else if !first_line.is_empty() && !first_line.starts_with('#') {
                // No header, parse as key=value
                parse_line(line_num + 1, first_line, &mut secrets)?;
            }
        }

        // Parse remaining lines
        for (line_num, line) in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            parse_line(line_num + 1, line, &mut secrets)?;
        }

        Ok(secrets)
    }

    /// Read from a file.
    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<Self, SecretsError> {
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Write to a file atomically with secure permissions.
    ///
    /// Uses write-to-temp + fsync + rename for atomicity.
    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), SecretsError> {
        let path = path.as_ref();
        let content = self.serialize();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to temp file in same directory (for atomic rename)
        let temp_path = path.with_extension("tmp");

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o400) // Read-only by owner
                .open(&temp_path)?;

            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        // Atomic rename
        fs::rename(&temp_path, path)?;

        Ok(())
    }
}

/// Validate a key.
fn validate_key(key: &str) -> Result<(), SecretsError> {
    if key.is_empty() {
        return Err(SecretsError::InvalidKey {
            key: key.to_string(),
            reason: "key cannot be empty".to_string(),
        });
    }

    if key.len() > MAX_KEY_LENGTH {
        return Err(SecretsError::InvalidKey {
            key: key.to_string(),
            reason: format!("key exceeds maximum length of {} bytes", MAX_KEY_LENGTH),
        });
    }

    let mut chars = key.chars();
    let first = chars.next().unwrap();

    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(SecretsError::InvalidKey {
            key: key.to_string(),
            reason: "key must start with a letter or underscore".to_string(),
        });
    }

    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return Err(SecretsError::InvalidKey {
                key: key.to_string(),
                reason: format!("invalid character '{}' in key", c),
            });
        }
    }

    Ok(())
}

/// Validate a value.
fn validate_value(key: &str, value: &str) -> Result<(), SecretsError> {
    if value.len() > MAX_VALUE_LENGTH {
        return Err(SecretsError::InvalidValue {
            key: key.to_string(),
            reason: format!("value exceeds maximum length of {} bytes", MAX_VALUE_LENGTH),
        });
    }

    // Values must be valid UTF-8 (already enforced by String type)
    Ok(())
}

/// Escape a value for dotenv format.
///
/// Escapes newlines, carriage returns, and backslashes.
fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// Unescape a value from dotenv format.
fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }

    out
}

/// Parse a single line.
fn parse_line(line_num: usize, line: &str, secrets: &mut Secrets) -> Result<(), SecretsError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(SecretsError::ParseError {
            line: line_num,
            reason: "expected KEY=value format".to_string(),
        });
    };

    let key = key.trim();
    let value = unescape_value(value);

    secrets
        .set(key, value)
        .map_err(|e| SecretsError::ParseError {
            line: line_num,
            reason: e.to_string(),
        })?;

    Ok(())
}

/// Redact a secrets collection for logging/display.
///
/// Returns a map with all values replaced by `[REDACTED]`.
pub fn redact_for_display(secrets: &Secrets) -> BTreeMap<String, String> {
    secrets
        .keys()
        .map(|k| (k.to_string(), "[REDACTED]".to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_validation() {
        assert!(validate_key("FOO").is_ok());
        assert!(validate_key("foo_bar").is_ok());
        assert!(validate_key("_private").is_ok());
        assert!(validate_key("FOO123").is_ok());

        assert!(validate_key("").is_err());
        assert!(validate_key("123foo").is_err());
        assert!(validate_key("foo-bar").is_err());
        assert!(validate_key("foo.bar").is_err());
    }

    #[test]
    fn test_escape_unescape() {
        assert_eq!(escape_value("hello"), "hello");
        assert_eq!(escape_value("hello\nworld"), "hello\\nworld");
        assert_eq!(escape_value("path\\to\\file"), "path\\\\to\\\\file");

        assert_eq!(unescape_value("hello"), "hello");
        assert_eq!(unescape_value("hello\\nworld"), "hello\nworld");
        assert_eq!(unescape_value("path\\\\to\\\\file"), "path\\to\\file");
    }

    #[test]
    fn test_roundtrip() {
        let mut secrets = Secrets::new();
        secrets.set("FOO", "bar").unwrap();
        secrets.set("MULTI_LINE", "line1\nline2").unwrap();
        secrets.set("WITH_BACKSLASH", "path\\to\\file").unwrap();

        let serialized = secrets.serialize();
        let parsed = Secrets::parse(&serialized).unwrap();

        assert_eq!(secrets, parsed);
    }

    #[test]
    fn test_data_hash_deterministic() {
        let mut s1 = Secrets::new();
        s1.set("B", "2").unwrap();
        s1.set("A", "1").unwrap();

        let mut s2 = Secrets::new();
        s2.set("A", "1").unwrap();
        s2.set("B", "2").unwrap();

        // Same content, same hash regardless of insertion order
        assert_eq!(s1.data_hash(), s2.data_hash());
    }

    #[test]
    fn test_parse_with_header() {
        let content = "# plfm-secrets v1\nFOO=bar\nBAZ=qux\n";
        let secrets = Secrets::parse(content).unwrap();
        assert_eq!(secrets.get("FOO"), Some("bar"));
        assert_eq!(secrets.get("BAZ"), Some("qux"));
    }

    #[test]
    fn test_parse_without_header() {
        let content = "FOO=bar\nBAZ=qux\n";
        let secrets = Secrets::parse(content).unwrap();
        assert_eq!(secrets.get("FOO"), Some("bar"));
        assert_eq!(secrets.get("BAZ"), Some("qux"));
    }

    #[test]
    fn test_unsupported_version() {
        let content = "# plfm-secrets v999\nFOO=bar\n";
        let result = Secrets::parse(content);
        assert!(matches!(
            result,
            Err(SecretsError::UnsupportedVersion { .. })
        ));
    }
}
