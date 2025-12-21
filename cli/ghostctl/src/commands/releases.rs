//! Release commands.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_receipt, print_single, OutputFormat, ReceiptNextStep};

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

    /// Manifest file path (TOML). If omitted, defaults to ./vt.toml when present.
    #[arg(long, value_name = "PATH")]
    manifest: Option<PathBuf>,

    /// Manifest hash (sha256:...).
    #[arg(long)]
    manifest_hash: Option<String>,
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

#[derive(Debug, Serialize)]
struct CreateReleaseRequest {
    image_ref: String,
    image_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
    command: Vec<String>,
}

/// List all releases for the current app.
async fn list_releases(ctx: CommandContext, args: ListReleasesArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

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
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

    if args.manifest.is_some() && args.manifest_hash.is_some() {
        anyhow::bail!("use either --manifest or --manifest-hash (not both)");
    }

    let (manifest_hash, command) = if let Some(hash) = args.manifest_hash.as_deref() {
        let command = if let Some(path) = args.manifest.as_ref() {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read manifest: {}", path.display()))?;
            command_from_manifest_contents(&contents)?
        } else {
            default_command()
        };
        (hash.to_string(), command)
    } else {
        let path = args.manifest.unwrap_or_else(|| PathBuf::from("vt.toml"));
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read manifest: {}", path.display()))?;
        let manifest_hash = crate::manifest::manifest_hash_from_toml_str(&contents)?;
        let command = command_from_manifest_contents(&contents)?;
        (manifest_hash, command)
    };

    let request = CreateReleaseRequest {
        image_ref: args.image_ref.clone(),
        image_digest: args.image_digest.clone(),
        manifest_schema_version: args.manifest_schema_version,
        manifest_hash,
        command,
    };
    let path = format!("/v1/orgs/{}/apps/{}/releases", org, app);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("releases.create", &path, &request)?,
    };

    let response: ReleaseResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let release_id = response.id.clone();
    let org_id_str = org.to_string();
    let app_id_str = app.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} releases get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                release_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env <env> deploys create {}",
                org_id_str.clone(),
                app_id_str.clone(),
                release_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Debug",
            cmd: format!(
                "vt events tail --org {} --app {}",
                org_id_str.clone(),
                app_id_str.clone()
            ),
        },
    ];

    print_receipt(
        ctx.format,
        &format!(
            "Created release {} for app {}",
            release_id.as_str(),
            app_id_str.as_str()
        ),
        "accepted",
        "releases.create",
        "release",
        &response,
        serde_json::json!({
            "release_id": release_id,
            "app_id": app_id_str,
            "org_id": org_id_str
        }),
        &next,
    );

    Ok(())
}

/// Get release details.
async fn get_release(ctx: CommandContext, args: GetReleaseArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

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

fn default_command() -> Vec<String> {
    vec!["./start".to_string()]
}

fn command_from_manifest_contents(contents: &str) -> Result<Vec<String>> {
    let manifest_json = crate::manifest::manifest_json_from_toml_str(contents)?;
    let Some(processes) = manifest_json.get("processes").and_then(|v| v.as_object()) else {
        anyhow::bail!("manifest missing [processes] section (at least one process type required)");
    };

    let mut keys: Vec<&String> = processes.keys().collect();
    keys.sort();
    let Some(primary) = keys.first() else {
        anyhow::bail!("manifest [processes] must include at least one process type");
    };

    let command = processes
        .get(*primary)
        .and_then(|process| process.get("command"))
        .and_then(|command| command.as_array())
        .map(|command| {
            command
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    if command.is_empty() {
        Ok(default_command())
    } else {
        Ok(command)
    }
}
