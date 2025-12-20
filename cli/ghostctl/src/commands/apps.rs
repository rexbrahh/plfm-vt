//! Application commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{print_output, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Application commands.
#[derive(Debug, Args)]
pub struct AppsCommand {
    #[command(subcommand)]
    command: AppsSubcommand,
}

#[derive(Debug, Subcommand)]
enum AppsSubcommand {
    /// List applications in an organization.
    List(ListAppsArgs),

    /// Create a new application.
    Create(CreateAppArgs),

    /// Get application details.
    Get(GetAppArgs),

    /// Set the default application in local context.
    Use(UseAppArgs),
}

#[derive(Debug, Args)]
struct ListAppsArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateAppArgs {
    /// Application name.
    name: String,

    /// Optional description.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, Args)]
struct GetAppArgs {
    /// Application ID or name.
    app: String,
}

#[derive(Debug, Args)]
struct UseAppArgs {
    /// Application ID or name.
    app: String,
}

impl AppsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            AppsSubcommand::List(args) => list_apps(ctx, args).await,
            AppsSubcommand::Create(args) => create_app(ctx, args).await,
            AppsSubcommand::Get(args) => get_app(ctx, args).await,
            AppsSubcommand::Use(args) => use_app(ctx, args).await,
        }
    }
}

/// Application response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct AppResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Org ID")]
    org_id: String,

    #[tabled(rename = "Name")]
    name: String,

    #[tabled(rename = "Description", display = "display_option")]
    #[serde(default)]
    description: Option<String>,

    #[tabled(rename = "Created")]
    created_at: String,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

/// List response from API.
#[derive(Debug, Serialize, Deserialize)]
struct ListAppsResponse {
    items: Vec<AppResponse>,
    next_cursor: Option<String>,
}

/// Create app request.
#[derive(Debug, Serialize)]
struct CreateAppRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// List all applications in the current org.
async fn list_apps(ctx: CommandContext, args: ListAppsArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let mut path = format!("/v1/orgs/{}/apps?limit={}", org, args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListAppsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Create a new application.
async fn create_app(ctx: CommandContext, args: CreateAppArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;

    let request = CreateAppRequest {
        name: args.name.clone(),
        description: args.description,
    };

    let path = format!("/v1/orgs/{}/apps", org);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("apps.create", &path, &request)?,
    };
    let response: AppResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    match ctx.format {
        OutputFormat::Json => print_single(&response, ctx.format),
        OutputFormat::Table => {
            print_success(&format!(
                "Created application '{}' ({}) in org {}",
                response.name, response.id, org
            ));
        }
    }

    Ok(())
}

/// Get application details.
async fn get_app(ctx: CommandContext, args: GetAppArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org, &args.app).await?;

    let response: AppResponse = client
        .get(&format!("/v1/orgs/{}/apps/{}", org, app_id))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Application '{}' not found", args.app))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}

/// Set the default application context (and optionally env via `--env`).
async fn use_app(mut ctx: CommandContext, args: UseAppArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, &args.app).await?;

    let env_id = match ctx.env.as_deref() {
        None => None,
        Some(env_ident) => {
            Some(crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?)
        }
    };

    ctx.config.context.org = Some(org_id.to_string());
    ctx.config.context.app = Some(app_id.to_string());
    ctx.config.context.env = env_id.map(|id| id.to_string());
    ctx.config.save()?;

    match ctx.format {
        OutputFormat::Json => print_single(
            &serde_json::json!({
                "ok": true,
                "org_id": org_id,
                "app_id": app_id,
                "env_id": env_id,
            }),
            ctx.format,
        ),
        OutputFormat::Table => match env_id {
            Some(env_id) => print_success(&format!(
                "Set default app to {} (org {}) and env to {}",
                app_id, org_id, env_id
            )),
            None => print_success(&format!(
                "Set default app to {} (org {}) (cleared env)",
                app_id, org_id
            )),
        },
    }

    Ok(())
}
