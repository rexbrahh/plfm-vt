//! Reconciliation loop primitives.
//!
//! This library provides helpers for implementing reconciliation loops
//! that converge desired state to current state. Key concepts:
//!
//! - **Desired state**: What the system should look like (from API/events).
//! - **Current state**: What the system actually looks like (from agents).
//! - **Convergence**: The process of making current match desired.
//!
//! # Invariants
//!
//! - All operations are idempotent
//! - Decisions are deterministic given the same inputs
//! - State changes are monotonic (version always increases)

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Reconciliation errors.
#[derive(Debug, Error)]
pub enum ReconcileError {
    /// Timeout waiting for convergence.
    #[error("timeout after {elapsed:?} waiting for {resource}")]
    Timeout {
        resource: String,
        elapsed: Duration,
    },

    /// Resource not found.
    #[error("resource not found: {0}")]
    NotFound(String),

    /// Conflict detected (concurrent modification).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convergence status for a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceStatus {
    /// Resource has converged (current matches desired).
    Converged,

    /// Resource is converging (current is moving toward desired).
    Converging,

    /// Resource has diverged (requires intervention).
    Diverged,

    /// Status is unknown (insufficient data).
    Unknown,
}

impl ConvergenceStatus {
    /// Returns true if the resource has converged.
    pub fn is_converged(&self) -> bool {
        matches!(self, Self::Converged)
    }

    /// Returns true if the resource is still converging.
    pub fn is_converging(&self) -> bool {
        matches!(self, Self::Converging)
    }
}

/// A spec hash for deterministic comparison.
///
/// Used to detect when instance configuration has changed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpecHash(String);

impl SpecHash {
    /// Compute a spec hash from canonical JSON.
    pub fn from_json(json: &serde_json::Value) -> Self {
        let canonical = canonical_json(json);
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        let result = hasher.finalize();
        Self(format!("sha256:{}", hex::encode(&result[..16]))) // First 16 bytes (128 bits)
    }

    /// Get the hash string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SpecHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Produce canonical JSON (sorted keys, no extra whitespace).
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut pairs: Vec<_> = map.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("\"{}\":{}", escape_json_string(k), canonical_json(v)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        serde_json::Value::String(s) => format!("\"{}\"", escape_json_string(s)),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Instance classification based on spec hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceClass {
    /// Instance matches the current desired spec.
    Matching,

    /// Instance has an old spec (needs replacement).
    Old,
}

/// Classify instances based on their spec hash.
pub fn classify_instances<I, F>(
    instances: I,
    desired_spec_hash: &SpecHash,
    get_spec_hash: F,
) -> (Vec<I::Item>, Vec<I::Item>)
where
    I: IntoIterator,
    F: Fn(&I::Item) -> &SpecHash,
{
    let mut matching = Vec::new();
    let mut old = Vec::new();

    for instance in instances {
        if get_spec_hash(&instance) == desired_spec_hash {
            matching.push(instance);
        } else {
            old.push(instance);
        }
    }

    (matching, old)
}

/// Rollout strategy for stateless workloads.
#[derive(Debug, Clone)]
pub struct RollingStrategy {
    /// Maximum number of instances that can be created above desired count.
    pub max_surge: u32,

    /// Maximum number of instances that can be unavailable during rollout.
    pub max_unavailable: u32,
}

impl Default for RollingStrategy {
    fn default() -> Self {
        Self {
            max_surge: 1,
            max_unavailable: 0,
        }
    }
}

impl RollingStrategy {
    /// Calculate how many instances can be started in this reconciliation pass.
    ///
    /// Returns (new_to_start, old_to_drain).
    pub fn calculate_actions(
        &self,
        desired_count: u32,
        matching_ready: u32,
        matching_pending: u32,
        old_running: u32,
    ) -> (u32, u32) {
        let total_running = matching_ready + matching_pending + old_running;

        // How many new instances can we start?
        let max_total = desired_count + self.max_surge;
        let can_start = max_total.saturating_sub(total_running);

        // How many do we need to start?
        let need_to_start = desired_count.saturating_sub(matching_ready + matching_pending);
        let new_to_start = can_start.min(need_to_start);

        // How many old instances can we drain?
        let min_available = desired_count.saturating_sub(self.max_unavailable);
        let currently_available = matching_ready;
        let can_drain = currently_available.saturating_sub(min_available);
        let old_to_drain = can_drain.min(old_running);

        (new_to_start, old_to_drain)
    }
}

/// Drain selection priority for instances.
///
/// Lower priority values are drained first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrainPriority {
    /// Instance has failed (drain first).
    Failed = 0,

    /// Instance is not ready.
    NotReady = 1,

    /// Instance is oldest.
    Oldest = 2,

    /// Instance is most loaded.
    MostLoaded = 3,

    /// Instance is healthy and recent (drain last).
    Healthy = 4,
}

/// Select instances to drain based on priority.
///
/// Returns instances sorted by drain priority (first to drain first).
pub fn select_for_drain<T, F>(instances: Vec<T>, get_priority: F) -> Vec<T>
where
    F: Fn(&T) -> DrainPriority,
{
    let mut with_priority: Vec<_> = instances
        .into_iter()
        .map(|i| {
            let p = get_priority(&i);
            (p, i)
        })
        .collect();

    with_priority.sort_by_key(|(p, _)| *p);
    with_priority.into_iter().map(|(_, i)| i).collect()
}

/// Checkpoint for projection state.
///
/// Tracks the last processed event for exactly-once semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Last applied event ID.
    pub last_event_id: i64,

    /// Timestamp of last checkpoint update.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Checkpoint {
    /// Create a new checkpoint.
    pub fn new(last_event_id: i64) -> Self {
        Self {
            last_event_id,
            updated_at: chrono::Utc::now(),
        }
    }

    /// Check if an event has already been processed.
    pub fn is_processed(&self, event_id: i64) -> bool {
        event_id <= self.last_event_id
    }

    /// Advance the checkpoint to a new event.
    pub fn advance(&mut self, event_id: i64) {
        if event_id > self.last_event_id {
            self.last_event_id = event_id;
            self.updated_at = chrono::Utc::now();
        }
    }
}

/// Retry tracker for failed operations.
#[derive(Debug, Clone)]
pub struct RetryTracker {
    /// Maximum retries per resource.
    max_retries: u32,

    /// Retry window duration.
    window: Duration,

    /// Tracked failures: resource_key -> (count, first_failure_time).
    failures: BTreeMap<String, (u32, Instant)>,
}

impl RetryTracker {
    /// Create a new retry tracker.
    pub fn new(max_retries: u32, window: Duration) -> Self {
        Self {
            max_retries,
            window,
            failures: BTreeMap::new(),
        }
    }

    /// Record a failure for a resource.
    ///
    /// Returns true if retries are exhausted.
    pub fn record_failure(&mut self, resource_key: &str) -> bool {
        let now = Instant::now();

        let (count, first) = self
            .failures
            .entry(resource_key.to_string())
            .or_insert((0, now));

        // Reset if outside window
        if now.duration_since(*first) > self.window {
            *count = 0;
            *first = now;
        }

        *count += 1;
        *count > self.max_retries
    }

    /// Check if retries are exhausted for a resource.
    pub fn is_exhausted(&self, resource_key: &str) -> bool {
        let Some((count, first)) = self.failures.get(resource_key) else {
            return false;
        };

        let now = Instant::now();
        if now.duration_since(*first) > self.window {
            return false;
        }

        *count > self.max_retries
    }

    /// Clear failure tracking for a resource (on success).
    pub fn clear(&mut self, resource_key: &str) {
        self.failures.remove(resource_key);
    }

    /// Prune expired entries.
    pub fn prune(&mut self) {
        let now = Instant::now();
        self.failures
            .retain(|_, (_, first)| now.duration_since(*first) <= self.window);
    }
}

/// Default reconciliation interval.
pub const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Default retry limit per group per deploy.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default retry window.
pub const DEFAULT_RETRY_WINDOW: Duration = Duration::from_secs(10 * 60); // 10 minutes

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_hash_deterministic() {
        let json1 = serde_json::json!({"b": 2, "a": 1});
        let json2 = serde_json::json!({"a": 1, "b": 2});

        let hash1 = SpecHash::from_json(&json1);
        let hash2 = SpecHash::from_json(&json2);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_rolling_strategy() {
        let strategy = RollingStrategy {
            max_surge: 1,
            max_unavailable: 0,
        };

        // Starting fresh: can start 1 (max_surge allows desired + 1)
        let (start, drain) = strategy.calculate_actions(3, 0, 0, 0);
        assert_eq!(start, 3); // Start all since we have room
        assert_eq!(drain, 0);

        // 2 ready, 1 pending, 0 old: don't start more, don't drain
        let (start, drain) = strategy.calculate_actions(3, 2, 1, 0);
        assert_eq!(start, 0);
        assert_eq!(drain, 0);

        // 3 ready, 0 pending, 2 old: can drain old now
        let (start, drain) = strategy.calculate_actions(3, 3, 0, 2);
        assert_eq!(start, 0);
        assert_eq!(drain, 2); // All old can be drained since we have 3 ready
    }

    #[test]
    fn test_classify_instances() {
        let desired = SpecHash("sha256:abc".to_string());
        let instances = vec![
            ("i1", SpecHash("sha256:abc".to_string())),
            ("i2", SpecHash("sha256:old".to_string())),
            ("i3", SpecHash("sha256:abc".to_string())),
        ];

        let (matching, old) = classify_instances(instances, &desired, |(_, h)| h);

        assert_eq!(matching.len(), 2);
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].0, "i2");
    }

    #[test]
    fn test_checkpoint() {
        let mut cp = Checkpoint::new(100);

        assert!(cp.is_processed(50));
        assert!(cp.is_processed(100));
        assert!(!cp.is_processed(101));

        cp.advance(150);
        assert!(cp.is_processed(150));
        assert!(!cp.is_processed(151));
    }

    #[test]
    fn test_retry_tracker() {
        let mut tracker = RetryTracker::new(3, Duration::from_secs(60));

        assert!(!tracker.record_failure("resource-1")); // 1st
        assert!(!tracker.record_failure("resource-1")); // 2nd
        assert!(!tracker.record_failure("resource-1")); // 3rd
        assert!(tracker.record_failure("resource-1")); // 4th - exhausted

        assert!(tracker.is_exhausted("resource-1"));
        assert!(!tracker.is_exhausted("resource-2"));

        tracker.clear("resource-1");
        assert!(!tracker.is_exhausted("resource-1"));
    }
}
