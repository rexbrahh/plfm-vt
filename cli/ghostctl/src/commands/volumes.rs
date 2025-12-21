//! Volume commands.
//!
//! Volumes are org-scoped resources that can be attached to env/process types.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{
    print_output, print_receipt, print_receipt_no_resource, print_single, OutputFormat, Receipt,
    ReceiptNextStep, ReceiptNoResource,
};

use super::CommandContext;

/// Volume commands.
#[derive(Debug, Args)]
pub struct VolumesCommand {
    #[command(subcommand)]
    command: VolumesSubcommand,
}

#[derive(Debug, Subcommand)]
enum VolumesSubcommand {
    /// List volumes (org scoped).
    List(ListVolumesArgs),

    /// Create a new volume.
    Create(CreateVolumeArgs),

    /// Get a volume.
    Get(GetVolumeArgs),

    /// Delete a volume (idempotent).
    Delete(DeleteVolumeArgs),

    /// Attach a volume to the current env/process type.
    Attach(AttachVolumeArgs),

    /// Detach a volume attachment (idempotent).
    Detach(DetachVolumeArgs),

    /// Create a snapshot.
    SnapshotCreate(SnapshotCreateArgs),

    /// List snapshots for a volume.
    SnapshotList(SnapshotListArgs),

    /// Restore a volume from a snapshot (creates a new volume).
    Restore(RestoreVolumeArgs),
}

#[derive(Debug, Args)]
struct ListVolumesArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateVolumeArgs {
    /// Optional volume name (label).
    #[arg(long)]
    name: Option<String>,

    /// Volume size in bytes (>= 1073741824).
    #[arg(long, value_name = "BYTES")]
    size_bytes: i64,

    /// Filesystem type.
    #[arg(long, default_value = "ext4")]
    filesystem: String,

    /// Disable backups for this volume (default: backups enabled).
    #[arg(long)]
    no_backup: bool,
}

#[derive(Debug, Args)]
struct GetVolumeArgs {
    /// Volume ID.
    volume: String,
}

#[derive(Debug, Args)]
struct DeleteVolumeArgs {
    /// Volume ID.
    volume: String,
}

#[derive(Debug, Args)]
struct AttachVolumeArgs {
    /// Volume ID.
    volume: String,

    /// Process type in the env (e.g. web).
    #[arg(long)]
    process_type: String,

    /// Mount path inside the guest (absolute).
    #[arg(long)]
    mount_path: String,

    /// Attach read-only.
    #[arg(long)]
    read_only: bool,
}

#[derive(Debug, Args)]
struct DetachVolumeArgs {
    /// Attachment ID.
    attachment: String,
}

#[derive(Debug, Args)]
struct SnapshotCreateArgs {
    /// Volume ID.
    volume: String,

    /// Optional note.
    #[arg(long)]
    note: Option<String>,
}

#[derive(Debug, Args)]
struct SnapshotListArgs {
    /// Volume ID.
    volume: String,

    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct RestoreVolumeArgs {
    /// Source volume ID (must match snapshot's volume_id).
    volume: String,

    /// Snapshot ID.
    #[arg(long)]
    snapshot_id: String,

    /// Name for the new volume.
    #[arg(long)]
    new_volume_name: Option<String>,
}

impl VolumesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            VolumesSubcommand::List(args) => list_volumes(ctx, args).await,
            VolumesSubcommand::Create(args) => create_volume(ctx, args).await,
            VolumesSubcommand::Get(args) => get_volume(ctx, args).await,
            VolumesSubcommand::Delete(args) => delete_volume(ctx, args).await,
            VolumesSubcommand::Attach(args) => attach_volume(ctx, args).await,
            VolumesSubcommand::Detach(args) => detach_volume(ctx, args).await,
            VolumesSubcommand::SnapshotCreate(args) => snapshot_create(ctx, args).await,
            VolumesSubcommand::SnapshotList(args) => snapshot_list(ctx, args).await,
            VolumesSubcommand::Restore(args) => restore_volume(ctx, args).await,
        }
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VolumeAttachmentResponse {
    id: String,
    volume_id: String,
    env_id: String,
    process_type: String,
    mount_path: String,
    #[serde(default)]
    read_only: bool,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VolumeResponse {
    id: String,
    org_id: String,
    #[serde(default)]
    name: Option<String>,
    size_bytes: i64,
    filesystem: String,
    created_at: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    attachments: Vec<VolumeAttachmentResponse>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListVolumesResponse {
    items: Vec<VolumeResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateVolumeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    size_bytes: i64,
    filesystem: String,
    backup_enabled: bool,
}

#[derive(Debug, Serialize)]
struct CreateVolumeAttachmentRequest {
    volume_id: String,
    process_type: String,
    mount_path: String,
    #[serde(default)]
    read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotResponse {
    id: String,
    volume_id: String,
    created_at: String,
    status: String,
    #[serde(default)]
    size_bytes: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListSnapshotsResponse {
    items: Vec<SnapshotResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateSnapshotRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct RestoreVolumeRequest {
    snapshot_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_volume_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Tabled)]
struct VolumeListRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Size Bytes")]
    size_bytes: i64,
    #[tabled(rename = "FS")]
    filesystem: String,
    #[tabled(rename = "Attachments")]
    attachments: usize,
    #[tabled(rename = "Created")]
    created_at: String,
}

impl From<&VolumeResponse> for VolumeListRow {
    fn from(v: &VolumeResponse) -> Self {
        Self {
            id: v.id.clone(),
            name: v.name.clone().unwrap_or_else(|| "-".to_string()),
            size_bytes: v.size_bytes,
            filesystem: v.filesystem.clone(),
            attachments: v.attachments.len(),
            created_at: v.created_at.clone(),
        }
    }
}

async fn list_volumes(ctx: CommandContext, args: ListVolumesArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let mut path = format!("/v1/orgs/{org_id}/volumes?limit={}", args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListVolumesResponse = client.get(&path).await?;
    match ctx.format {
        OutputFormat::Table => {
            let rows: Vec<VolumeListRow> = response.items.iter().map(VolumeListRow::from).collect();
            print_output(&rows, ctx.format);
        }
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

async fn create_volume(ctx: CommandContext, args: CreateVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let request = CreateVolumeRequest {
        name: args.name.clone(),
        size_bytes: args.size_bytes,
        filesystem: args.filesystem.clone(),
        backup_enabled: !args.no_backup,
    };

    let path = format!("/v1/orgs/{org_id}/volumes");
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("volumes.create", &path, &request)?,
    };

    let response: VolumeResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let volume_id = response.id.clone();
    let org_id_str = org_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes get {}", org_id_str.clone(), volume_id),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes list", org_id_str.clone()),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Created volume {} ({} bytes)",
                response.id.as_str(),
                response.size_bytes
            ),
            status: "accepted",
            kind: "volumes.create",
            resource_key: "volume",
            resource: &response,
            ids: serde_json::json!({
                "volume_id": response.id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn get_volume(ctx: CommandContext, args: GetVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let response: VolumeResponse = client
        .get(&format!("/v1/orgs/{org_id}/volumes/{}", args.volume))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Volume '{}' not found", args.volume))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}

async fn delete_volume(ctx: CommandContext, args: DeleteVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let path = format!("/v1/orgs/{org_id}/volumes/{}", args.volume);
    client.delete_with_idempotency_key(&path, None).await?;

    let org_id_str = org_id.to_string();
    let volume_id = args.volume.clone();
    let next = vec![ReceiptNextStep {
        label: "Next",
        cmd: format!("vt --org {} volumes list", org_id_str.clone()),
    }];

    print_receipt_no_resource(
        ctx.format,
        ReceiptNoResource {
            message: format!("Deleted volume {}", volume_id),
            status: "accepted",
            kind: "volumes.delete",
            ids: serde_json::json!({
                "volume_id": volume_id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn attach_volume(ctx: CommandContext, args: AttachVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let request = CreateVolumeAttachmentRequest {
        volume_id: args.volume.clone(),
        process_type: args.process_type.clone(),
        mount_path: args.mount_path.clone(),
        read_only: args.read_only,
    };

    let path = format!("/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments");
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key(
            "volume_attachments.create",
            &path,
            &request,
        )?,
    };

    let response: VolumeAttachmentResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let attachment_id = response.id.clone();
    let volume_id = response.volume_id.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes get {}", org_id_str.clone(), volume_id),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!(
                "vt events tail --org {} --app {} --env {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone()
            ),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Attached volume {} at {} ({})",
                response.volume_id.as_str(),
                response.mount_path.as_str(),
                attachment_id.as_str()
            ),
            status: "accepted",
            kind: "volume_attachments.create",
            resource_key: "volume_attachment",
            resource: &response,
            ids: serde_json::json!({
                "attachment_id": attachment_id,
                "volume_id": response.volume_id,
                "env_id": env_id_str,
                "app_id": app_id_str,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn detach_volume(ctx: CommandContext, args: DetachVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let path = format!(
        "/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments/{}",
        args.attachment
    );
    client.delete_with_idempotency_key(&path, None).await?;

    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let attachment_id = args.attachment.clone();
    let next = vec![ReceiptNextStep {
        label: "Next",
        cmd: format!("vt --org {} volumes list", org_id_str.clone()),
    }];

    print_receipt_no_resource(
        ctx.format,
        ReceiptNoResource {
            message: format!("Detached attachment {}", attachment_id),
            status: "accepted",
            kind: "volume_attachments.delete",
            ids: serde_json::json!({
                "attachment_id": attachment_id,
                "env_id": env_id_str,
                "app_id": app_id_str,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn snapshot_create(ctx: CommandContext, args: SnapshotCreateArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let path = format!("/v1/orgs/{org_id}/volumes/{}/snapshots", args.volume);
    let request = CreateSnapshotRequest { note: args.note };

    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("snapshots.create", &path, &request)?,
    };

    let response: SnapshotResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let snapshot_id = response.id.clone();
    let volume_id = response.volume_id.clone();
    let org_id_str = org_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} volumes snapshot-list {}",
                org_id_str.clone(),
                volume_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes get {}", org_id_str.clone(), volume_id),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Created snapshot {} for volume {}",
                snapshot_id.as_str(),
                response.volume_id.as_str()
            ),
            status: "accepted",
            kind: "snapshots.create",
            resource_key: "snapshot",
            resource: &response,
            ids: serde_json::json!({
                "snapshot_id": snapshot_id,
                "volume_id": response.volume_id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn snapshot_list(ctx: CommandContext, args: SnapshotListArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let mut path = format!(
        "/v1/orgs/{org_id}/volumes/{}/snapshots?limit={}",
        args.volume, args.limit
    );
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListSnapshotsResponse = client.get(&path).await?;
    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => print_single(&response.items, ctx.format),
    }

    Ok(())
}

async fn restore_volume(ctx: CommandContext, args: RestoreVolumeArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let path = format!("/v1/orgs/{org_id}/volumes/{}/restore", args.volume);
    let request = RestoreVolumeRequest {
        snapshot_id: args.snapshot_id,
        new_volume_name: args.new_volume_name,
    };

    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("volumes.restore", &path, &request)?,
    };

    let response: VolumeResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let volume_id = response.id.clone();
    let org_id_str = org_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes get {}", org_id_str.clone(), volume_id),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!("vt --org {} volumes list", org_id_str.clone()),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!("Restored volume {}", response.id.as_str()),
            status: "accepted",
            kind: "volumes.restore",
            resource_key: "volume",
            resource: &response,
            ids: serde_json::json!({
                "volume_id": response.id,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}
