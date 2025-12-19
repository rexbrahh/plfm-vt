//! Release commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Release commands.
#[derive(Debug, Args)]
pub struct ReleasesCommand {
    #[command(subcommand)]
    command: ReleasesSubcommand,
}

#[derive(Debug, Subcommand)]
enum ReleasesSubcommand {
    /// List releases for an application.
    List(ListReleasesArgs),

    /// Create a new release.
    Create(CreateReleaseArgs),

    /// Get release details.
    Get(GetReleaseArgs),
}

#[derive(Debug, Args)]
struct ListReleasesArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateReleaseArgs {
    /// OCI image reference (e.g., ghcr.io/org/app@sha256:...).
    image_ref: String,

    /// Image digest (sha256:...).
    #[arg(long)]
    image_digest: String,

    /// Manifest schema version.
    #[arg(long, default_value_t = 1)]
    manifest_schema_version: i32,

    /// Manifest hash (sha256:...).
    #[arg(long)]
    manifest_hash: String,
}

#[derive(Debug, Args)]
struct GetReleaseArgs {
    /// Release ID.
    release: String,
}

impl ReleasesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            ReleasesSubcommand::List(args) => list_releases(ctx, args).await,
            ReleasesSubcommand::Create(args) => create_release(ctx, args).await,
            ReleasesSubcommand::Get(args) => get_release(ctx, args).await,
        }
    }
}

/// Release response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct ReleaseResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Org")]
    org_id: String,

    #[tabled(rename = "App ID")]
    app_id: String,

    #[tabled(rename = "Image Ref")]
    image_ref: String,

    #[tabled(rename = "Image Digest")]
    image_digest: String,

    #[tabled(rename = "Manifest Ver")]
    manifest_schema_version: i32,

    #[tabled(rename = "Manifest Hash")]
    manifest_hash: String,

    #[tabled(rename = "Ver")]
    resource_version: i32,

    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Serialize, Deserialize)]
struct ListReleasesResponse {
    items: Vec<ReleaseResponse>,
    next_cursor: Option<String>,
}

/// Create release request.
#[derive(Debug, Serialize)]
struct CreateReleaseRequest {
    image_ref: String,
    image_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
}

/// List all releases for the current app.
async fn list_releases(ctx: CommandContext, args: ListReleasesArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let mut path = format!(
        "/v1/orgs/{}/apps/{}/releases?limit={}",
        org, app, args.limit
    );
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListReleasesResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Create a new release.
async fn create_release(ctx: CommandContext, args: CreateReleaseArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let request = CreateReleaseRequest {
        image_ref: args.image_ref.clone(),
        image_digest: args.image_digest.clone(),
        manifest_schema_version: args.manifest_schema_version,
        manifest_hash: args.manifest_hash.clone(),
    };

    let response: ReleaseResponse = client
        .post(&format!("/v1/orgs/{}/apps/{}/releases", org, app), &request)
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!("Created release {} for app {}", response.id, app));
        }
    }

    Ok(())
}

/// Get release details.
async fn get_release(ctx: CommandContext, args: GetReleaseArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let client = ctx.client()?;

    let response: ReleaseResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/releases/{}",
            org, app, args.release
        ))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Release '{}' not found", args.release))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
