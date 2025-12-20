//! Node API endpoints.
//!
//! Provides endpoints for node enrollment, heartbeats, and plan delivery.
//! These are internal APIs called by node-agents, not tenant-facing.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use plfm_events::{ActorType, AggregateType, NodeState};
use plfm_id::{AppId, EnvId, InstanceId, NodeId, OrgId, SecretVersionId};
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::api::error::ApiError;
use crate::api::request_context::RequestContext;
use crate::db::AppendEvent;
use crate::secrets as secrets_crypto;
use crate::state::AppState;

const MAX_LOG_ENTRIES: usize = 500;
const MAX_LOG_LINE_BYTES: usize = 16 * 1024;

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
        .route("/{node_id}/secrets/{version_id}", get(get_secret_material))
        .route("/{node_id}/logs", post(ingest_logs))
        .route(
            "/{node_id}/instances/{instance_id}/status",
            post(report_instance_status),
        )
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

    /// Next cursor (null if no more results).
    pub next_cursor: Option<String>,
}

/// Query parameters for listing nodes.
#[derive(Debug, Deserialize)]
pub struct ListNodesQuery {
    /// Max number of items to return.
    pub limit: Option<i64>,
    /// Cursor (exclusive). Interpreted as a node_id.
    pub cursor: Option<String>,
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

    /// Process type.
    pub process_type: String,

    /// Release ID to run.
    pub release_id: String,

    /// Deploy ID that triggered this instance.
    pub deploy_id: String,

    /// OCI image reference.
    pub image: String,

    /// Resource requests.
    pub resources: InstanceResources,

    /// Overlay IPv6 address for this instance.
    #[serde(default)]
    pub overlay_ipv6: String,

    /// Secrets version ID (if configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets_version_id: Option<String>,

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
#[derive(Debug, Clone, Serialize)]
pub struct VolumeMount {
    /// Volume ID.
    pub volume_id: String,

    /// Mount path inside the instance.
    pub mount_path: String,

    /// Whether the mount is read-only.
    pub read_only: bool,
}

/// Secret material response for node agent delivery.
#[derive(Debug, Serialize)]
pub struct SecretMaterialResponse {
    pub version_id: String,
    pub format: String,
    pub data_hash: String,
    pub data: String,
}

/// Request to report instance status for a node-assigned instance.
#[derive(Debug, Deserialize)]
pub struct ReportInstanceStatusRequest {
    /// Current status.
    pub status: String,

    /// Optional boot ID.
    #[serde(default)]
    pub boot_id: Option<String>,

    /// Optional error message.
    #[serde(default)]
    pub error_message: Option<String>,

    /// Optional exit code.
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// Response for instance status reports.
#[derive(Debug, Serialize)]
pub struct ReportInstanceStatusResponse {
    pub accepted: bool,
}

/// Workload log ingestion request (from node agents).
#[derive(Debug, Deserialize)]
pub struct WorkloadLogIngestRequest {
    pub entries: Vec<WorkloadLogIngestEntry>,
}

#[derive(Debug, Deserialize)]
pub struct WorkloadLogIngestEntry {
    pub ts: DateTime<Utc>,
    pub instance_id: String,
    #[serde(default)]
    pub stream: Option<String>,
    pub line: String,
    #[serde(default)]
    pub truncated: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadLogIngestResponse {
    pub accepted: usize,
    pub rejected: usize,
}

// =============================================================================
// Handlers
// =============================================================================

/// Enroll a new node.
///
/// POST /v1/nodes/enroll
async fn enroll_node(
    State(state): State<AppState>,
    ctx: RequestContext,
    Json(req): Json<EnrollNodeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate hostname
    if req.hostname.is_empty() {
        return Err(
            ApiError::bad_request("invalid_hostname", "Hostname cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    if req.hostname.len() > 255 {
        return Err(ApiError::bad_request(
            "invalid_hostname",
            "Hostname cannot exceed 255 characters",
        )
        .with_request_id(request_id.clone()));
    }

    // Validate region
    if req.region.is_empty() {
        return Err(
            ApiError::bad_request("invalid_region", "Region cannot be empty")
                .with_request_id(request_id.clone()),
        );
    }

    // Validate WireGuard key (should be base64-encoded 32 bytes = 44 chars with padding)
    if req.wireguard_public_key.len() < 40 || req.wireguard_public_key.len() > 50 {
        return Err(ApiError::bad_request(
            "invalid_wireguard_key",
            "Invalid WireGuard public key format",
        )
        .with_request_id(request_id.clone()));
    }

    // Validate resources
    if req.cpu_cores < 1 {
        return Err(
            ApiError::bad_request("invalid_cpu_cores", "CPU cores must be at least 1")
                .with_request_id(request_id.clone()),
        );
    }

    if req.memory_bytes < 1024 * 1024 * 512 {
        return Err(
            ApiError::bad_request("invalid_memory", "Memory must be at least 512MB")
                .with_request_id(request_id.clone()),
        );
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
            .with_request_id(request_id.clone())
    })?;

    if key_exists {
        return Err(ApiError::conflict(
            "wireguard_key_exists",
            "A node with this WireGuard key is already enrolled",
        )
        .with_request_id(request_id.clone()));
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
        request_id: request_id.clone(),
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
            .with_request_id(request_id.clone())
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
async fn list_nodes(
    State(state): State<AppState>,
    ctx: RequestContext,
    Query(query): Query<ListNodesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    let limit: i64 = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query.cursor;

    let rows = sqlx::query_as::<_, NodeRow>(
        r#"
        SELECT node_id, state, wireguard_public_key, agent_mtls_subject,
               host(public_ipv6)::TEXT as public_ipv6,
               host(public_ipv4)::TEXT as public_ipv4,
               labels, allocatable, mtu,
               resource_version, created_at, updated_at
        FROM nodes_view
        WHERE ($1::text IS NULL OR node_id > $1)
        ORDER BY node_id ASC
        LIMIT $2
        "#,
    )
    .bind(cursor.as_deref())
    .bind(limit)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to list nodes");
        ApiError::internal("internal_error", "Failed to list nodes")
            .with_request_id(request_id.clone())
    })?;

    let items: Vec<NodeResponse> = rows.into_iter().map(NodeResponse::from).collect();
    let next_cursor = if items.len() == limit as usize {
        items.last().map(|item| item.id.clone())
    } else {
        None
    };

    Ok(Json(ListNodesResponse { items, next_cursor }))
}

/// Get a single node by ID.
///
/// GET /v1/nodes/{node_id}
async fn get_node(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(node_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate node_id format
    let _node_id: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
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
            .with_request_id(request_id.clone())
    })?;

    match row {
        Some(row) => Ok(Json(NodeResponse::from(row))),
        None => Err(
            ApiError::not_found("node_not_found", format!("Node {} not found", node_id))
                .with_request_id(request_id.clone()),
        ),
    }
}

/// Process node heartbeat.
///
/// POST /v1/nodes/{node_id}/heartbeat
async fn heartbeat(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(node_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate node_id format
    let node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
    })?;

    // Check node exists and get current state
    let current_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM nodes_view WHERE node_id = $1")
            .bind(&node_id)
            .fetch_optional(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check node existence");
                ApiError::internal("internal_error", "Failed to verify node")
                    .with_request_id(request_id.clone())
            })?;

    let current_state = match current_state {
        Some(s) => s,
        None => {
            return Err(ApiError::not_found(
                "node_not_found",
                format!("Node {} not found", node_id),
            )
            .with_request_id(request_id.clone()));
        }
    };

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Node, &node_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to process heartbeat")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let instance_statuses_entries = req
        .instance_statuses
        .as_object()
        .map(|entries| entries.len() as i32)
        .unwrap_or(0);

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
        request_id: request_id.clone(),
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
            "instance_statuses_entries": instance_statuses_entries,
        }),
    };

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
            request_id: request_id.clone(),
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

        let events = vec![capacity_event, state_event];
        event_store.append_batch(events).await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to process heartbeat");
            ApiError::internal("internal_error", "Failed to process heartbeat")
                .with_request_id(request_id.clone())
        })?;

        tracing::info!(
            node_id = %node_id,
            old_state = %current_state,
            new_state = %new_state_str,
            request_id = %request_id,
            "Node state changed"
        );
    } else {
        event_store.append_batch(vec![capacity_event]).await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to process heartbeat");
            ApiError::internal("internal_error", "Failed to process heartbeat")
                .with_request_id(request_id.clone())
        })?;
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
    ctx: RequestContext,
    Path(node_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id;

    // Validate node_id format
    let _node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
    })?;

    // Check node exists
    let node_exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)")
            .bind(&node_id)
            .fetch_one(state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check node existence");
                ApiError::internal("internal_error", "Failed to get plan")
                    .with_request_id(request_id.clone())
            })?;

    if !node_exists {
        return Err(
            ApiError::not_found("node_not_found", format!("Node {} not found", node_id))
                .with_request_id(request_id.clone()),
        );
    }

    // Query instances assigned to this node from instances_desired_view
    // Instances are allocated by the scheduler
    let instances = sqlx::query_as::<_, InstancePlanRow>(
        r#"
        SELECT i.instance_id,
               i.app_id,
               i.env_id,
               i.process_type,
               i.release_id,
               COALESCE(i.deploy_id, '') as deploy_id,
               r.image_ref as image_ref,
               r.index_or_manifest_digest as image_digest,
               i.secrets_version_id,
               host(i.overlay_ipv6)::TEXT as overlay_ipv6,
               i.resources_snapshot,
               i.resource_version
        FROM instances_desired_view i
        JOIN releases_view r ON i.release_id = r.release_id
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
            .with_request_id(request_id.clone())
    })?;

    // Get max event_id as plan version
    let event_store = state.db().event_store();
    let plan_version = event_store.get_max_event_id().await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get plan version");
        ApiError::internal("internal_error", "Failed to get plan")
            .with_request_id(request_id.clone())
    })?;

    let volume_mounts = load_volume_mounts(&state, &request_id, &instances).await?;
    let instance_plans: Vec<InstancePlan> = instances
        .into_iter()
        .map(|row| InstancePlan::from_row(row, &volume_mounts))
        .collect();

    Ok(Json(NodePlanResponse {
        plan_version,
        instances: instance_plans,
    }))
}

/// Fetch decrypted secret material for a specific version.
///
/// GET /v1/nodes/{node_id}/secrets/{version_id}
async fn get_secret_material(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((node_id, version_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    let _node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
    })?;

    let version_id_typed: SecretVersionId = version_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_secret_version_id", "Invalid secret version ID format")
            .with_request_id(request_id.clone())
    })?;

    let node_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)",
    )
    .bind(&node_id)
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check node existence");
        ApiError::internal("internal_error", "Failed to load secrets")
            .with_request_id(request_id.clone())
    })?;

    if !node_exists {
        return Err(
            ApiError::not_found("node_not_found", format!("Node {} not found", node_id))
                .with_request_id(request_id.clone()),
        );
    }

    let row = sqlx::query_as::<_, SecretMaterialRow>(
        r#"
        SELECT sv.version_id,
               sv.bundle_id,
               sv.org_id,
               sv.env_id,
               sv.data_hash,
               sv.format,
               sm.cipher,
               sm.nonce,
               sm.ciphertext,
               sm.master_key_id,
               sm.wrapped_data_key,
               sm.wrapped_data_key_nonce
        FROM secret_versions sv
        JOIN secret_material sm ON sv.material_id = sm.material_id
        WHERE sv.version_id = $1
        "#,
    )
    .bind(version_id_typed.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load secret material");
        ApiError::internal("internal_error", "Failed to load secrets")
            .with_request_id(request_id.clone())
    })?;

    let Some(row) = row else {
        return Err(ApiError::not_found(
            "secret_version_not_found",
            "Secret version not found",
        )
        .with_request_id(request_id));
    };

    let aad = secrets_aad(&row.org_id, &row.env_id, &row.bundle_id, &row.version_id, &row.data_hash);
    let plaintext = secrets_crypto::decrypt(
        &row.master_key_id,
        &row.nonce,
        &row.ciphertext,
        &row.wrapped_data_key,
        &row.wrapped_data_key_nonce,
        aad.as_bytes(),
    )
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to decrypt secrets");
        ApiError::internal("secrets_decrypt_failed", "Failed to decrypt secrets")
            .with_request_id(request_id.clone())
    })?;

    let data = String::from_utf8(plaintext).map_err(|_| {
        ApiError::internal("secrets_decode_failed", "Secrets payload was not valid UTF-8")
            .with_request_id(request_id.clone())
    })?;

    Ok(Json(SecretMaterialResponse {
        version_id: row.version_id,
        format: row.format,
        data_hash: row.data_hash,
        data,
    }))
}

/// Ingest workload logs from a node agent.
///
/// POST /v1/nodes/{node_id}/logs
async fn ingest_logs(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path(node_id): Path<String>,
    Json(req): Json<WorkloadLogIngestRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    let node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
    })?;

    let node_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)",
    )
    .bind(node_id_typed.to_string())
    .fetch_one(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to check node existence");
        ApiError::internal("internal_error", "Failed to ingest logs")
            .with_request_id(request_id.clone())
    })?;

    if !node_exists {
        return Err(
            ApiError::not_found("node_not_found", format!("Node {} not found", node_id))
                .with_request_id(request_id.clone()),
        );
    }

    if req.entries.is_empty() {
        return Ok(Json(WorkloadLogIngestResponse {
            accepted: 0,
            rejected: 0,
        }));
    }

    if req.entries.len() > MAX_LOG_ENTRIES {
        return Err(ApiError::bad_request(
            "too_many_entries",
            format!("Log batch exceeds max of {MAX_LOG_ENTRIES} entries"),
        )
        .with_request_id(request_id));
    }

    let mut instance_ids: Vec<String> = req.entries.iter().map(|e| e.instance_id.clone()).collect();
    instance_ids.sort();
    instance_ids.dedup();

    let instance_rows = sqlx::query_as::<_, InstanceLogMetaRow>(
        r#"
        SELECT instance_id, org_id, app_id, env_id, process_type, node_id
        FROM instances_desired_view
        WHERE instance_id = ANY($1::TEXT[])
        "#,
    )
    .bind(&instance_ids)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load instance metadata");
        ApiError::internal("internal_error", "Failed to ingest logs")
            .with_request_id(request_id.clone())
    })?;

    let mut instance_meta: HashMap<String, InstanceLogMetaRow> = HashMap::new();
    for row in instance_rows {
        if row.node_id == node_id {
            instance_meta.insert(row.instance_id.clone(), row);
        }
    }

    let mut accepted_entries: Vec<WorkloadLogRow> = Vec::new();
    let mut rejected = 0usize;

    for entry in req.entries {
        let Some(meta) = instance_meta.get(&entry.instance_id) else {
            rejected += 1;
            continue;
        };

        let stream = normalize_log_stream(entry.stream.as_deref());
        let (line, truncated) =
            normalize_log_line(&entry.line, entry.truncated.unwrap_or(false));

        accepted_entries.push(WorkloadLogRow {
            org_id: meta.org_id.clone(),
            app_id: meta.app_id.clone(),
            env_id: meta.env_id.clone(),
            process_type: meta.process_type.clone(),
            instance_id: entry.instance_id,
            node_id: node_id.clone(),
            ts: entry.ts,
            stream,
            line,
            truncated,
        });
    }

    if accepted_entries.is_empty() {
        return Ok(Json(WorkloadLogIngestResponse {
            accepted: 0,
            rejected,
        }));
    }

    let mut builder = QueryBuilder::new(
        "INSERT INTO workload_logs (org_id, app_id, env_id, process_type, instance_id, node_id, ts, stream, line, truncated) ",
    );
    builder.push_values(accepted_entries.iter(), |mut b, entry| {
        b.push_bind(&entry.org_id)
            .push_bind(&entry.app_id)
            .push_bind(&entry.env_id)
            .push_bind(&entry.process_type)
            .push_bind(&entry.instance_id)
            .push_bind(&entry.node_id)
            .push_bind(entry.ts)
            .push_bind(&entry.stream)
            .push_bind(&entry.line)
            .push_bind(entry.truncated);
    });

    builder.build().execute(state.db().pool()).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to insert workload logs");
        ApiError::internal("internal_error", "Failed to ingest logs")
            .with_request_id(request_id.clone())
    })?;

    Ok(Json(WorkloadLogIngestResponse {
        accepted: accepted_entries.len(),
        rejected,
    }))
}

/// Report instance status for an instance assigned to this node.
///
/// POST /v1/nodes/{node_id}/instances/{instance_id}/status
async fn report_instance_status(
    State(state): State<AppState>,
    ctx: RequestContext,
    Path((node_id, instance_id)): Path<(String, String)>,
    Json(req): Json<ReportInstanceStatusRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request_id = ctx.request_id.clone();

    if ctx.actor_type != ActorType::System {
        return Err(ApiError::forbidden(
            "forbidden",
            "This endpoint is only available to system actors",
        )
        .with_request_id(request_id));
    }

    let node_id_typed: NodeId = node_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_node_id", "Invalid node ID format")
            .with_request_id(request_id.clone())
    })?;

    let instance_id_typed: InstanceId = instance_id.parse().map_err(|_| {
        ApiError::bad_request("invalid_instance_id", "Invalid instance ID format")
            .with_request_id(request_id.clone())
    })?;

    let valid_statuses = ["booting", "ready", "draining", "stopped", "failed"];
    if !valid_statuses.contains(&req.status.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_status",
            format!("Status must be one of: {:?}", valid_statuses),
        )
        .with_request_id(request_id.clone()));
    }

    let current_status = sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM instances_status_view WHERE instance_id = $1",
    )
    .bind(instance_id_typed.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to get current status");
        ApiError::internal("internal_error", "Failed to process status")
            .with_request_id(request_id.clone())
    })?
    .flatten();

    let instance_info = sqlx::query_as::<_, InstanceInfoRow>(
        r#"
        SELECT org_id, app_id, env_id
        FROM instances_desired_view
        WHERE instance_id = $1 AND node_id = $2
        "#,
    )
    .bind(instance_id_typed.to_string())
    .bind(node_id_typed.to_string())
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to get instance info");
        ApiError::internal("internal_error", "Failed to process status")
            .with_request_id(request_id.clone())
    })?;

    let instance_info = match instance_info {
        Some(info) => info,
        None => {
            return Err(ApiError::not_found(
                "instance_not_found",
                "Instance not found on this node",
            )
            .with_request_id(request_id.clone()));
        }
    };

    let event_store = state.db().event_store();
    let current_seq = event_store
        .get_latest_aggregate_seq(&AggregateType::Instance, &instance_id_typed.to_string())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get aggregate sequence");
            ApiError::internal("internal_error", "Failed to process status")
                .with_request_id(request_id.clone())
        })?
        .unwrap_or(0);

    let org_id = instance_info.org_id.parse::<OrgId>().map_err(|_| {
        ApiError::internal("internal_error", "Invalid org_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;
    let app_id = instance_info.app_id.parse::<AppId>().map_err(|_| {
        ApiError::internal("internal_error", "Invalid app_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;
    let env_id = instance_info.env_id.parse::<EnvId>().map_err(|_| {
        ApiError::internal("internal_error", "Invalid env_id in instances_desired_view")
            .with_request_id(request_id.clone())
    })?;

    let event = AppendEvent {
        aggregate_type: AggregateType::Instance,
        aggregate_id: instance_id_typed.to_string(),
        aggregate_seq: current_seq + 1,
        event_type: "instance.status_changed".to_string(),
        event_version: 1,
        actor_type: ActorType::ServicePrincipal, // Node agent
        actor_id: node_id_typed.to_string(),
        org_id: Some(org_id),
        request_id: request_id.clone(),
        idempotency_key: None,
        app_id: Some(app_id),
        env_id: Some(env_id),
        correlation_id: None,
        causation_id: None,
        payload: serde_json::json!({
            "instance_id": instance_id_typed.to_string(),
            "old_status": current_status.unwrap_or_else(|| "unknown".to_string()),
            "new_status": req.status,
            "boot_id": req.boot_id,
            "error_message": req.error_message,
            "exit_code": req.exit_code,
        }),
    };

    event_store.append(event).await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to record status");
        ApiError::internal("internal_error", "Failed to record status")
            .with_request_id(request_id.clone())
    })?;

    Ok((
        StatusCode::OK,
        Json(ReportInstanceStatusResponse { accepted: true }),
    ))
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
    process_type: String,
    release_id: String,
    deploy_id: String,
    image_ref: String,
    image_digest: String,
    secrets_version_id: Option<String>,
    overlay_ipv6: Option<String>,
    resources_snapshot: serde_json::Value,
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
            process_type: row.try_get("process_type")?,
            release_id: row.try_get("release_id")?,
            deploy_id: row.try_get("deploy_id")?,
            image_ref: row.try_get("image_ref")?,
            image_digest: row.try_get("image_digest")?,
            secrets_version_id: row.try_get("secrets_version_id")?,
            overlay_ipv6: row.try_get("overlay_ipv6")?,
            resources_snapshot: row.try_get("resources_snapshot")?,
            resource_version: row.try_get("resource_version")?,
        })
    }
}

impl InstancePlan {
    fn from_row(row: InstancePlanRow, volume_mounts: &VolumeMountMap) -> Self {
        let image = compose_image_ref(&row.image_ref, &row.image_digest);
        let resources = resources_from_snapshot(&row.resources_snapshot);
        let volumes = volume_mounts
            .get(&(row.env_id.clone(), row.process_type.clone()))
            .cloned()
            .unwrap_or_default();

        Self {
            instance_id: row.instance_id,
            app_id: row.app_id,
            env_id: row.env_id,
            process_type: row.process_type,
            release_id: row.release_id,
            deploy_id: row.deploy_id,
            image,
            resources,
            overlay_ipv6: row.overlay_ipv6.unwrap_or_default(),
            secrets_version_id: row.secrets_version_id,
            env_vars: serde_json::json!({}),
            volumes,
        }
    }
}

#[derive(Debug)]
struct InstanceLogMetaRow {
    instance_id: String,
    org_id: String,
    app_id: String,
    env_id: String,
    process_type: String,
    node_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceLogMetaRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            node_id: row.try_get("node_id")?,
        })
    }
}

#[derive(Debug)]
struct WorkloadLogRow {
    org_id: String,
    app_id: String,
    env_id: String,
    process_type: String,
    instance_id: String,
    node_id: String,
    ts: DateTime<Utc>,
    stream: String,
    line: String,
    truncated: bool,
}

fn normalize_log_stream(stream: Option<&str>) -> String {
    match stream {
        Some("stdout") => "stdout".to_string(),
        Some("stderr") => "stderr".to_string(),
        _ => "stdout".to_string(),
    }
}

fn normalize_log_line(line: &str, truncated_flag: bool) -> (String, bool) {
    if line.as_bytes().len() <= MAX_LOG_LINE_BYTES {
        return (line.to_string(), truncated_flag);
    }

    let limit = MAX_LOG_LINE_BYTES.saturating_sub(3);
    let mut end = 0;
    for (idx, ch) in line.char_indices() {
        let next = idx + ch.len_utf8();
        if next > limit {
            break;
        }
        end = next;
    }

    let mut trimmed = line[..end].to_string();
    trimmed.push_str("...");
    (trimmed, true)
}

type VolumeMountMap = HashMap<(String, String), Vec<VolumeMount>>;

fn compose_image_ref(image_ref: &str, image_digest: &str) -> String {
    if image_ref.contains('@') {
        image_ref.to_string()
    } else {
        format!("{image_ref}@{image_digest}")
    }
}

fn resources_from_snapshot(snapshot: &serde_json::Value) -> InstanceResources {
    let cpu = snapshot
        .get("cpu")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let memory_bytes = snapshot
        .get("memory_bytes")
        .and_then(|v| v.as_i64())
        .unwrap_or(512 * 1024 * 1024);

    InstanceResources { cpu, memory_bytes }
}

async fn load_volume_mounts(
    state: &AppState,
    request_id: &str,
    instances: &[InstancePlanRow],
) -> Result<VolumeMountMap, ApiError> {
    let mut env_ids = Vec::new();
    let mut process_types = Vec::new();
    for instance in instances {
        env_ids.push(instance.env_id.clone());
        process_types.push(instance.process_type.clone());
    }

    env_ids.sort();
    env_ids.dedup();
    process_types.sort();
    process_types.dedup();

    if env_ids.is_empty() || process_types.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as::<_, VolumeMountRow>(
        r#"
        SELECT env_id, process_type, volume_id, mount_path, read_only
        FROM volume_attachments_view
        WHERE env_id = ANY($1::TEXT[])
          AND process_type = ANY($2::TEXT[])
          AND NOT is_deleted
        ORDER BY env_id ASC, process_type ASC, volume_id ASC
        "#,
    )
    .bind(env_ids)
    .bind(process_types)
    .fetch_all(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load volume mounts");
        ApiError::internal("internal_error", "Failed to load volume mounts")
            .with_request_id(request_id.to_string())
    })?;

    let mut mounts: VolumeMountMap = HashMap::new();
    for row in rows {
        mounts
            .entry((row.env_id, row.process_type))
            .or_default()
            .push(VolumeMount {
                volume_id: row.volume_id,
                mount_path: row.mount_path,
                read_only: row.read_only,
            });
    }

    Ok(mounts)
}

struct SecretMaterialRow {
    version_id: String,
    bundle_id: String,
    org_id: String,
    env_id: String,
    data_hash: String,
    format: String,
    cipher: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    master_key_id: String,
    wrapped_data_key: Vec<u8>,
    wrapped_data_key_nonce: Vec<u8>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for SecretMaterialRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            version_id: row.try_get("version_id")?,
            bundle_id: row.try_get("bundle_id")?,
            org_id: row.try_get("org_id")?,
            env_id: row.try_get("env_id")?,
            data_hash: row.try_get("data_hash")?,
            format: row.try_get("format")?,
            cipher: row.try_get("cipher")?,
            nonce: row.try_get("nonce")?,
            ciphertext: row.try_get("ciphertext")?,
            master_key_id: row.try_get("master_key_id")?,
            wrapped_data_key: row.try_get("wrapped_data_key")?,
            wrapped_data_key_nonce: row.try_get("wrapped_data_key_nonce")?,
        })
    }
}

struct VolumeMountRow {
    env_id: String,
    process_type: String,
    volume_id: String,
    mount_path: String,
    read_only: bool,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for VolumeMountRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            volume_id: row.try_get("volume_id")?,
            mount_path: row.try_get("mount_path")?,
            read_only: row.try_get("read_only")?,
        })
    }
}

fn secrets_aad(
    org_id: &str,
    env_id: &str,
    bundle_id: &str,
    version_id: &str,
    data_hash: &str,
) -> String {
    format!(
        "trc-secrets-v1|org:{org_id}|env:{env_id}|bundle:{bundle_id}|version:{version_id}|hash:{data_hash}"
    )
}

struct InstanceInfoRow {
    org_id: String,
    app_id: String,
    env_id: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstanceInfoRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
        })
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
