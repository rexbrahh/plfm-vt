use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use plfm_proto::agent::v1::{
    node_agent_client::NodeAgentClient, GetPlanRequest, GetSecretMaterialRequest,
    HeartbeatRequest as ProtoHeartbeatRequest, ReportInstanceStatusRequest,
    SendWorkloadLogsRequest, WorkloadLogEntry,
};
use plfm_proto::events::v1::{
    InstanceDesiredState as ProtoInstanceDesiredState, InstanceStatus as ProtoInstanceStatus,
    NodeState as ProtoNodeState,
};
use tonic::transport::Channel;
use tonic::Request;
use tracing::debug;

use crate::config::Config;

pub struct ControlPlaneGrpcClient {
    client: NodeAgentClient<Channel>,
    node_id: String,
}

impl ControlPlaneGrpcClient {
    pub async fn connect(config: &Config) -> Result<Self> {
        let channel = Channel::from_shared(config.control_plane_grpc_url.clone())?
            .timeout(Duration::from_secs(30))
            .connect()
            .await?;

        Ok(Self {
            client: NodeAgentClient::new(channel),
            node_id: config.node_id.to_string(),
        })
    }

    pub async fn fetch_plan(&mut self) -> Result<NodePlan> {
        debug!(node_id = %self.node_id, "Fetching node plan via gRPC");

        let request = GetPlanRequest {
            node_id: self.node_id.clone(),
        };

        let response = self.client.get_plan(request).await?;
        let proto_plan = response
            .into_inner()
            .plan
            .context("missing node plan in GetPlanResponse")?;

        let instances: Vec<DesiredInstanceAssignment> = proto_plan
            .instances
            .into_iter()
            .map(|i| {
                let desired_state = ProtoInstanceDesiredState::try_from(i.desired_state)
                    .unwrap_or(ProtoInstanceDesiredState::Unspecified);
                DesiredInstanceAssignment {
                    assignment_id: i.assignment_id,
                    node_id: i.node_id,
                    instance_id: i.instance_id,
                    generation: i.generation,
                    desired_state: map_instance_desired_state(desired_state),
                    drain_grace_seconds: i.drain_grace_seconds,
                    workload: i.workload.map(|w| InstancePlan {
                        spec_version: w.spec_version,
                        org_id: w.org_id,
                        app_id: w.app_id,
                        env_id: w.env_id,
                        process_type: w.process_type,
                        instance_id: w.instance_id,
                        generation: w.generation,
                        release_id: w.release_id,
                        image: w
                            .image
                            .map(|img| WorkloadImage {
                                image_ref: img.image_ref,
                                digest: img.digest,
                                index_digest: img.index_digest,
                                resolved_digest: img.resolved_digest,
                                os: img.os,
                                arch: img.arch,
                            })
                            .unwrap_or_default(),
                        manifest_hash: w.manifest_hash,
                        command: w.command,
                        workdir: w.workdir,
                        env_vars: if w.env_vars.is_empty() {
                            None
                        } else {
                            Some(w.env_vars)
                        },
                        resources: w
                            .resources
                            .map(|r| WorkloadResources {
                                cpu_request: r.cpu_request,
                                memory_limit_bytes: r.memory_limit_bytes,
                                ephemeral_disk_bytes: r.ephemeral_disk_bytes,
                                vcpu_count: r.vcpu_count,
                                cpu_weight: r.cpu_weight,
                            })
                            .unwrap_or_default(),
                        network: w
                            .network
                            .map(|n| WorkloadNetwork {
                                overlay_ipv6: n.overlay_ipv6,
                                gateway_ipv6: n.gateway_ipv6,
                                mtu: n.mtu,
                                dns: if n.dns.is_empty() { None } else { Some(n.dns) },
                                ports: if n.ports.is_empty() {
                                    None
                                } else {
                                    Some(
                                        n.ports
                                            .into_iter()
                                            .map(|p| WorkloadPort {
                                                name: p.name,
                                                port: p.port,
                                                protocol: p.protocol,
                                            })
                                            .collect(),
                                    )
                                },
                            })
                            .unwrap_or_default(),
                        mounts: if w.mounts.is_empty() {
                            None
                        } else {
                            Some(
                                w.mounts
                                    .into_iter()
                                    .map(|m| WorkloadMount {
                                        volume_id: m.volume_id,
                                        mount_path: m.mount_path,
                                        read_only: m.read_only,
                                        filesystem: m.filesystem,
                                        device_hint: m.device_hint,
                                    })
                                    .collect(),
                            )
                        },
                        secrets: w.secrets.map(|s| WorkloadSecrets {
                            required: s.required,
                            secret_version_id: s.secret_version_id,
                            mount_path: s.mount_path,
                            mode: s.mode,
                            uid: s.uid,
                            gid: s.gid,
                        }),
                        spec_hash: w.spec_hash,
                    }),
                }
            })
            .collect();

        debug!(
            cursor_event_id = proto_plan.cursor_event_id,
            instance_count = instances.len(),
            "Fetched node plan via gRPC"
        );

        Ok(NodePlan {
            spec_version: proto_plan.spec_version,
            node_id: proto_plan.node_id,
            plan_id: proto_plan.plan_id,
            created_at: Utc::now(),
            cursor_event_id: proto_plan.cursor_event_id,
            instances,
        })
    }

    pub async fn report_instance_status(&mut self, status: &InstanceStatusReport) -> Result<()> {
        debug!(
            instance_id = %status.instance_id,
            status = %status.status,
            "Reporting instance status via gRPC"
        );

        let proto_status = plfm_proto::agent::v1::InstanceStatusReport {
            instance_id: status.instance_id.clone(),
            status: map_instance_status_to_proto(&status.status).into(),
            boot_id: status.boot_id.clone(),
            error_message: status.error_message.clone(),
            exit_code: status.exit_code,
        };

        let request = ReportInstanceStatusRequest {
            node_id: self.node_id.clone(),
            status: Some(proto_status),
        };

        self.client.report_instance_status(request).await?;
        Ok(())
    }

    pub async fn fetch_secret_material(
        &mut self,
        version_id: &str,
    ) -> Result<SecretMaterialResponse> {
        debug!(version_id = %version_id, "Fetching secret material via gRPC");

        let request = GetSecretMaterialRequest {
            node_id: self.node_id.clone(),
            version_id: version_id.to_string(),
        };

        let response = self.client.get_secret_material(request).await?;
        let proto_material = response
            .into_inner()
            .material
            .context("missing secret material in GetSecretMaterialResponse")?;

        Ok(SecretMaterialResponse {
            version_id: proto_material.version_id,
            format: proto_material.format,
            data_hash: proto_material.data_hash,
            data: proto_material.data,
        })
    }

    pub async fn send_workload_logs(&mut self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let proto_entries: Vec<WorkloadLogEntry> = entries
            .into_iter()
            .map(|e| WorkloadLogEntry {
                timestamp_nanos: e.ts.timestamp_nanos_opt().unwrap_or(0),
                instance_id: e.instance_id,
                stream: e.stream,
                line: e.line,
                truncated: e.truncated,
            })
            .collect();

        let request = SendWorkloadLogsRequest {
            node_id: self.node_id.clone(),
            entries: proto_entries,
        };

        self.client.send_workload_logs(request).await?;
        Ok(())
    }

    pub async fn send_heartbeat(
        &mut self,
        request: &ClientHeartbeatRequest,
    ) -> Result<ClientHeartbeatResponse> {
        let mut grpc_request = Request::new(ProtoHeartbeatRequest {
            state: map_node_state_to_proto(&request.state).into(),
            available_cpu_cores: request.available_cpu_cores,
            available_memory_bytes: request.available_memory_bytes,
            instance_count: request.instance_count,
        });

        grpc_request
            .metadata_mut()
            .insert("x-node-id", self.node_id.parse().unwrap());

        let response = self.client.heartbeat(grpc_request).await?;
        let inner = response.into_inner();

        Ok(ClientHeartbeatResponse {
            accepted: inner.accepted,
            next_heartbeat_secs: inner.next_heartbeat_secs,
        })
    }
}

fn map_instance_desired_state(state: ProtoInstanceDesiredState) -> LocalInstanceDesiredState {
    match state {
        ProtoInstanceDesiredState::Running => LocalInstanceDesiredState::Running,
        ProtoInstanceDesiredState::Draining => LocalInstanceDesiredState::Draining,
        ProtoInstanceDesiredState::Stopped => LocalInstanceDesiredState::Stopped,
        ProtoInstanceDesiredState::Unspecified => LocalInstanceDesiredState::Stopped,
    }
}

fn map_instance_status_to_proto(status: &InstanceStatus) -> ProtoInstanceStatus {
    match status {
        InstanceStatus::Booting => ProtoInstanceStatus::Booting,
        InstanceStatus::Ready => ProtoInstanceStatus::Ready,
        InstanceStatus::Draining => ProtoInstanceStatus::Draining,
        InstanceStatus::Stopped => ProtoInstanceStatus::Stopped,
        InstanceStatus::Failed => ProtoInstanceStatus::Failed,
    }
}

fn map_node_state_to_proto(state: &ClientNodeState) -> ProtoNodeState {
    match state {
        ClientNodeState::Active => ProtoNodeState::Active,
        ClientNodeState::Draining => ProtoNodeState::Draining,
        ClientNodeState::Disabled => ProtoNodeState::Disabled,
        ClientNodeState::Degraded => ProtoNodeState::Degraded,
        ClientNodeState::Offline => ProtoNodeState::Offline,
    }
}

#[derive(Debug, Clone)]
pub struct NodePlan {
    pub spec_version: String,
    pub node_id: String,
    pub plan_id: String,
    pub created_at: DateTime<Utc>,
    pub cursor_event_id: i64,
    pub instances: Vec<DesiredInstanceAssignment>,
}

#[derive(Debug, Clone)]
pub struct DesiredInstanceAssignment {
    pub assignment_id: String,
    pub node_id: String,
    pub instance_id: String,
    pub generation: i32,
    pub desired_state: LocalInstanceDesiredState,
    pub drain_grace_seconds: Option<i32>,
    pub workload: Option<InstancePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalInstanceDesiredState {
    Running,
    Draining,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct InstancePlan {
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
    pub workdir: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
    pub resources: WorkloadResources,
    pub network: WorkloadNetwork,
    pub mounts: Option<Vec<WorkloadMount>>,
    pub secrets: Option<WorkloadSecrets>,
    pub spec_hash: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkloadImage {
    pub image_ref: Option<String>,
    pub digest: String,
    pub index_digest: Option<String>,
    pub resolved_digest: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkloadResources {
    pub cpu_request: f64,
    pub memory_limit_bytes: i64,
    pub ephemeral_disk_bytes: Option<i64>,
    pub vcpu_count: Option<i32>,
    pub cpu_weight: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkloadNetwork {
    pub overlay_ipv6: String,
    pub gateway_ipv6: String,
    pub mtu: Option<i32>,
    pub dns: Option<Vec<String>>,
    pub ports: Option<Vec<WorkloadPort>>,
}

#[derive(Debug, Clone)]
pub struct WorkloadPort {
    pub name: String,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone)]
pub struct WorkloadMount {
    pub volume_id: String,
    pub mount_path: String,
    pub read_only: bool,
    pub filesystem: String,
    pub device_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkloadSecrets {
    pub required: bool,
    pub secret_version_id: Option<String>,
    pub mount_path: String,
    pub mode: Option<i32>,
    pub uid: Option<i32>,
    pub gid: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct SecretMaterialResponse {
    pub version_id: String,
    pub format: String,
    pub data_hash: String,
    pub data: String,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    pub instance_id: String,
    pub stream: String,
    pub line: String,
    pub truncated: bool,
}

#[derive(Debug)]
pub struct InstanceStatusReport {
    pub instance_id: String,
    pub status: InstanceStatus,
    pub boot_id: Option<String>,
    pub error_message: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Booting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceStatus::Booting => write!(f, "booting"),
            InstanceStatus::Ready => write!(f, "ready"),
            InstanceStatus::Draining => write!(f, "draining"),
            InstanceStatus::Stopped => write!(f, "stopped"),
            InstanceStatus::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug)]
pub struct ClientHeartbeatRequest {
    pub state: ClientNodeState,
    pub available_cpu_cores: i32,
    pub available_memory_bytes: i64,
    pub instance_count: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum ClientNodeState {
    Active,
    Draining,
    Disabled,
    Degraded,
    Offline,
}

#[derive(Debug)]
pub struct ClientHeartbeatResponse {
    pub accepted: bool,
    pub next_heartbeat_secs: i32,
}
