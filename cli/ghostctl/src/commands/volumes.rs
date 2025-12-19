//! Volume commands.
//!
//! Volumes are org-scoped resources that can be attached to env/process types.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

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
struct DeleteResponse {
    ok: bool,
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
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

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
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

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

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created volume {} ({} bytes)",
                response.id, response.size_bytes
            ));
        }
    }

    Ok(())
}

async fn get_volume(ctx: CommandContext, args: GetVolumeArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

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
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let path = format!("/v1/orgs/{org_id}/volumes/{}", args.volume);
    client.delete_with_idempotency_key(&path, None).await?;

    match ctx.format {
        OutputFormat::Json => {
            print_single(&DeleteResponse { ok: true }, ctx.format);
        }
        OutputFormat::Table => {
            print_success(&format!("Deleted volume {}", args.volume));
        }
    }

    Ok(())
}

async fn attach_volume(ctx: CommandContext, args: AttachVolumeArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let app_id = ctx.require_app()?;
    let env_id = require_env(&ctx)?;
    let client = ctx.client()?;

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

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Attached volume {} at {} ({})",
                response.volume_id, response.mount_path, response.id
            ));
        }
    }

    Ok(())
}

async fn detach_volume(ctx: CommandContext, args: DetachVolumeArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let app_id = ctx.require_app()?;
    let env_id = require_env(&ctx)?;
    let client = ctx.client()?;

    let path = format!(
        "/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/volume-attachments/{}",
        args.attachment
    );
    client.delete_with_idempotency_key(&path, None).await?;

    match ctx.format {
        OutputFormat::Json => print_single(&DeleteResponse { ok: true }, ctx.format),
        OutputFormat::Table => print_success(&format!("Detached attachment {}", args.attachment)),
    }

    Ok(())
}

async fn snapshot_create(ctx: CommandContext, args: SnapshotCreateArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

    let path = format!("/v1/orgs/{org_id}/volumes/{}/snapshots", args.volume);
    let request = CreateSnapshotRequest { note: args.note };

    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("snapshots.create", &path, &request)?,
    };

    let response: SnapshotResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created snapshot {} for volume {}",
                response.id, response.volume_id
            ));
        }
    }

    Ok(())
}

async fn snapshot_list(ctx: CommandContext, args: SnapshotListArgs) -> Result<()> {
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

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
    let org_id = ctx.require_org()?;
    let client = ctx.client()?;

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

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!("Restored volume {}", response.id));
        }
    }

    Ok(())
}
