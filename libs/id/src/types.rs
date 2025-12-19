//! Typed ID definitions for all platform resources.
//!
//! Each ID type has a unique prefix that identifies the resource type.
//! IDs are ULID-based for sortability and uniqueness.

use crate::define_id;

// =============================================================================
// Organization and Membership
// =============================================================================

define_id!(OrgId, "org");
define_id!(ProjectId, "prj");
define_id!(MemberId, "mem");
define_id!(ServicePrincipalId, "sp");

// =============================================================================
// Application Model
// =============================================================================

define_id!(AppId, "app");
define_id!(EnvId, "env");
define_id!(ReleaseId, "rel");
define_id!(DeployId, "dep");

// =============================================================================
// Runtime and Instances
// =============================================================================

define_id!(InstanceId, "inst");
define_id!(BootId, "boot");
define_id!(NodeId, "node");
define_id!(AssignmentId, "asgn");

// =============================================================================
// Networking
// =============================================================================

define_id!(RouteId, "rt");
define_id!(EndpointId, "ep");

// =============================================================================
// Storage
// =============================================================================

define_id!(VolumeId, "vol");
define_id!(VolumeAttachmentId, "vat");
define_id!(SnapshotId, "snap");
define_id!(RestoreJobId, "rjob");

// =============================================================================
// Secrets
// =============================================================================

define_id!(SecretBundleId, "sb");
define_id!(SecretVersionId, "sv");

// =============================================================================
// Sessions and Requests
// =============================================================================

define_id!(ExecSessionId, "exec");
define_id!(RequestId, "req");

// =============================================================================
// Events
// =============================================================================

/// Event ID is a simple monotonic integer, not ULID-based.
/// This is handled separately from the typed IDs above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(i64);

impl EventId {
    /// Creates a new EventId from an i64.
    #[must_use]
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    /// Returns the underlying i64 value.
    #[must_use]
    pub const fn value(&self) -> i64 {
        self.0
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<i64> for EventId {
    fn from(id: i64) -> Self {
        Self(id)
    }
}

impl From<EventId> for i64 {
    fn from(id: EventId) -> Self {
        id.0
    }
}

impl serde::Serialize for EventId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for EventId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = i64::deserialize(deserializer)?;
        Ok(Self(id))
    }
}

// =============================================================================
// Aggregate Sequence Number
// =============================================================================

/// Aggregate sequence number for event ordering within an aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AggregateSeq(i32);

impl AggregateSeq {
    /// The first sequence number for a new aggregate.
    pub const FIRST: Self = Self(1);

    /// Creates a new AggregateSeq from an i32.
    #[must_use]
    pub const fn new(seq: i32) -> Self {
        Self(seq)
    }

    /// Returns the underlying i32 value.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.0
    }

    /// Returns the next sequence number.
    #[must_use]
    pub const fn next(&self) -> Self {
        Self(self.0 + 1)
    }
}

impl Default for AggregateSeq {
    fn default() -> Self {
        Self::FIRST
    }
}

impl std::fmt::Display for AggregateSeq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<i32> for AggregateSeq {
    fn from(seq: i32) -> Self {
        Self(seq)
    }
}

impl From<AggregateSeq> for i32 {
    fn from(seq: AggregateSeq) -> Self {
        seq.0
    }
}

impl serde::Serialize for AggregateSeq {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_i32(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for AggregateSeq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let seq = i32::deserialize(deserializer)?;
        Ok(Self(seq))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_org_id_roundtrip() {
        let id = OrgId::new();
        let s = id.to_string();
        let parsed: OrgId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_org_id_prefix() {
        let id = OrgId::new();
        let s = id.to_string();
        assert!(s.starts_with("org_"));
    }

    #[test]
    fn test_org_id_invalid_prefix() {
        let result: Result<OrgId, _> = "app_01HV4Z2WQXKJNM8GPQY6VBKC3D".parse();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::IdError::InvalidPrefix { .. }
        ));
    }

    #[test]
    fn test_org_id_missing_separator() {
        let result: Result<OrgId, _> = "org01HV4Z2WQXKJNM8GPQY6VBKC3D".parse();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::IdError::MissingSeparator
        ));
    }

    #[test]
    fn test_org_id_empty() {
        let result: Result<OrgId, _> = "".parse();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), crate::IdError::Empty));
    }

    #[test]
    fn test_org_id_invalid_ulid() {
        let result: Result<OrgId, _> = "org_invalid".parse();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::IdError::InvalidUlid(_)
        ));
    }

    #[test]
    fn test_org_id_json_roundtrip() {
        let id = OrgId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: OrgId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_instance_id_sortable() {
        let id1 = InstanceId::new();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let id2 = InstanceId::new();
        // ULIDs are time-ordered, so id1 < id2
        assert!(id1 < id2);
    }

    #[test]
    fn test_event_id_roundtrip() {
        let id = EventId::new(12345);
        let json = serde_json::to_string(&id).unwrap();
        let parsed: EventId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_aggregate_seq_next() {
        let seq = AggregateSeq::FIRST;
        assert_eq!(seq.value(), 1);
        let next = seq.next();
        assert_eq!(next.value(), 2);
    }

    #[test]
    fn test_all_id_prefixes_unique() {
        // Ensure all prefixes are unique
        let prefixes = vec![
            OrgId::PREFIX,
            ProjectId::PREFIX,
            MemberId::PREFIX,
            ServicePrincipalId::PREFIX,
            AppId::PREFIX,
            EnvId::PREFIX,
            ReleaseId::PREFIX,
            DeployId::PREFIX,
            InstanceId::PREFIX,
            BootId::PREFIX,
            NodeId::PREFIX,
            AssignmentId::PREFIX,
            RouteId::PREFIX,
            EndpointId::PREFIX,
            VolumeId::PREFIX,
            VolumeAttachmentId::PREFIX,
            SnapshotId::PREFIX,
            RestoreJobId::PREFIX,
            SecretBundleId::PREFIX,
            SecretVersionId::PREFIX,
            ExecSessionId::PREFIX,
            RequestId::PREFIX,
        ];

        let unique: std::collections::HashSet<_> = prefixes.iter().collect();
        assert_eq!(prefixes.len(), unique.len(), "Duplicate ID prefixes found!");
    }
}
