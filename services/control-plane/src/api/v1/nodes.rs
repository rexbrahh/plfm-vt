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
use plfm_id::{AppId, AssignmentId, EnvId, InstanceId, NodeId, OrgId, SecretVersionId, Ulid};
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
const NODE_PLAN_SPEC_VERSION: &str = "v1";
const WORKLOAD_SPEC_VERSION: &str = "v1";
const DEFAULT_DRAIN_GRACE_SECONDS: i32 = 10;
const DEFAULT_EPHEMERAL_DISK_BYTES: i64 = 4 * 1024 * 1024 * 1024;
const DEFAULT_GATEWAY_IPV6: &str = "fe80::1";
const DEFAULT_MTU: i32 = 1420;

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

    /// Overlay IPv6 address (/128).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_ipv6: Option<String>,

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
    pub spec_version: String,
    pub node_id: String,
    pub plan_id: String,
    pub created_at: DateTime<Utc>,
    pub cursor_event_id: i64,
    pub instances: Vec<DesiredInstanceAssignment>,
}

#[derive(Debug, Serialize)]
pub struct DesiredInstanceAssignment {
    pub assignment_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub generation: i32,
    pub desired_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_grace_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload: Option<WorkloadSpec>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadSpec {
    pub spec_version: String,
    pub org_id: String,
    pub app_id: String,
    pub env_id: String,
    pub process_type: String,
    pub instance_id: String,
    pub generation: i32,
    pub release_id: String,
    pub image: WorkloadImage,
    pub manifest_hash: String,
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_vars: Option<HashMap<String, String>>,
    pub resources: WorkloadResources,
    pub network: WorkloadNetwork,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<WorkloadMount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<WorkloadSecrets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadImage {
    #[serde(skip_serializing_if = "Option::is_none", rename = "ref")]
    pub image_ref: Option<String>,
    pub digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_digest: Option<String>,
    pub resolved_digest: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Serialize)]
pub struct WorkloadResources {
    pub cpu_request: f64,
    pub memory_limit_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral_disk_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpu_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_weight: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadNetwork {
    pub overlay_ipv6: String,
    pub gateway_ipv6: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<WorkloadPort>>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadPort {
    pub name: String,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkloadMount {
    pub volume_id: String,
    pub mount_path: String,
    pub read_only: bool,
    pub filesystem: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkloadSecrets {
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_version_id: Option<String>,
    pub mount_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gid: Option<i32>,
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
    let overlay_ipv6 = allocate_node_ipv6(state.db().pool(), &node_id, &request_id).await?;

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
            "overlay_ipv6": overlay_ipv6.clone(),
            "cpu_cores": req.cpu_cores,
            "memory_bytes": req.memory_bytes,
            "mtu": req.mtu,
            "labels": req.labels,
            "allocatable": allocatable,
        }),
        ..Default::default()
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
        overlay_ipv6: Some(overlay_ipv6.clone()),
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

async fn allocate_node_ipv6(
    pool: &sqlx::PgPool,
    node_id: &NodeId,
    request_id: &str,
) -> Result<String, ApiError> {
    let request_id = request_id.to_string();
    let prefix = std::env::var("PLFM_NODE_IPV6_PREFIX")
        .or_else(|_| std::env::var("GHOST_NODE_IPV6_PREFIX"))
        .unwrap_or_else(|_| "fd00:0:0:1::".to_string());

    let base: Ipv6Addr = prefix.parse().map_err(|_| {
        ApiError::internal(
            "ipam_error",
            format!(
                "invalid node IPv6 prefix '{}'; expected /64 base address",
                prefix
            ),
        )
        .with_request_id(request_id.clone())
    })?;

    let base_u128 = u128::from(base) & (!0u128 << 64);
    let mut attempts = 0;

    loop {
        let suffix: i64 = sqlx::query_scalar("SELECT nextval('ipam_node_suffix_seq')")
            .fetch_one(pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to allocate node IPv6 suffix");
                ApiError::internal("ipam_error", "Failed to allocate node IPv6 suffix")
                    .with_request_id(request_id.clone())
            })?;

        if suffix < 0 {
            attempts += 1;
            if attempts > 5 {
                return Err(ApiError::internal(
                    "ipam_error",
                    "Failed to allocate node IPv6 suffix",
                )
                .with_request_id(request_id.clone()));
            }
            continue;
        }

        let addr = Ipv6Addr::from(base_u128 | suffix as u128);

        let insert = sqlx::query(
            r#"
            INSERT INTO ipam_nodes (node_id, ipv6_suffix, overlay_ipv6)
            VALUES ($1, $2, $3::inet)
            "#,
        )
        .bind(node_id.to_string())
        .bind(suffix)
        .bind(addr.to_string())
        .execute(pool)
        .await;

        match insert {
            Ok(_) => return Ok(addr.to_string()),
            Err(sqlx::Error::Database(db_err)) => {
                let constraint = db_err.constraint().unwrap_or_default();
                if constraint == "ipam_nodes_pkey"
                    || constraint == "ipam_nodes_ipv6_suffix_key"
                    || constraint == "ipam_nodes_overlay_ipv6_key"
                {
                    attempts += 1;
                    if attempts > 5 {
                        return Err(ApiError::internal(
                            "ipam_error",
                            "ipam allocation retry limit reached",
                        )
                        .with_request_id(request_id.clone()));
                    }
                    continue;
                }
                tracing::error!(
                    error = %db_err,
                    request_id = %request_id,
                    "Failed to allocate node overlay IPv6"
                );
                return Err(ApiError::internal(
                    "ipam_error",
                    "Failed to allocate node overlay IPv6",
                )
                .with_request_id(request_id.clone()));
            }
            Err(e) => {
                tracing::error!(error = %e, request_id = %request_id, "Failed to allocate node overlay IPv6");
                return Err(ApiError::internal(
                    "ipam_error",
                    "Failed to allocate node overlay IPv6",
                )
                .with_request_id(request_id.clone()));
            }
        }
    }
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
               host(overlay_ipv6)::TEXT as overlay_ipv6,
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
               host(overlay_ipv6)::TEXT as overlay_ipv6,
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
        ..Default::default()
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
            ..Default::default()
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

    let node_info = sqlx::query_as::<_, NodePlanNodeRow>(
        "SELECT labels, mtu FROM nodes_view WHERE node_id = $1",
    )
    .bind(&node_id)
    .fetch_optional(state.db().pool())
    .await
    .map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to load node info");
        ApiError::internal("internal_error", "Failed to get plan")
            .with_request_id(request_id.clone())
    })?;

    let node_info = match node_info {
        Some(info) => info,
        None => {
            return Err(ApiError::not_found(
                "node_not_found",
                format!("Node {} not found", node_id),
            )
            .with_request_id(request_id.clone()));
        }
    };

    // Query instances assigned to this node from instances_desired_view
    // Instances are allocated by the scheduler
    let instances = sqlx::query_as::<_, InstancePlanRow>(
        r#"
        SELECT i.instance_id,
               i.org_id,
               i.app_id,
               i.env_id,
               i.process_type,
               i.node_id,
               i.desired_state,
               i.generation,
               i.release_id,
               r.image_ref as image_ref,
               r.index_or_manifest_digest as index_or_manifest_digest,
               r.resolved_digests as resolved_digests,
               r.manifest_hash as manifest_hash,
               r.command as command,
               i.secrets_version_id,
               host(i.overlay_ipv6)::TEXT as overlay_ipv6,
               i.resources_snapshot,
               i.spec_hash
        FROM instances_desired_view i
        JOIN releases_view r ON i.release_id = r.release_id
        WHERE i.node_id = $1
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

    let event_store = state.db().event_store();
    let cursor_event_id = event_store.get_max_event_id().await.map_err(|e| {
        tracing::error!(error = %e, request_id = %request_id, "Failed to get plan cursor");
        ApiError::internal("internal_error", "Failed to get plan")
            .with_request_id(request_id.clone())
    })?;

    let volume_mounts = load_volume_mounts(&state, &request_id, &instances).await?;
    let arch_hint = label_value(&node_info.labels, "arch");
    let instance_assignments: Vec<DesiredInstanceAssignment> = instances
        .into_iter()
        .map(|row| assignment_from_row(row, &volume_mounts, node_info.mtu, arch_hint.as_deref()))
        .collect();

    Ok(Json(NodePlanResponse {
        spec_version: NODE_PLAN_SPEC_VERSION.to_string(),
        node_id,
        plan_id: Ulid::new().to_string(),
        created_at: Utc::now(),
        cursor_event_id,
        instances: instance_assignments,
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
        ApiError::bad_request(
            "invalid_secret_version_id",
            "Invalid secret version ID format",
        )
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
        return Err(
            ApiError::not_found("secret_version_not_found", "Secret version not found")
                .with_request_id(request_id),
        );
    };

    if row.cipher != secrets_crypto::CIPHER_NAME {
        tracing::error!(
            cipher = %row.cipher,
            request_id = %request_id,
            "Unsupported cipher for secret material"
        );
        return Err(ApiError::internal(
            "unsupported_cipher",
            "Unsupported cipher for secret material",
        )
        .with_request_id(request_id));
    }

    let aad = secrets_aad(
        &row.org_id,
        &row.env_id,
        &row.bundle_id,
        &row.version_id,
        &row.data_hash,
    );
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
        ApiError::internal(
            "secrets_decode_failed",
            "Secrets payload was not valid UTF-8",
        )
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
        let (line, truncated) = normalize_log_line(&entry.line, entry.truncated.unwrap_or(false));

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

    builder
        .build()
        .execute(state.db().pool())
        .await
        .map_err(|e| {
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

    let _current_status = sqlx::query_scalar::<_, Option<String>>(
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
            "node_id": node_id_typed.to_string(),
            "status": req.status,
            "boot_id": req.boot_id,
            "exit_code": req.exit_code,
            "reason_code": if req.status == "failed" { req.error_message.as_ref().map(|_| "unknown_error") } else { None },
            "reason_detail": req.error_message,
            "reported_at": chrono::Utc::now().to_rfc3339(),
        }),
        ..Default::default()
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
    overlay_ipv6: Option<String>,
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
            overlay_ipv6: row.try_get("overlay_ipv6")?,
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
            overlay_ipv6: row.overlay_ipv6,
            labels: row.labels,
            allocatable: row.allocatable,
            mtu: row.mtu,
            resource_version: row.resource_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

struct NodePlanNodeRow {
    labels: serde_json::Value,
    mtu: Option<i32>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for NodePlanNodeRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            labels: row.try_get("labels")?,
            mtu: row.try_get("mtu")?,
        })
    }
}

/// Row for instance plan query.
struct InstancePlanRow {
    instance_id: String,
    org_id: String,
    app_id: String,
    env_id: String,
    process_type: String,
    node_id: String,
    desired_state: String,
    generation: i32,
    release_id: String,
    image_ref: String,
    index_or_manifest_digest: String,
    resolved_digests: serde_json::Value,
    manifest_hash: String,
    command: serde_json::Value,
    secrets_version_id: Option<String>,
    overlay_ipv6: Option<String>,
    resources_snapshot: serde_json::Value,
    spec_hash: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for InstancePlanRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            instance_id: row.try_get("instance_id")?,
            org_id: row.try_get("org_id")?,
            app_id: row.try_get("app_id")?,
            env_id: row.try_get("env_id")?,
            process_type: row.try_get("process_type")?,
            node_id: row.try_get("node_id")?,
            desired_state: row.try_get("desired_state")?,
            generation: row.try_get("generation")?,
            release_id: row.try_get("release_id")?,
            image_ref: row.try_get("image_ref")?,
            index_or_manifest_digest: row.try_get("index_or_manifest_digest")?,
            resolved_digests: row.try_get("resolved_digests")?,
            manifest_hash: row.try_get("manifest_hash")?,
            command: row.try_get("command")?,
            secrets_version_id: row.try_get("secrets_version_id")?,
            overlay_ipv6: row.try_get("overlay_ipv6")?,
            resources_snapshot: row.try_get("resources_snapshot")?,
            spec_hash: row.try_get("spec_hash")?,
        })
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
    if line.len() <= MAX_LOG_LINE_BYTES {
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

type VolumeMountMap = HashMap<(String, String), Vec<WorkloadMount>>;

#[derive(Debug, Deserialize)]
struct ResolvedDigestEntry {
    os: String,
    arch: String,
    digest: String,
}

fn label_value(labels: &serde_json::Value, key: &str) -> Option<String> {
    labels
        .get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn assignment_id_from_instance_id(instance_id: &str) -> String {
    match InstanceId::parse(instance_id) {
        Ok(id) => AssignmentId::from_ulid(id.ulid()).to_string(),
        Err(_) => AssignmentId::new().to_string(),
    }
}

fn assignment_from_row(
    row: InstancePlanRow,
    volume_mounts: &VolumeMountMap,
    node_mtu: Option<i32>,
    arch_hint: Option<&str>,
) -> DesiredInstanceAssignment {
    let workload = if desired_state_requires_workload(&row.desired_state) {
        Some(workload_spec_from_row(
            &row,
            volume_mounts,
            node_mtu,
            arch_hint,
        ))
    } else {
        None
    };

    let drain_grace_seconds = if row.desired_state == "draining" {
        Some(DEFAULT_DRAIN_GRACE_SECONDS)
    } else {
        None
    };

    DesiredInstanceAssignment {
        assignment_id: assignment_id_from_instance_id(&row.instance_id),
        node_id: row.node_id,
        instance_id: row.instance_id,
        generation: row.generation,
        desired_state: row.desired_state,
        drain_grace_seconds,
        workload,
    }
}

fn workload_spec_from_row(
    row: &InstancePlanRow,
    volume_mounts: &VolumeMountMap,
    node_mtu: Option<i32>,
    arch_hint: Option<&str>,
) -> WorkloadSpec {
    let command: Vec<String> = serde_json::from_value(row.command.clone()).unwrap_or_default();
    let resources = resources_from_snapshot(&row.resources_snapshot);
    let mounts = volume_mounts
        .get(&(row.env_id.clone(), row.process_type.clone()))
        .cloned()
        .filter(|items| !items.is_empty());
    let secrets = row
        .secrets_version_id
        .as_ref()
        .map(|version_id| WorkloadSecrets {
            required: true,
            secret_version_id: Some(version_id.clone()),
            mount_path: "/run/secrets/platform.env".to_string(),
            mode: None,
            uid: None,
            gid: None,
        });
    let overlay_ipv6 = row
        .overlay_ipv6
        .clone()
        .unwrap_or_else(|| "fd00::1".to_string());
    let network = WorkloadNetwork {
        overlay_ipv6,
        gateway_ipv6: DEFAULT_GATEWAY_IPV6.to_string(),
        mtu: Some(node_mtu.unwrap_or(DEFAULT_MTU)),
        dns: None,
        ports: None,
    };

    WorkloadSpec {
        spec_version: WORKLOAD_SPEC_VERSION.to_string(),
        org_id: row.org_id.clone(),
        app_id: row.app_id.clone(),
        env_id: row.env_id.clone(),
        process_type: row.process_type.clone(),
        instance_id: row.instance_id.clone(),
        generation: row.generation,
        release_id: row.release_id.clone(),
        image: workload_image_from_row(row, arch_hint),
        manifest_hash: row.manifest_hash.clone(),
        command,
        workdir: None,
        env_vars: None,
        resources,
        network,
        mounts,
        secrets,
        spec_hash: Some(row.spec_hash.clone()),
    }
}

fn workload_image_from_row(row: &InstancePlanRow, arch_hint: Option<&str>) -> WorkloadImage {
    let entries = resolved_digest_entries(&row.resolved_digests);
    let resolved = select_resolved_digest(&entries, arch_hint);
    let resolved_digest = resolved
        .map(|entry| entry.digest.clone())
        .unwrap_or_else(|| row.index_or_manifest_digest.clone());
    let os = resolved
        .map(|entry| entry.os.clone())
        .unwrap_or_else(|| "linux".to_string());
    let arch = resolved
        .map(|entry| entry.arch.clone())
        .or_else(|| arch_hint.map(|value| value.to_string()))
        .unwrap_or_else(|| "amd64".to_string());
    let index_digest = if resolved_digest != row.index_or_manifest_digest {
        Some(row.index_or_manifest_digest.clone())
    } else {
        None
    };

    WorkloadImage {
        image_ref: Some(row.image_ref.clone()),
        digest: row.index_or_manifest_digest.clone(),
        index_digest,
        resolved_digest,
        os,
        arch,
    }
}

fn resolved_digest_entries(value: &serde_json::Value) -> Vec<ResolvedDigestEntry> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn select_resolved_digest<'a>(
    entries: &'a [ResolvedDigestEntry],
    arch_hint: Option<&str>,
) -> Option<&'a ResolvedDigestEntry> {
    if let Some(arch) = arch_hint {
        if let Some(entry) = entries.iter().find(|entry| entry.arch == arch) {
            return Some(entry);
        }
    }

    if entries.len() == 1 {
        return entries.first();
    }

    None
}

fn resources_from_snapshot(snapshot: &serde_json::Value) -> WorkloadResources {
    let cpu_request = snapshot
        .get("cpu_request")
        .and_then(|value| value.as_f64())
        .or_else(|| snapshot.get("cpu").and_then(|value| value.as_f64()))
        .unwrap_or(1.0);
    let memory_limit_bytes = snapshot
        .get("memory_limit_bytes")
        .and_then(|value| value.as_i64())
        .or_else(|| {
            snapshot
                .get("memory_bytes")
                .and_then(|value| value.as_i64())
        })
        .unwrap_or(512 * 1024 * 1024);
    let ephemeral_disk_bytes = snapshot
        .get("ephemeral_disk_bytes")
        .and_then(|value| value.as_i64())
        .or(Some(DEFAULT_EPHEMERAL_DISK_BYTES));

    WorkloadResources {
        cpu_request,
        memory_limit_bytes,
        ephemeral_disk_bytes,
        vcpu_count: None,
        cpu_weight: None,
    }
}

fn desired_state_requires_workload(state: &str) -> bool {
    matches!(state, "running" | "draining")
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
            .push(WorkloadMount {
                volume_id: row.volume_id,
                mount_path: row.mount_path,
                read_only: row.read_only,
                filesystem: "ext4".to_string(),
                device_hint: None,
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
            overlay_ipv6: Some("fd00::1".to_string()),
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
