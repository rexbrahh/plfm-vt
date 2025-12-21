//! Deploy commands.

use std::time::Duration;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;
use tokio::time::{sleep, Instant};

use crate::client::ApiClient;
use crate::error::CliError;
use crate::output::{
    print_info, print_output, print_receipt, print_single, OutputFormat, Receipt, ReceiptNextStep,
};

use super::CommandContext;

/// Default timeout for waiting on deploy convergence.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Polling interval for deploy status checks.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Deploy commands.
#[derive(Debug, Args)]
pub struct DeploysCommand {
    #[command(subcommand)]
    command: DeploysSubcommand,
}

#[derive(Debug, Subcommand)]
enum DeploysSubcommand {
    /// List deploys for an environment.
    List(ListDeploysArgs),

    /// Create a new deploy (deploy a release to an environment).
    Create(CreateDeployArgs),

    /// Create a rollback (select a previous release).
    Rollback(RollbackArgs),

    /// Get deploy details.
    Get(GetDeployArgs),
}

#[derive(Debug, Args)]
struct ListDeploysArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct CreateDeployArgs {
    /// Release ID to deploy.
    release: String,

    /// Process type(s) to deploy (repeatable).
    #[arg(long)]
    process_type: Vec<String>,

    /// Deploy strategy (v1 only supports rolling).
    #[arg(long, default_value = "rolling")]
    strategy: String,

    /// Wait for deploy to complete before returning.
    #[arg(long)]
    wait: bool,

    /// Timeout for waiting (e.g., "5m", "300s"). Default is 5 minutes.
    #[arg(long, value_name = "DURATION")]
    wait_timeout: Option<String>,

    /// Do not wait for deploy (default behavior, explicit flag for clarity).
    #[arg(long, conflicts_with = "wait")]
    no_wait: bool,
}

#[derive(Debug, Args)]
struct RollbackArgs {
    /// Release ID to roll back to.
    release: String,

    /// Wait for rollback to complete before returning.
    #[arg(long)]
    wait: bool,

    /// Timeout for waiting (e.g., "5m", "300s"). Default is 5 minutes.
    #[arg(long, value_name = "DURATION")]
    wait_timeout: Option<String>,

    /// Do not wait for rollback (default behavior, explicit flag for clarity).
    #[arg(long, conflicts_with = "wait")]
    no_wait: bool,
}

#[derive(Debug, Args)]
struct GetDeployArgs {
    /// Deploy ID.
    deploy: String,
}

impl DeploysCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            DeploysSubcommand::List(args) => list_deploys(ctx, args).await,
            DeploysSubcommand::Create(args) => create_deploy(ctx, args).await,
            DeploysSubcommand::Rollback(args) => rollback(ctx, args).await,
            DeploysSubcommand::Get(args) => get_deploy(ctx, args).await,
        }
    }
}

/// Deploy response from API.
#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct DeployResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Org")]
    org_id: String,

    #[tabled(rename = "App")]
    app_id: String,

    #[tabled(rename = "Env ID")]
    env_id: String,

    #[tabled(rename = "Kind")]
    kind: String,

    #[tabled(rename = "Release ID")]
    release_id: String,

    #[tabled(rename = "Processes", display = "display_process_types")]
    process_types: Vec<String>,

    #[tabled(rename = "Status")]
    status: String,

    #[tabled(rename = "Message", display = "display_option")]
    #[serde(default)]
    message: Option<String>,

    #[tabled(rename = "Ver")]
    resource_version: i32,

    #[tabled(rename = "Created")]
    created_at: String,

    #[tabled(rename = "Updated")]
    updated_at: String,
}

fn display_option(opt: &Option<String>) -> String {
    opt.as_deref().unwrap_or("-").to_string()
}

fn display_process_types(process_types: &[String]) -> String {
    if process_types.is_empty() {
        "-".to_string()
    } else {
        process_types.join(",")
    }
}

/// List response from API.
#[derive(Debug, Serialize, Deserialize)]
struct ListDeploysResponse {
    items: Vec<DeployResponse>,
    next_cursor: Option<String>,
}

/// Create deploy request.
#[derive(Debug, Serialize)]
struct CreateDeployRequest {
    release_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_types: Option<Vec<String>>,
    strategy: String,
}

/// Rollback request.
#[derive(Debug, Serialize)]
struct RollbackRequest {
    release_id: String,
}

/// Terminal deploy statuses that indicate the deploy is done.
const TERMINAL_STATUSES: &[&str] = &["completed", "failed", "cancelled"];

/// Parse a duration string like "5m", "300s", "2h" into a Duration.
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("duration cannot be empty");
    }

    // Try to parse as just seconds first
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }

    // Parse with suffix
    let (num_str, unit) = s.split_at(s.len().saturating_sub(1));
    let num: u64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration format: {}", s))?;

    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 60 * 60)),
        _ => anyhow::bail!("invalid duration unit '{}', expected s/m/h", unit),
    }
}

/// Wait for a deploy to reach a terminal status.
///
/// Returns Ok(status) on success, or Err if timeout/failure.
async fn wait_for_deploy(
    client: &ApiClient,
    org_id: plfm_id::OrgId,
    app_id: plfm_id::AppId,
    env_id: plfm_id::EnvId,
    deploy_id: &str,
    timeout: Duration,
    format: OutputFormat,
) -> Result<DeployResponse> {
    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/deploys/{}",
        org_id, app_id, env_id, deploy_id
    );

    let start = Instant::now();
    let mut last_status = String::new();

    loop {
        let response: DeployResponse = client.get(&path).await?;

        // Print status updates (only in table mode, and only when status changes)
        if matches!(format, OutputFormat::Table) && response.status != last_status {
            print_info(&format!("Deploy {} status: {}", deploy_id, response.status));
            last_status = response.status.clone();
        }

        // Check if terminal status
        if TERMINAL_STATUSES.contains(&response.status.as_str()) {
            if response.status == "completed" {
                return Ok(response);
            } else {
                anyhow::bail!(
                    "Deploy {} {}: {}",
                    deploy_id,
                    response.status,
                    response.message.as_deref().unwrap_or("no details")
                );
            }
        }

        // Check timeout
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timeout waiting for deploy {} to complete (last status: {})",
                deploy_id,
                response.status
            );
        }

        sleep(POLL_INTERVAL).await;
    }
}

/// Require an env to be specified.
fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

/// List all deploys for the current env.
async fn list_deploys(ctx: CommandContext, args: ListDeploysArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, org).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env).await?;

    let mut path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/deploys?limit={}",
        org_id, app_id, env_id, args.limit
    );
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListDeploysResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }
    Ok(())
}

/// Create a new deploy.
async fn create_deploy(ctx: CommandContext, args: CreateDeployArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, org).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env).await?;

    super::secrets::ensure_secrets_configured(&client, org_id, app_id, env_id).await?;

    // Parse wait timeout if provided
    let wait_timeout = match args.wait_timeout.as_deref() {
        Some(t) => parse_duration(t)?,
        None => DEFAULT_WAIT_TIMEOUT,
    };

    let request = CreateDeployRequest {
        release_id: args.release.clone(),
        process_types: if args.process_type.is_empty() {
            None
        } else {
            Some(args.process_type)
        },
        strategy: args.strategy,
    };
    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/deploys",
        org_id, app_id, env_id
    );
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("deploys.create", &path, &request)?,
    };

    let response: DeployResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let deploy_id = response.id.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} deploys get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone(),
                deploy_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} instances list",
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

    if !args.wait {
        print_receipt(
            ctx.format,
            Receipt {
                message: format!(
                    "Created deploy {} for env {}",
                    deploy_id.as_str(),
                    env_id_str.as_str()
                ),
                status: "accepted",
                kind: "deploys.create",
                resource_key: "deploy",
                resource: &response,
                ids: serde_json::json!({
                    "deploy_id": deploy_id,
                    "env_id": env_id_str,
                    "app_id": app_id_str,
                    "org_id": org_id_str
                }),
                next: &next,
            },
        );
        return Ok(());
    }

    let final_response = wait_for_deploy(
        &client,
        org_id,
        app_id,
        env_id,
        &deploy_id,
        wait_timeout,
        ctx.format,
    )
    .await?;

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Deploy {} completed with status {}",
                deploy_id.as_str(),
                final_response.status.as_str()
            ),
            status: final_response.status.as_str(),
            kind: "deploys.create",
            resource_key: "deploy",
            resource: &final_response,
            ids: serde_json::json!({
                "deploy_id": deploy_id,
                "env_id": env_id.to_string(),
                "app_id": app_id.to_string(),
                "org_id": org_id.to_string()
            }),
            next: &next,
        },
    );

    Ok(())
}

/// Create a rollback (represented as a deploy).
async fn rollback(ctx: CommandContext, args: RollbackArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, org).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env).await?;

    super::secrets::ensure_secrets_configured(&client, org_id, app_id, env_id).await?;

    // Parse wait timeout if provided
    let wait_timeout = match args.wait_timeout.as_deref() {
        Some(t) => parse_duration(t)?,
        None => DEFAULT_WAIT_TIMEOUT,
    };

    let request = RollbackRequest {
        release_id: args.release.clone(),
    };
    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/rollbacks",
        org_id, app_id, env_id
    );
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("rollbacks.create", &path, &request)?,
    };

    let response: DeployResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let deploy_id = response.id.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} deploys get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone(),
                deploy_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} status",
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

    if !args.wait {
        print_receipt(
            ctx.format,
            Receipt {
                message: format!(
                    "Created rollback {} for env {}",
                    deploy_id.as_str(),
                    env_id_str.as_str()
                ),
                status: "accepted",
                kind: "rollbacks.create",
                resource_key: "deploy",
                resource: &response,
                ids: serde_json::json!({
                    "deploy_id": deploy_id,
                    "env_id": env_id_str,
                    "app_id": app_id_str,
                    "org_id": org_id_str
                }),
                next: &next,
            },
        );
        return Ok(());
    }

    let final_response = wait_for_deploy(
        &client,
        org_id,
        app_id,
        env_id,
        &deploy_id,
        wait_timeout,
        ctx.format,
    )
    .await?;

    print_receipt(
        ctx.format,
        Receipt {
            message: format!(
                "Rollback {} completed with status {}",
                deploy_id.as_str(),
                final_response.status.as_str()
            ),
            status: final_response.status.as_str(),
            kind: "rollbacks.create",
            resource_key: "deploy",
            resource: &final_response,
            ids: serde_json::json!({
                "deploy_id": deploy_id,
                "env_id": env_id.to_string(),
                "app_id": app_id.to_string(),
                "org_id": org_id.to_string()
            }),
            next: &next,
        },
    );

    Ok(())
}

/// Get deploy details.
async fn get_deploy(ctx: CommandContext, args: GetDeployArgs) -> Result<()> {
    let org = ctx.require_org()?;
    let app = ctx.require_app()?;
    let env = require_env(&ctx)?;
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, org).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, app).await?;
    let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env).await?;

    let response: DeployResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/deploys/{}",
            org_id, app_id, env_id, args.deploy
        ))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Deploy '{}' not found", args.deploy))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}
