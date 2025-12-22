use std::collections::HashMap;
use std::net::Ipv6Addr;

use chrono::Utc;
use plfm_events::{ActorType, AggregateType};
use plfm_id::{AppId, AssignmentId, EnvId, InstanceId, NodeId, OrgId, SecretVersionId, Ulid};
use plfm_proto::agent::v1::{
    node_agent_server::NodeAgent, DesiredInstanceAssignment, EnrollRequest, EnrollResponse,
    GetPlanRequest, GetSecretMaterialRequest, HeartbeatRequest, HeartbeatResponse, NodePlan,
    ReportInstanceStatusRequest, ReportInstanceStatusResponse, SecretMaterial,
    SendWorkloadLogsRequest, SendWorkloadLogsResponse, WorkloadImage, WorkloadMount,
    WorkloadNetwork, WorkloadResources, WorkloadSecrets, WorkloadSpec,
};
use plfm_proto::events::v1::{InstanceDesiredState, InstanceStatus, NodeState};
use sqlx::QueryBuilder;
use tonic::{Request, Response, Status};

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

pub struct NodeAgentService {
    state: AppState,
}

impl NodeAgentService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn map_instance_status_from_proto(status: InstanceStatus) -> &'static str {
        match status {
            InstanceStatus::Booting => "booting",
            InstanceStatus::Ready => "ready",
            InstanceStatus::Draining => "draining",
            InstanceStatus::Stopped => "stopped",
            InstanceStatus::Failed => "failed",
            InstanceStatus::Unspecified => "unknown",
        }
    }
}

#[tonic::async_trait]
impl NodeAgent for NodeAgentService {
    async fn enroll(
        &self,
        request: Request<EnrollRequest>,
    ) -> Result<Response<EnrollResponse>, Status> {
        let req = request.into_inner();
        let request_id = Ulid::new().to_string();

        if req.hostname.is_empty() {
            return Err(Status::invalid_argument("hostname cannot be empty"));
        }
        if req.hostname.len() > 255 {
            return Err(Status::invalid_argument(
                "hostname cannot exceed 255 characters",
            ));
        }
        if req.region.is_empty() {
            return Err(Status::invalid_argument("region cannot be empty"));
        }
        if req.wireguard_public_key.len() < 40 || req.wireguard_public_key.len() > 50 {
            return Err(Status::invalid_argument(
                "invalid WireGuard public key format",
            ));
        }
        if req.cpu_cores < 1 {
            return Err(Status::invalid_argument("cpu_cores must be at least 1"));
        }
        if req.memory_bytes < 1024 * 1024 * 512 {
            return Err(Status::invalid_argument("memory must be at least 512MB"));
        }

        let key_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE wireguard_public_key = $1)",
        )
        .bind(&req.wireguard_public_key)
        .fetch_one(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to check WireGuard key uniqueness");
            Status::internal("failed to verify node")
        })?;

        if key_exists {
            return Err(Status::already_exists(
                "a node with this WireGuard key is already enrolled",
            ));
        }

        let node_id = NodeId::new();
        let overlay_ipv6 = allocate_node_ipv6(self.state.db().pool(), &node_id, &request_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let allocatable = serde_json::json!({
            "cpu_cores": req.cpu_cores,
            "memory_bytes": req.memory_bytes,
        });

        let labels: serde_json::Value = if req.labels.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::to_value(&req.labels).unwrap_or_else(|_| serde_json::json!({}))
        };

        let event = AppendEvent {
            aggregate_type: AggregateType::Node,
            aggregate_id: node_id.to_string(),
            aggregate_seq: 1,
            event_type: "node.enrolled".to_string(),
            event_version: 1,
            actor_type: ActorType::ServicePrincipal,
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
                "public_ipv6": req.public_ipv6,
                "public_ipv4": req.public_ipv4,
                "overlay_ipv6": overlay_ipv6.clone(),
                "cpu_cores": req.cpu_cores,
                "memory_bytes": req.memory_bytes,
                "mtu": req.mtu,
                "labels": labels,
                "allocatable": allocatable,
            }),
            ..Default::default()
        };

        let event_store = self.state.db().event_store();
        event_store.append(event).await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to enroll node");
            Status::internal("failed to enroll node")
        })?;

        tracing::info!(
            node_id = %node_id,
            hostname = %req.hostname,
            region = %req.region,
            request_id = %request_id,
            "Node enrolled via gRPC"
        );

        Ok(Response::new(EnrollResponse {
            node_id: node_id.to_string(),
            overlay_ipv6,
            state: NodeState::Active.into(),
        }))
    }

    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let request_id = Ulid::new().to_string();

        let node_id = request
            .metadata()
            .get("x-node-id")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::invalid_argument("missing x-node-id header"))?
            .to_string();

        let req = request.into_inner();

        let node_state = NodeState::try_from(req.state).unwrap_or(NodeState::Active);
        let node_state_str = match node_state {
            NodeState::Active => "active",
            NodeState::Draining => "draining",
            NodeState::Disabled => "disabled",
            NodeState::Degraded => "degraded",
            NodeState::Offline => "offline",
            NodeState::Unspecified => "active",
        };

        let node_id_typed: NodeId = node_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid node_id format"))?;

        let current_state =
            sqlx::query_scalar::<_, String>("SELECT state FROM nodes_view WHERE node_id = $1")
                .bind(&node_id)
                .fetch_optional(self.state.db().pool())
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to check node existence");
                    Status::internal("failed to verify node")
                })?;

        let current_state = match current_state {
            Some(s) => s,
            None => return Err(Status::not_found(format!("node {} not found", node_id))),
        };

        let event_store = self.state.db().event_store();
        let current_seq = event_store
            .get_latest_aggregate_seq(&AggregateType::Node, &node_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get aggregate sequence");
                Status::internal("failed to process heartbeat")
            })?
            .unwrap_or(0);

        let capacity_event = AppendEvent {
            aggregate_type: AggregateType::Node,
            aggregate_id: node_id.clone(),
            aggregate_seq: current_seq + 1,
            event_type: "node.capacity_updated".to_string(),
            event_version: 1,
            actor_type: ActorType::ServicePrincipal,
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
            }),
            ..Default::default()
        };

        if current_state != node_state_str {
            let state_event = AppendEvent {
                aggregate_type: AggregateType::Node,
                aggregate_id: node_id.clone(),
                aggregate_seq: current_seq + 2,
                event_type: "node.state_changed".to_string(),
                event_version: 1,
                actor_type: ActorType::ServicePrincipal,
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
                    "new_state": node_state_str,
                }),
                ..Default::default()
            };

            event_store
                .append_batch(vec![capacity_event, state_event])
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, request_id = %request_id, "Failed to process heartbeat");
                    Status::internal("failed to process heartbeat")
                })?;

            tracing::info!(
                node_id = %node_id,
                old_state = %current_state,
                new_state = %node_state_str,
                request_id = %request_id,
                "Node state changed"
            );
        } else {
            event_store
                .append_batch(vec![capacity_event])
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, request_id = %request_id, "Failed to process heartbeat");
                    Status::internal("failed to process heartbeat")
                })?;
        }

        Ok(Response::new(HeartbeatResponse {
            accepted: true,
            next_heartbeat_secs: 30,
        }))
    }

    async fn get_plan(
        &self,
        request: Request<GetPlanRequest>,
    ) -> Result<Response<NodePlan>, Status> {
        let req = request.into_inner();
        let request_id = Ulid::new().to_string();

        let _node_id_typed: NodeId = req
            .node_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid node_id format"))?;

        let node_info = sqlx::query_as::<_, NodePlanNodeRow>(
            "SELECT labels, mtu FROM nodes_view WHERE node_id = $1",
        )
        .bind(&req.node_id)
        .fetch_optional(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to load node info");
            Status::internal("failed to get plan")
        })?;

        let node_info = match node_info {
            Some(info) => info,
            None => {
                return Err(Status::not_found(format!("node {} not found", req.node_id)));
            }
        };

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
        .bind(&req.node_id)
        .fetch_all(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, node_id = %req.node_id, "Failed to get node plan");
            Status::internal("failed to get plan")
        })?;

        let event_store = self.state.db().event_store();
        let cursor_event_id = event_store.get_max_event_id().await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to get plan cursor");
            Status::internal("failed to get plan")
        })?;

        let volume_mounts = load_volume_mounts(&self.state, &request_id, &instances)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let arch_hint = label_value(&node_info.labels, "arch");
        let instance_assignments: Vec<DesiredInstanceAssignment> = instances
            .into_iter()
            .map(|row| {
                assignment_from_row(row, &volume_mounts, node_info.mtu, arch_hint.as_deref())
            })
            .collect();

        Ok(Response::new(NodePlan {
            spec_version: NODE_PLAN_SPEC_VERSION.to_string(),
            node_id: req.node_id,
            plan_id: Ulid::new().to_string(),
            cursor_event_id,
            instances: instance_assignments,
        }))
    }

    async fn report_instance_status(
        &self,
        request: Request<ReportInstanceStatusRequest>,
    ) -> Result<Response<ReportInstanceStatusResponse>, Status> {
        let req = request.into_inner();
        let request_id = Ulid::new().to_string();

        let status_report = req
            .status
            .ok_or_else(|| Status::invalid_argument("status is required"))?;

        let node_id_typed: NodeId = req
            .node_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid node_id format"))?;

        let instance_id_typed: InstanceId = status_report
            .instance_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid instance_id format"))?;

        let status =
            InstanceStatus::try_from(status_report.status).unwrap_or(InstanceStatus::Unspecified);
        let status_str = Self::map_instance_status_from_proto(status);

        let valid_statuses = ["booting", "ready", "draining", "stopped", "failed"];
        if !valid_statuses.contains(&status_str) {
            return Err(Status::invalid_argument(format!(
                "status must be one of: {:?}",
                valid_statuses
            )));
        }

        let instance_info = sqlx::query_as::<_, InstanceInfoRow>(
            r#"
            SELECT org_id, app_id, env_id
            FROM instances_desired_view
            WHERE instance_id = $1 AND node_id = $2
            "#,
        )
        .bind(instance_id_typed.to_string())
        .bind(node_id_typed.to_string())
        .fetch_optional(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get instance info");
            Status::internal("failed to process status")
        })?;

        let instance_info = match instance_info {
            Some(info) => info,
            None => {
                return Err(Status::not_found("instance not found on this node"));
            }
        };

        let event_store = self.state.db().event_store();
        let current_seq = event_store
            .get_latest_aggregate_seq(&AggregateType::Instance, &instance_id_typed.to_string())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get aggregate sequence");
                Status::internal("failed to process status")
            })?
            .unwrap_or(0);

        let org_id = instance_info
            .org_id
            .parse::<OrgId>()
            .map_err(|_| Status::internal("invalid org_id in instances_desired_view"))?;
        let app_id = instance_info
            .app_id
            .parse::<AppId>()
            .map_err(|_| Status::internal("invalid app_id in instances_desired_view"))?;
        let env_id = instance_info
            .env_id
            .parse::<EnvId>()
            .map_err(|_| Status::internal("invalid env_id in instances_desired_view"))?;

        let event = AppendEvent {
            aggregate_type: AggregateType::Instance,
            aggregate_id: instance_id_typed.to_string(),
            aggregate_seq: current_seq + 1,
            event_type: "instance.status_changed".to_string(),
            event_version: 1,
            actor_type: ActorType::ServicePrincipal,
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
                "status": status_str,
                "boot_id": status_report.boot_id,
                "exit_code": status_report.exit_code,
                "reason_code": if status_str == "failed" { status_report.error_message.as_ref().map(|_| "unknown_error") } else { None },
                "reason_detail": status_report.error_message,
                "reported_at": chrono::Utc::now().to_rfc3339(),
            }),
            ..Default::default()
        };

        event_store.append(event).await.map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to record status");
            Status::internal("failed to record status")
        })?;

        Ok(Response::new(ReportInstanceStatusResponse {
            accepted: true,
        }))
    }

    async fn get_secret_material(
        &self,
        request: Request<GetSecretMaterialRequest>,
    ) -> Result<Response<SecretMaterial>, Status> {
        let req = request.into_inner();
        let request_id = Ulid::new().to_string();

        let _node_id_typed: NodeId = req
            .node_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid node_id format"))?;

        let version_id_typed: SecretVersionId = req
            .version_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid secret version_id format"))?;

        let node_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)",
        )
        .bind(&req.node_id)
        .fetch_one(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to check node existence");
            Status::internal("failed to load secrets")
        })?;

        if !node_exists {
            return Err(Status::not_found(format!("node {} not found", req.node_id)));
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
        .fetch_optional(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to load secret material");
            Status::internal("failed to load secrets")
        })?;

        let row = match row {
            Some(r) => r,
            None => return Err(Status::not_found("secret version not found")),
        };

        if row.cipher != secrets_crypto::CIPHER_NAME {
            tracing::error!(
                cipher = %row.cipher,
                request_id = %request_id,
                "Unsupported cipher for secret material"
            );
            return Err(Status::internal("unsupported cipher for secret material"));
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
            Status::internal("failed to decrypt secrets")
        })?;

        let data = String::from_utf8(plaintext)
            .map_err(|_| Status::internal("secrets payload was not valid UTF-8"))?;

        Ok(Response::new(SecretMaterial {
            version_id: row.version_id,
            format: row.format,
            data_hash: row.data_hash,
            data,
        }))
    }

    async fn send_workload_logs(
        &self,
        request: Request<SendWorkloadLogsRequest>,
    ) -> Result<Response<SendWorkloadLogsResponse>, Status> {
        let req = request.into_inner();
        let request_id = Ulid::new().to_string();

        let node_id_typed: NodeId = req
            .node_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid node_id format"))?;

        let node_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM nodes_view WHERE node_id = $1)",
        )
        .bind(node_id_typed.to_string())
        .fetch_one(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to check node existence");
            Status::internal("failed to ingest logs")
        })?;

        if !node_exists {
            return Err(Status::not_found(format!("node {} not found", req.node_id)));
        }

        if req.entries.is_empty() {
            return Ok(Response::new(SendWorkloadLogsResponse {
                accepted: 0,
                rejected: 0,
            }));
        }

        if req.entries.len() > MAX_LOG_ENTRIES {
            return Err(Status::invalid_argument(format!(
                "log batch exceeds max of {} entries",
                MAX_LOG_ENTRIES
            )));
        }

        let mut instance_ids: Vec<String> =
            req.entries.iter().map(|e| e.instance_id.clone()).collect();
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
        .fetch_all(self.state.db().pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, request_id = %request_id, "Failed to load instance metadata");
            Status::internal("failed to ingest logs")
        })?;

        let mut instance_meta: HashMap<String, InstanceLogMetaRow> = HashMap::new();
        for row in instance_rows {
            if row.node_id == req.node_id {
                instance_meta.insert(row.instance_id.clone(), row);
            }
        }

        let mut accepted_entries: Vec<WorkloadLogRow> = Vec::new();
        let mut rejected = 0i32;

        for entry in req.entries {
            let Some(meta) = instance_meta.get(&entry.instance_id) else {
                rejected += 1;
                continue;
            };

            let stream = normalize_log_stream(&entry.stream);
            let (line, truncated) = normalize_log_line(&entry.line, entry.truncated);
            let ts = chrono::DateTime::from_timestamp_nanos(entry.timestamp_nanos);

            accepted_entries.push(WorkloadLogRow {
                org_id: meta.org_id.clone(),
                app_id: meta.app_id.clone(),
                env_id: meta.env_id.clone(),
                process_type: meta.process_type.clone(),
                instance_id: entry.instance_id,
                node_id: req.node_id.clone(),
                ts,
                stream,
                line,
                truncated,
            });
        }

        if accepted_entries.is_empty() {
            return Ok(Response::new(SendWorkloadLogsResponse {
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
            .execute(self.state.db().pool())
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to insert workload logs");
                Status::internal("failed to ingest logs")
            })?;

        Ok(Response::new(SendWorkloadLogsResponse {
            accepted: accepted_entries.len() as i32,
            rejected,
        }))
    }
}

async fn allocate_node_ipv6(
    pool: &sqlx::PgPool,
    node_id: &NodeId,
    request_id: &str,
) -> Result<String, String> {
    let prefix = std::env::var("PLFM_NODE_IPV6_PREFIX")
        .or_else(|_| std::env::var("GHOST_NODE_IPV6_PREFIX"))
        .unwrap_or_else(|_| "fd00:0:0:1::".to_string());

    let base: Ipv6Addr = prefix.parse().map_err(|_| {
        format!(
            "invalid node IPv6 prefix '{}'; expected /64 base address",
            prefix
        )
    })?;

    let base_u128 = u128::from(base) & (!0u128 << 64);
    let mut attempts = 0;

    loop {
        let suffix: i64 = sqlx::query_scalar("SELECT nextval('ipam_node_suffix_seq')")
            .fetch_one(pool)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, request_id = %request_id, "Failed to allocate node IPv6 suffix");
                "failed to allocate node IPv6 suffix".to_string()
            })?;

        if suffix < 0 {
            attempts += 1;
            if attempts > 5 {
                return Err("failed to allocate node IPv6 suffix".to_string());
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
                        return Err("ipam allocation retry limit reached".to_string());
                    }
                    continue;
                }
                tracing::error!(
                    error = %db_err,
                    request_id = %request_id,
                    "Failed to allocate node overlay IPv6"
                );
                return Err("failed to allocate node overlay IPv6".to_string());
            }
            Err(e) => {
                tracing::error!(error = %e, request_id = %request_id, "Failed to allocate node overlay IPv6");
                return Err("failed to allocate node overlay IPv6".to_string());
            }
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
    ts: chrono::DateTime<Utc>,
    stream: String,
    line: String,
    truncated: bool,
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

type VolumeMountMap = HashMap<(String, String), Vec<VolumeMountData>>;

#[derive(Clone)]
struct VolumeMountData {
    volume_id: String,
    mount_path: String,
    read_only: bool,
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

async fn load_volume_mounts(
    state: &AppState,
    request_id: &str,
    instances: &[InstancePlanRow],
) -> Result<VolumeMountMap, String> {
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
        "failed to load volume mounts".to_string()
    })?;

    let mut mounts: VolumeMountMap = HashMap::new();
    for row in rows {
        mounts
            .entry((row.env_id, row.process_type))
            .or_default()
            .push(VolumeMountData {
                volume_id: row.volume_id,
                mount_path: row.mount_path,
                read_only: row.read_only,
            });
    }

    Ok(mounts)
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

    let desired_state = match row.desired_state.as_str() {
        "running" => InstanceDesiredState::Running,
        "draining" => InstanceDesiredState::Draining,
        "stopped" => InstanceDesiredState::Stopped,
        _ => InstanceDesiredState::Unspecified,
    };

    DesiredInstanceAssignment {
        assignment_id: assignment_id_from_instance_id(&row.instance_id),
        node_id: row.node_id,
        instance_id: row.instance_id,
        generation: row.generation,
        desired_state: desired_state.into(),
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
    let mounts: Vec<WorkloadMount> = volume_mounts
        .get(&(row.env_id.clone(), row.process_type.clone()))
        .map(|items| {
            items
                .iter()
                .map(|m| WorkloadMount {
                    volume_id: m.volume_id.clone(),
                    mount_path: m.mount_path.clone(),
                    read_only: m.read_only,
                    filesystem: "ext4".to_string(),
                    device_hint: None,
                })
                .collect()
        })
        .unwrap_or_default();

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
        dns: vec![],
        ports: vec![],
    };

    let env_vars: HashMap<String, String> = HashMap::new();

    WorkloadSpec {
        spec_version: WORKLOAD_SPEC_VERSION.to_string(),
        org_id: row.org_id.clone(),
        app_id: row.app_id.clone(),
        env_id: row.env_id.clone(),
        process_type: row.process_type.clone(),
        instance_id: row.instance_id.clone(),
        generation: row.generation,
        release_id: row.release_id.clone(),
        image: Some(workload_image_from_row(row, arch_hint)),
        manifest_hash: row.manifest_hash.clone(),
        command,
        workdir: None,
        env_vars,
        resources: Some(resources),
        network: Some(network),
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

#[derive(Debug, serde::Deserialize)]
struct ResolvedDigestEntry {
    os: String,
    arch: String,
    digest: String,
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

fn normalize_log_stream(stream: &str) -> String {
    match stream {
        "stdout" => "stdout".to_string(),
        "stderr" => "stderr".to_string(),
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
