//! Environment commands.

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{
    print_output, print_receipt, print_single, print_success, OutputFormat, Receipt,
    ReceiptNextStep,
};

use super::CommandContext;

/// Environment commands.
#[derive(Debug, Args)]
pub struct EnvsCommand {
    #[command(subcommand)]
    command: EnvsSubcommand,
}

#[derive(Debug, Subcommand)]
enum EnvsSubcommand {
    /// List environments in an application.
    List(ListEnvsArgs),

    /// Create a new environment.
    Create(CreateEnvArgs),

    #[command(about = "Update environment")]
    Update(UpdateEnvArgs),

    /// Get environment details.
    Get(GetEnvArgs),

    /// Set the default environment in local context.
    Use(UseEnvArgs),
}

#[derive(Debug, Args)]
struct ListEnvsArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateEnvArgs {
    /// Environment name (e.g., production, staging).
    name: String,
}

#[derive(Debug, Args)]
struct UpdateEnvArgs {
    env: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    expected_version: i32,
}

#[derive(Debug, Args)]
struct GetEnvArgs {
    /// Environment ID or name.
    env: String,
}

#[derive(Debug, Args)]
struct UseEnvArgs {
    /// Environment ID or name.
    env: String,
}

impl EnvsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            EnvsSubcommand::List(args) => list_envs(ctx, args).await,
            EnvsSubcommand::Create(args) => create_env(ctx, args).await,
            EnvsSubcommand::Update(args) => update_env(ctx, args).await,
            EnvsSubcommand::Get(args) => get_env(ctx, args).await,
            EnvsSubcommand::Use(args) => use_env(ctx, args).await,
        }
    }
}

/// Environment response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct EnvResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "App ID")]
    app_id: String,

    #[tabled(rename = "Org ID")]
    org_id: String,

    #[tabled(rename = "Name")]
    name: String,

    #[tabled(rename = "Created")]
    created_at: String,
}

/// List response from API.
#[derive(Debug, Serialize, Deserialize)]
struct ListEnvsResponse {
    items: Vec<EnvResponse>,
    next_cursor: Option<String>,
}

/// Create env request.
#[derive(Debug, Serialize)]
struct CreateEnvRequest {
    name: String,
}

#[derive(Debug, Serialize)]
struct UpdateEnvRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    expected_version: i32,
}

/// List all environments in the current app.
async fn list_envs(ctx: CommandContext, args: ListEnvsArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

    let mut path = format!("/v1/orgs/{}/apps/{}/envs?limit={}", org, app, args.limit);
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListEnvsResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Create a new environment.
async fn create_env(ctx: CommandContext, args: CreateEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;

    let request = CreateEnvRequest {
        name: args.name.clone(),
    };
    let path = format!("/v1/orgs/{}/apps/{}/envs", org, app);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("envs.create", &path, &request)?,
    };

    let response: EnvResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let env_id = response.id.clone();
    let env_name = response.name.clone();
    let org_id_str = org.to_string();
    let app_id_str = app.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} envs get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} secrets confirm --none --env {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} deploy --env {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id.clone()
            ),
        },
    ];

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Created environment '{}' ({}) in {}/{}",
                env_name,
                env_id.as_str(),
                org_id_str.as_str(),
                app_id_str.as_str()
            ),
            status: "accepted",
            kind: "envs.create",
            resource_key: "env",
            resource: &response,
            ids: serde_json::json!({
                "env_id": env_id,
                "app_id": app_id_str,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

async fn update_env(ctx: CommandContext, args: UpdateEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, &args.env).await?;

    let request = UpdateEnvRequest {
        name: args.name.clone(),
        expected_version: args.expected_version,
    };
    let path = format!("/v1/orgs/{}/apps/{}/envs/{}", org_id, app_id, env_id);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("envs.update", &path, &request)?,
    };

    let response: EnvResponse = client
        .patch_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Environment '{}' not found", args.env))
            }
            other => other,
        })?;

    let env_id_str = env_id.to_string();
    let env_name = response.name.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} envs get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone()
            ),
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
                "Updated environment '{}' ({})",
                env_name,
                env_id_str.as_str()
            ),
            status: "accepted",
            kind: "envs.update",
            resource_key: "env",
            resource: &response,
            ids: serde_json::json!({
                "env_id": env_id_str,
                "app_id": app_id_str,
                "org_id": org_id_str
            }),
            next: &next,
        },
    );

    Ok(())
}

/// Get environment details.
async fn get_env(ctx: CommandContext, args: GetEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app = crate::resolve::resolve_app_id(&client, org, ctx.require_app()?).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org, app, &args.env).await?;

    let response: EnvResponse = client
        .get(&format!("/v1/orgs/{}/apps/{}/envs/{}", org, app, env_id))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Environment '{}' not found", args.env))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}

/// Set the default environment context.
async fn use_env(mut ctx: CommandContext, args: UseEnvArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, &args.env).await?;

    ctx.config.context.org = Some(org_id.to_string());
    ctx.config.context.app = Some(app_id.to_string());
    ctx.config.context.env = Some(env_id.to_string());
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
        OutputFormat::Table => print_success(&format!(
            "Set default env to {} (app {}, org {})",
            env_id, app_id, org_id
        )),
    }

    Ok(())
}
