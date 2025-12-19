//! Event type definitions for all platform events.
//!
//! Each event type has a corresponding payload struct with the event-specific data.
//! Events are versioned for schema evolution.

use plfm_id::{
    AppId, AssignmentId, DeployId, EnvId, ExecSessionId, InstanceId, MemberId, NodeId, OrgId,
    ReleaseId, RestoreJobId, RouteId, SecretBundleId, SecretVersionId, ServicePrincipalId,
    SnapshotId, VolumeAttachmentId, VolumeId,
};
use serde::{Deserialize, Serialize};

// =============================================================================
// Event Type Constants
// =============================================================================

/// All event type names as constants.
pub mod event_types {
    // Organization
    pub const ORG_CREATED: &str = "org.created";
    pub const ORG_UPDATED: &str = "org.updated";
    pub const ORG_MEMBER_ADDED: &str = "org_member.added";
    pub const ORG_MEMBER_ROLE_UPDATED: &str = "org_member.role_updated";
    pub const ORG_MEMBER_REMOVED: &str = "org_member.removed";

    // Service Principal
    pub const SERVICE_PRINCIPAL_CREATED: &str = "service_principal.created";
    pub const SERVICE_PRINCIPAL_SCOPES_UPDATED: &str = "service_principal.scopes_updated";
    pub const SERVICE_PRINCIPAL_SECRET_ROTATED: &str = "service_principal.secret_rotated";
    pub const SERVICE_PRINCIPAL_DELETED: &str = "service_principal.deleted";

    // Application
    pub const APP_CREATED: &str = "app.created";
    pub const APP_UPDATED: &str = "app.updated";
    pub const APP_DELETED: &str = "app.deleted";

    // Environment
    pub const ENV_CREATED: &str = "env.created";
    pub const ENV_UPDATED: &str = "env.updated";
    pub const ENV_DELETED: &str = "env.deleted";
    pub const ENV_SCALE_SET: &str = "env.scale_set";
    pub const ENV_DESIRED_RELEASE_SET: &str = "env.desired_release_set";
    pub const ENV_IPV4_ADDON_ENABLED: &str = "env.ipv4_addon_enabled";
    pub const ENV_IPV4_ADDON_DISABLED: &str = "env.ipv4_addon_disabled";

    // Release
    pub const RELEASE_CREATED: &str = "release.created";

    // Deploy
    pub const DEPLOY_CREATED: &str = "deploy.created";
    pub const DEPLOY_STATUS_CHANGED: &str = "deploy.status_changed";

    // Route
    pub const ROUTE_CREATED: &str = "route.created";
    pub const ROUTE_UPDATED: &str = "route.updated";
    pub const ROUTE_DELETED: &str = "route.deleted";

    // Secret Bundle
    pub const SECRET_BUNDLE_CREATED: &str = "secret_bundle.created";
    pub const SECRET_BUNDLE_VERSION_SET: &str = "secret_bundle.version_set";

    // Volume
    pub const VOLUME_CREATED: &str = "volume.created";
    pub const VOLUME_DELETED: &str = "volume.deleted";
    pub const VOLUME_ATTACHMENT_CREATED: &str = "volume_attachment.created";
    pub const VOLUME_ATTACHMENT_DELETED: &str = "volume_attachment.deleted";

    // Snapshot
    pub const SNAPSHOT_CREATED: &str = "snapshot.created";
    pub const SNAPSHOT_STATUS_CHANGED: &str = "snapshot.status_changed";

    // Restore Job
    pub const RESTORE_JOB_CREATED: &str = "restore_job.created";
    pub const RESTORE_JOB_STATUS_CHANGED: &str = "restore_job.status_changed";

    // Instance
    pub const INSTANCE_ALLOCATED: &str = "instance.allocated";
    pub const INSTANCE_DESIRED_STATE_CHANGED: &str = "instance.desired_state_changed";
    pub const INSTANCE_STATUS_CHANGED: &str = "instance.status_changed";

    // Node
    pub const NODE_ENROLLED: &str = "node.enrolled";
    pub const NODE_STATE_CHANGED: &str = "node.state_changed";
    pub const NODE_CAPACITY_UPDATED: &str = "node.capacity_updated";

    // Exec Session
    pub const EXEC_SESSION_GRANTED: &str = "exec_session.granted";
    pub const EXEC_SESSION_CONNECTED: &str = "exec_session.connected";
    pub const EXEC_SESSION_ENDED: &str = "exec_session.ended";
}

// =============================================================================
// Status Enums
// =============================================================================

/// Deploy status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    Queued,
    Rolling,
    Succeeded,
    Failed,
}

/// Instance desired state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceDesiredState {
    Running,
    Draining,
    Stopped,
}

/// Instance actual status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    Booting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

/// Node state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Active,
    Draining,
    Disabled,
    Degraded,
    Offline,
}

/// Snapshot/restore job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

/// Instance failure reason codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceFailureReason {
    ImagePullFailed,
    RootfsBuildFailed,
    FirecrackerStartFailed,
    NetworkSetupFailed,
    VolumeAttachFailed,
    SecretsMissing,
    SecretsInjectionFailed,
    HealthcheckFailed,
    OomKilled,
    CrashLoopBackoff,
    TerminatedByOperator,
    NodeDraining,
}

/// Organization member role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    Owner,
    Admin,
    Developer,
    Readonly,
}

// =============================================================================
// Route Enums
// =============================================================================

/// Protocol hint for edge routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProtocolHint {
    TlsPassthrough,
    TcpRaw,
}

/// Proxy Protocol mode for edge -> backend connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProxyProtocol {
    Off,
    V2,
}

impl Default for RouteProxyProtocol {
    fn default() -> Self {
        Self::Off
    }
}

// =============================================================================
// Event Payloads
// =============================================================================

// -----------------------------------------------------------------------------
// Organization Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgCreatedPayload {
    pub org_id: OrgId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgUpdatedPayload {
    pub org_id: OrgId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMemberAddedPayload {
    pub member_id: MemberId,
    pub org_id: OrgId,
    pub email: String,
    pub role: MemberRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMemberRoleUpdatedPayload {
    pub member_id: MemberId,
    pub org_id: OrgId,
    pub old_role: MemberRole,
    pub new_role: MemberRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMemberRemovedPayload {
    pub member_id: MemberId,
    pub org_id: OrgId,
    pub email: String,
}

// -----------------------------------------------------------------------------
// Service Principal Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePrincipalCreatedPayload {
    pub sp_id: ServicePrincipalId,
    pub org_id: OrgId,
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePrincipalScopesUpdatedPayload {
    pub sp_id: ServicePrincipalId,
    pub old_scopes: Vec<String>,
    pub new_scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePrincipalSecretRotatedPayload {
    pub sp_id: ServicePrincipalId,
    // Note: Never include the actual secret value!
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePrincipalDeletedPayload {
    pub sp_id: ServicePrincipalId,
}

// -----------------------------------------------------------------------------
// Application Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppCreatedPayload {
    pub app_id: AppId,
    pub org_id: OrgId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppUpdatedPayload {
    pub app_id: AppId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDeletedPayload {
    pub app_id: AppId,
}

// -----------------------------------------------------------------------------
// Environment Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCreatedPayload {
    pub env_id: EnvId,
    pub app_id: AppId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvUpdatedPayload {
    pub env_id: EnvId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDeletedPayload {
    pub env_id: EnvId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvScaleSetPayload {
    pub env_id: EnvId,
    pub process_type: String,
    pub min_replicas: i32,
    pub max_replicas: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDesiredReleaseSetPayload {
    pub env_id: EnvId,
    pub release_id: ReleaseId,
    pub deploy_id: DeployId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvIpv4AddonEnabledPayload {
    pub env_id: EnvId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvIpv4AddonDisabledPayload {
    pub env_id: EnvId,
}

// -----------------------------------------------------------------------------
// Release Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCreatedPayload {
    pub release_id: ReleaseId,
    pub app_id: AppId,
    pub image_digest: String,
    pub manifest_hash: String,
}

// -----------------------------------------------------------------------------
// Deploy Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployCreatedPayload {
    pub deploy_id: DeployId,
    pub env_id: EnvId,
    pub release_id: ReleaseId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_rollback: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployStatusChangedPayload {
    pub deploy_id: DeployId,
    pub old_status: DeployStatus,
    pub new_status: DeployStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

// -----------------------------------------------------------------------------
// Route Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCreatedPayload {
    pub route_id: RouteId,
    pub org_id: OrgId,
    pub app_id: AppId,
    pub env_id: EnvId,
    pub hostname: String,
    pub listen_port: i32,
    pub protocol_hint: RouteProtocolHint,
    pub backend_process_type: String,
    pub backend_port: i32,
    pub proxy_protocol: RouteProxyProtocol,
    pub backend_expects_proxy_protocol: bool,
    pub ipv4_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteUpdatedPayload {
    pub route_id: RouteId,
    pub org_id: OrgId,
    pub env_id: EnvId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_process_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_protocol: Option<RouteProxyProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_expects_proxy_protocol: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4_required: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDeletedPayload {
    pub route_id: RouteId,
    pub org_id: OrgId,
    pub env_id: EnvId,
    pub hostname: String,
}

// -----------------------------------------------------------------------------
// Secret Bundle Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretBundleCreatedPayload {
    pub bundle_id: SecretBundleId,
    pub org_id: OrgId,
    pub app_id: AppId,
    pub env_id: EnvId,
    pub format: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretBundleVersionSetPayload {
    pub bundle_id: SecretBundleId,
    pub org_id: OrgId,
    pub env_id: EnvId,
    pub version_id: SecretVersionId,
    pub format: String,
    pub data_hash: String,
    pub updated_at: String,
}

// -----------------------------------------------------------------------------
// Volume Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeCreatedPayload {
    pub volume_id: VolumeId,
    pub org_id: OrgId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub size_bytes: i64,
    pub filesystem: String,
    pub backup_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeDeletedPayload {
    pub volume_id: VolumeId,
    pub org_id: OrgId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeAttachmentCreatedPayload {
    pub attachment_id: VolumeAttachmentId,
    pub org_id: OrgId,
    pub volume_id: VolumeId,
    pub app_id: AppId,
    pub env_id: EnvId,
    pub process_type: String,
    pub mount_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeAttachmentDeletedPayload {
    pub attachment_id: VolumeAttachmentId,
    pub org_id: OrgId,
    pub volume_id: VolumeId,
    pub env_id: EnvId,
    pub process_type: String,
}

// -----------------------------------------------------------------------------
// Snapshot Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotCreatedPayload {
    pub snapshot_id: SnapshotId,
    pub org_id: OrgId,
    pub volume_id: VolumeId,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStatusChangedPayload {
    pub snapshot_id: SnapshotId,
    pub org_id: OrgId,
    pub volume_id: VolumeId,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
}

// -----------------------------------------------------------------------------
// Restore Job Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreJobCreatedPayload {
    pub restore_id: RestoreJobId,
    pub org_id: OrgId,
    pub snapshot_id: SnapshotId,
    pub source_volume_id: VolumeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_volume_name: Option<String>,
    pub status: JobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreJobStatusChangedPayload {
    pub restore_id: RestoreJobId,
    pub org_id: OrgId,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_volume_id: Option<VolumeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,
}

// -----------------------------------------------------------------------------
// Instance Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceAllocatedPayload {
    pub instance_id: InstanceId,
    pub assignment_id: AssignmentId,
    pub env_id: EnvId,
    pub process_type: String,
    pub node_id: NodeId,
    pub release_id: ReleaseId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceDesiredStateChangedPayload {
    pub instance_id: InstanceId,
    pub old_state: InstanceDesiredState,
    pub new_state: InstanceDesiredState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceStatusChangedPayload {
    pub instance_id: InstanceId,
    pub old_status: InstanceStatus,
    pub new_status: InstanceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<InstanceFailureReason>,
}

// -----------------------------------------------------------------------------
// Node Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEnrolledPayload {
    pub node_id: NodeId,
    pub hostname: String,
    pub region: String,
    pub cpu_cores: i32,
    pub memory_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStateChangedPayload {
    pub node_id: NodeId,
    pub old_state: NodeState,
    pub new_state: NodeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapacityUpdatedPayload {
    pub node_id: NodeId,
    pub available_cpu_cores: i32,
    pub available_memory_bytes: i64,
    pub instance_count: i32,
}

// -----------------------------------------------------------------------------
// Exec Session Events
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecSessionGrantedPayload {
    pub exec_session_id: ExecSessionId,
    pub org_id: OrgId,
    pub app_id: AppId,
    pub env_id: EnvId,
    pub instance_id: InstanceId,
    pub requested_command: Vec<String>,
    pub tty: bool,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecSessionConnectedPayload {
    pub exec_session_id: ExecSessionId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub connected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecSessionEndedPayload {
    pub exec_session_id: ExecSessionId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
    pub ended_at: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_status_serialization() {
        assert_eq!(
            serde_json::to_string(&DeployStatus::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(
            serde_json::to_string(&DeployStatus::Rolling).unwrap(),
            "\"rolling\""
        );
    }

    #[test]
    fn test_instance_failure_reason_serialization() {
        assert_eq!(
            serde_json::to_string(&InstanceFailureReason::OomKilled).unwrap(),
            "\"oom_killed\""
        );
        assert_eq!(
            serde_json::to_string(&InstanceFailureReason::CrashLoopBackoff).unwrap(),
            "\"crash_loop_backoff\""
        );
    }

    #[test]
    fn test_org_created_payload() {
        let payload = OrgCreatedPayload {
            org_id: OrgId::new(),
            name: "Acme Corp".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: OrgCreatedPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.name, parsed.name);
    }

    #[test]
    fn test_instance_status_changed_payload() {
        let payload = InstanceStatusChangedPayload {
            instance_id: InstanceId::new(),
            old_status: InstanceStatus::Booting,
            new_status: InstanceStatus::Failed,
            failure_reason: Some(InstanceFailureReason::HealthcheckFailed),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"healthcheck_failed\""));
    }

    #[test]
    fn test_node_state_values() {
        // Verify all node states can be serialized
        let states = vec![
            NodeState::Active,
            NodeState::Draining,
            NodeState::Disabled,
            NodeState::Degraded,
            NodeState::Offline,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: NodeState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, parsed);
        }
    }
}
