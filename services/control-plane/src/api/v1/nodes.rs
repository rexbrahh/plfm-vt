//! Node API endpoints.
//!
//! Provides endpoints for node enrollment, heartbeats, and plan delivery.
//! These are internal APIs called by node-agents, not tenant-facing.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType, NodeState};
use plfm_id::{NodeId, RequestId};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::api::error::ApiError;
use crate::db::AppendEvent;
use crate::state::AppState;

/// Create node routes.
///
/// Nodes are top-level infrastructure resources: /v1/nodes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/enroll", post(enroll_node))
        .route("/", get(list_nodes))
        .route("/{node_id}", get(get_node))
        .route("/{node_id}/heartbeat", post(heartbeat))
        .route("/{node_id}/plan", get(get_plan))
}

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to enroll a new node.
#[derive(Debug, Deserialize)]
pub struct EnrollNodeRequest {
    /// Hostname of the node.
    pub hostname: String,

    /// Region where the node is located.
    pub region: String,

    /// WireGuard public key for mesh networking.
    pub wireguard_public_key: String,

    /// mTLS subject for agent authentication.
    pub agent_mtls_subject: String,

    /// Public IPv6 address (required).
    pub public_ipv6: Ipv6Addr,

    /// Public IPv4 address (optional).
    #[serde(default)]
    pub public_ipv4: Option<Ipv4Addr>,

    /// Total CPU cores available.
    pub cpu_cores: i32,

    /// Total memory in bytes.
    pub memory_bytes: i64,

    /// MTU for network interfaces.
    #[serde(default)]
    pub mtu: Option<i32>,

    /// Labels for scheduling (region, zone, etc.).
    #[serde(default)]
    pub labels: serde_json::Value,
}

/// Response for a single node.
#[derive(Debug, Serialize)]
pub struct NodeResponse {
    /// Node ID.
    pub id: String,

    /// Node state.
    pub state: String,

    /// WireGuard public key.
    pub wireguard_public_key: String,

    /// mTLS subject.
    pub agent_mtls_subject: String,

    /// Public IPv6 address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ipv6: Option<String>,

    /// Public IPv4 address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ipv4: Option<String>,

    /// Labels for scheduling.
    pub labels: serde_json::Value,

    /// Allocatable resources.
    pub allocatable: serde_json::Value,

    /// MTU for network interfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<i32>,

    /// Resource version for optimistic concurrency.
    pub resource_version: i32,

    /// When the node was enrolled.
    pub created_at: DateTime<Utc>,

    /// When the node was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for listing nodes.
#[derive(Debug, Serialize)]
pub struct ListNodesResponse {
    /// List of nodes.
    pub items: Vec<NodeResponse>,

    /// Total count (for pagination).
    pub total: i64,
}

/// Request for node heartbeat.
#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    /// Current node state.
    pub state: NodeState,

    /// Available CPU cores (after allocations).
    pub available_cpu_cores: i32,

    /// Available memory in bytes (after allocations).
    pub available_memory_bytes: i64,

    /// Number of running instances.
    pub instance_count: i32,

    /// Instance statuses (instance_id -> status).
    #[serde(default)]
    pub instance_statuses: serde_json::Value,
}

/// Response for heartbeat.
#[derive(Debug, Serialize)]
pub struct HeartbeatResponse {
    /// Whether the heartbeat was accepted.
    pub accepted: bool,

    /// Next heartbeat interval in seconds.
    pub next_heartbeat_secs: i32,
}

/// Response for node plan (instances to run).
#[derive(Debug, Serialize)]
pub struct NodePlanResponse {
    /// Plan version (monotonically increasing).
    pub plan_version: i64,

    /// Instances assigned to this node.
    pub instances: Vec<InstancePlan>,
}

/// Plan for a single instance.
#[derive(Debug, Serialize)]
pub struct InstancePlan {
    /// Instance ID.
    pub instance_id: String,

    /// App ID.
    pub app_id: String,

    /// Env ID.
    pub env_id: String,

    /// Release ID to run.
    pub release_id: String,

    /// Deploy ID that triggered this instance.
    pub deploy_id: String,

    /// OCI image reference.
    pub image: String,

    /// Resource requests.
    pub resources: InstanceResources,

    /// Environment variables.
    #[serde(default)]
    pub env_vars: serde_json::Value,

    /// Volume mounts.
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
}

/// Resource requests for an instance.
#[derive(Debug, Serialize)]
pub struct InstanceResources {
    /// CPU cores (can be fractional).
    pub cpu: f64,

    /// Memory in bytes.
    pub memory_bytes: i64,
}

/// Volume mount specification.
#[derive(Debug, Serialize)]
pub struct VolumeMount {
    /// Volume ID.
    pub volume_id: String,

    /// Mount path inside the instance.
    pub mount_path: String,

    /// Whether the mount is read-only.
    pub read_only: bool,
}

// =============================================================================
// Handlers
// =============================================================================

/// Enroll a new node.
///
/// POST /v1/nodes/enroll
async fn enroll_node(
    State(state): State<AppState>,
    Json(req): Json<EnrollNodeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate hostname
    if req.hostname.is_empty() {
        return Err(ApiError::bad_request("invalid_hostname", "Hostname cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    if req.hostname.len() > 255 {
        return Err(ApiError::bad_request(
            "invalid_hostname",
            "Hostname cannot exceed 255 characters",
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate region
    if req.region.is_empty() {
        return Err(ApiError::bad_request("invalid_region", "Region cannot be empty")
            .with_request_id(request_id.to_string()));
    }

    // Validate WireGuard key (should be base64-encoded 32 bytes = 44 chars with padding)
    if req.wireguard_public_key.len() < 40 || req.wireguard_public_key.len() > 50 {
        return Err(ApiError::bad_request(
            "invalid_wireguard_key",
            "Invalid WireGuard public key format",
        )
        .with_request_id(request_id.to_string()));
    }

    // Validate resources
    if req.cpu_cores < 1 {
        return Err(ApiError::bad_request(
            "invalid_cpu_cores",
            "CPU cores must be at least 1",
        )
        .with_request_id(request_id.to_string()));
    }

    if req.memory_bytes < 1024 * 1024 * 512 {
        return Err(ApiError::bad_request(
            "invalid_memory",
            "Memory must be at least 512MB",
        )
        .with_request_id(request_id.to_string()));
    }

    // Check for duplicate WireGuard key
    let key_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE wireguard_public_key = $1)",
    )
    .bind(&req.wireguard_public_key)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check WireGuard key uniqueness");
        ApiError::internal("internal_error", "Failed to verify node")
            .with_request_id(request_id.to_string())
    })?;

    if key_exists {
        return Err(ApiError::conflict(
            "wireguard_key_exists",
            "A node with this WireGuard key is already enrolled",
        )
        .with_request_id(request_id.to_string()));
    }

    let node_id = NodeId::new();

    // Build allocatable resources
    let allocatable = serde_json::json!({
        "cpu_cores": req.cpu_cores,
        "memory_bytes": req.memory_bytes,
    });

    // Create the event
    let event = AppendEvent {
        aggregate_type: AggregateType::Node,
        aggregate_id: node_id.to_string(),
        aggregate_seq: 1,
        event_type: "node.enrolled".to_string(),
        event_version: 1,
        actor_type: ActorType::ServicePrincipal, // Node agents are service principals
        actor_id: node_id.to_string(),
        org_id: None,
        request_id: request_id.to_string(),
        idempotency_key: None,
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "node_id": node_id.to_string(),
            "hostname": req.hostname,
            "region": req.region,
            "wireguard_public_key": req.wireguard_public_key,
            "agent_mtls_subject": req.agent_mtls_subject,
            "public_ipv6": req.public_ipv6.to_string(),
            "public_ipv4": req.public_ipv4.map(|ip| ip.to_string()),
            "cpu_cores": req.cpu_cores,
            "memory_bytes": req.memory_bytes,
            "mtu": req.mtu,
            "labels": req.labels,
            "allocatable": allocatable,
        }),
    };

    // Append the event
    let event_store = state.db().event_store();
    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to enroll node");
        ApiError::internal("internal_error", "Failed to enroll node")
            .with_request_id(request_id.to_string())
    })?;

    let now = Utc::now();
    let response = NodeResponse {
        id: node_id.to_string(),
        state: "active".to_string(),
        wireguard_public_key: req.wireguard_public_key,
        agent_mtls_subject: req.agent_mtls_subject,
        public_ipv6: Some(req.public_ipv6.to_string()),
        public_ipv4: req.public_ipv4.map(|ip| ip.to_string()),
        labels: req.labels,
        allocatable,
        mtu: req.mtu,
        resource_version: 1,
        created_at: now,
        updated_at: now,
    };

    tracing::info!(
        node_id = %node_id,
        hostname = %req.hostname,
        region = %req.region,
        request_id = %request_id,
        "Node enrolled"
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// List all nodes.
///
/// GET /v1/nodes
async fn list_nodes(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    let rows = sqlx::query_as::<_, NodeRow>(
        r#"
        SELECT node_id, state, wireguard_public_key, agent_mtls_subject,
               host(public_ipv6)::TEXT as public_ipv6,
               host(public_ipv4)::TEXT as public_ipv4,
               labels, allocatable, mtu,
               resource_version, created_at, updated_at
        FROM nodes_view
        ORDER BY created_at DESC
        LIMIT 100
        "#,
    )
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list nodes");
        ApiError::internal("internal_error", "Failed to list nodes")
            .with_request_id(request_id.to_string())
    })?;

    let items: Vec<NodeResponse> = rows.into_iter().map(NodeResponse::from).collect();
    let total = items.len() as i64;

    Ok(Json(ListNodesResponse { items, total }))
}

/// Get a single node by ID.
///
/// GET /v1/nodes/{node_id}
async fn get_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate node_id format
    let _node_id: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.to_string())
    })?;

    let row = sqlx::query_as::<_, NodeRow>(
        r#"
        SELECT node_id, state, wireguard_public_key, agent_mtls_subject,
               host(public_ipv6)::TEXT as public_ipv6,
               host(public_ipv4)::TEXT as public_ipv4,
               labels, allocatable, mtu,
               resource_version, created_at, updated_at
        FROM nodes_view
        WHERE node_id = $1
        "#,
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, node_id = %node_id, "Failed to get node");
        ApiError::internal("internal_error", "Failed to get node")
            .with_request_id(request_id.to_string())
    })?;

    match row {
        Some(row) => Ok(Json(NodeResponse::from(row))),
        None => Err(ApiError::not_found(
            "node_not_found",
            format!("Node {} not found", node_id),
        )
        .with_request_id(request_id.to_string())),
    }
}

/// Process node heartbeat.
///
/// POST /v1/nodes/{node_id}/heartbeat
async fn heartbeat(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate node_id format
    let node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Check node exists and get current state
    let current_state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM nodes_view WHERE node_id = $1",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check node existence");
        ApiError::internal("internal_error", "Failed to verify node")
            .with_request_id(request_id.to_string())
    })?;

    let current_state = match current_state {
        Some(s) => s,
        None => {
            return Err(ApiError::not_found(
                "node_not_found",
                format!("Node {} not found", node_id),
            )
            .with_request_id(request_id.to_string()));
        }
    };

    // Get current aggregate sequence
    let current_seq = sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(MAX(aggregate_seq), 0) FROM event_log WHERE aggregate_id = $1",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to get aggregate sequence");
        ApiError::internal("internal_error", "Failed to process heartbeat")
            .with_request_id(request_id.to_string())
    })?;

    // Emit capacity update event
    let capacity_event = AppendEvent {
        aggregate_type: AggregateType::Node,
        aggregate_id: node_id.clone(),
        aggregate_seq: current_seq + 1,
        event_type: "node.capacity_updated".to_string(),
        event_version: 1,
        actor_type: ActorType::ServicePrincipal, // Node agents are service principals
        actor_id: node_id.clone(),
        org_id: None,
        request_id: request_id.to_string(),
        idempotency_key: None,
        app_id: None,
        env_id: None,
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "node_id": node_id_typed.to_string(),
            "available_cpu_cores": req.available_cpu_cores,
            "available_memory_bytes": req.available_memory_bytes,
            "instance_count": req.instance_count,
        }),
    };

    let event_store = state.db().event_store();
    event_store.append(capacity_event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to record capacity update");
        ApiError::internal("internal_error", "Failed to process heartbeat")
            .with_request_id(request_id.to_string())
    })?;

    // If state changed, emit state change event
    let new_state_str = match req.state {
        NodeState::Active => "active",
        NodeState::Draining => "draining",
        NodeState::Disabled => "disabled",
        NodeState::Degraded => "degraded",
        NodeState::Offline => "offline",
    };

    if current_state != new_state_str {
        let state_event = AppendEvent {
            aggregate_type: AggregateType::Node,
            aggregate_id: node_id.clone(),
            aggregate_seq: current_seq + 2,
            event_type: "node.state_changed".to_string(),
            event_version: 1,
            actor_type: ActorType::ServicePrincipal, // Node agents are service principals
            actor_id: node_id.clone(),
            org_id: None,
            request_id: request_id.to_string(),
            idempotency_key: None,
            app_id: None,
            env_id: None,
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({
                "node_id": node_id_typed.to_string(),
                "old_state": current_state,
                "new_state": new_state_str,
            }),
        };

        event_store.append(state_event).await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to record state change");
            ApiError::internal("internal_error", "Failed to process heartbeat")
                .with_request_id(request_id.to_string())
        })?;

        tracing::info!(
            node_id = %node_id,
            old_state = %current_state,
            new_state = %new_state_str,
            request_id = %request_id,
            "Node state changed"
        );
    }

    Ok(Json(HeartbeatResponse {
        accepted: true,
        next_heartbeat_secs: 30, // 30 second heartbeat interval
    }))
}

/// Get the current plan for a node.
///
/// GET /v1/nodes/{node_id}/plan
///
/// Returns the list of instances that should be running on this node.
async fn get_plan(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = RequestId::new();

    // Validate node_id format
    let _node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.to_string())
    })?;

    // Check node exists
    let node_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to check node existence");
        ApiError::internal("internal_error", "Failed to get plan")
            .with_request_id(request_id.to_string())
    })?;

    if !node_exists {
        return Err(ApiError::not_found(
            "node_not_found",
            format!("Node {} not found", node_id),
        )
        .with_request_id(request_id.to_string()));
    }

    // Query instances assigned to this node from instances_desired_view
    // Instances are allocated by the scheduler
    let instances = sqlx::query_as::<_, InstancePlanRow>(
        r#"
        SELECT i.instance_id, i.app_id, i.env_id, i.release_id,
               COALESCE(d.deploy_id, '') as deploy_id,
               r.image_ref as image, i.resource_version
        FROM instances_desired_view i
        JOIN releases_view r ON i.release_id = r.release_id
        LEFT JOIN deploys_view d ON i.env_id = d.env_id 
            AND d.status IN ('rolling', 'active')
        WHERE i.node_id = $1
          AND i.desired_state = 'running'
        ORDER BY i.created_at
        "#,
    )
    .bind(&node_id)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, node_id = %node_id, "Failed to get node plan");
        ApiError::internal("internal_error", "Failed to get plan")
            .with_request_id(request_id.to_string())
    })?;

    // Get max event_id as plan version
    let plan_version = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(event_id), 0) FROM event_log")
        .fetch_one(state.db().pool())
        .await
        .unwrap_or(0);

    let instance_plans: Vec<InstancePlan> = instances.into_iter().map(InstancePlan::from).collect();

    Ok(Json(NodePlanResponse {
        plan_version,
        instances: instance_plans,
    }))
}

// =============================================================================
// Database Row Types
// =============================================================================

/// Row from nodes_view table.
struct NodeRow {
    node_id: String,
    state: String,
    wireguard_public_key: String,
    agent_mtls_subject: String,
    public_ipv6: Option<String>,
    public_ipv4: Option<String>,
    labels: serde_json::Value,
    allocatable: serde_json::Value,
    mtu: Option<i32>,
    resource_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for NodeRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        Ok(Self {
            node_id: row.try_get("node_id")?,
            state: row.try_get("state")?,
            wireguard_public_key: row.try_get("wireguard_public_key")?,
            agent_mtls_subject: row.try_get("agent_mtls_subject")?,
            // These come as TEXT from the query since we cast with host()::TEXT
            public_ipv6: row.try_get("public_ipv6")?,
            public_ipv4: row.try_get("public_ipv4")?,
            labels: row.try_get("labels")?,
            allocatable: row.try_get("allocatable")?,
            mtu: row.try_get("mtu")?,
            resource_version: row.try_get("resource_version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl From<NodeRow> for NodeResponse {
    fn from(row: NodeRow) -> Self {
        Self {
            id: row.node_id,
            state: row.state,
            wireguard_public_key: row.wireguard_public_key,
            agent_mtls_subject: row.agent_mtls_subject,
            public_ipv6: row.public_ipv6,
            public_ipv4: row.public_ipv4,
            labels: row.labels,
            allocatable: row.allocatable,
            mtu: row.mtu,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Row for instance plan query.
struct InstancePlanRow {
    instance_id: String,
    app_id: String,
    env_id: String,
    release_id: String,
    deploy_id: String,
    image: String,
    #[allow(dead_code)]
    resource_version: i32,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstancePlanRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            release_id: row.try_get("release_id")?,
            deploy_id: row.try_get("deploy_id")?,
            image: row.try_get("image")?,
            resource_version: row.try_get("resource_version")?,
        })
    }
}

impl From<InstancePlanRow> for InstancePlan {
    fn from(row: InstancePlanRow) -> Self {
        Self {
            instance_id: row.instance_id,
            app_id: row.app_id,
            env_id: row.env_id,
            release_id: row.release_id,
            deploy_id: row.deploy_id,
            image: row.image,
            resources: InstanceResources {
                cpu: 1.0,           // Default, would come from release spec
                memory_bytes: 512 * 1024 * 1024, // Default 512MB
            },
            env_vars: serde_json::json!({}),
            volumes: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enroll_node_request_deserialization() {
        let json = r#"{
            "hostname": "node-1",
            "region": "us-west-2",
            "wireguard_public_key": "dGVzdGtleXRlc3RrZXl0ZXN0a2V5dGVzdGtleXRlc3Q=",
            "agent_mtls_subject": "CN=node-1.platform.local",
            "public_ipv6": "2001:db8::1",
            "cpu_cores": 8,
            "memory_bytes": 17179869184
        }"#;
        let req: EnrollNodeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.hostname, "node-1");
        assert_eq!(req.region, "us-west-2");
        assert_eq!(req.cpu_cores, 8);
        assert!(req.public_ipv4.is_none());
    }

    #[test]
    fn test_heartbeat_request_deserialization() {
        let json = r#"{
            "state": "active",
            "available_cpu_cores": 6,
            "available_memory_bytes": 12884901888,
            "instance_count": 4
        }"#;
        let req: HeartbeatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.state, NodeState::Active);
        assert_eq!(req.available_cpu_cores, 6);
        assert_eq!(req.instance_count, 4);
    }

    #[test]
    fn test_node_response_serialization() {
        let response = NodeResponse {
            id: "node_123".to_string(),
            state: "active".to_string(),
            wireguard_public_key: "key123".to_string(),
            agent_mtls_subject: "CN=test".to_string(),
            public_ipv6: Some("2001:db8::1".to_string()),
            public_ipv4: None,
            labels: serde_json::json!({"region": "us-west-2"}),
            allocatable: serde_json::json!({"cpu_cores": 8}),
            mtu: Some(1500),
            resource_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"id\":\"node_123\""));
        assert!(json.contains("\"state\":\"active\""));
        assert!(!json.contains("public_ipv4")); // Should be skipped when None
    }
}
