//! Routes command (hostname bindings).

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::error::CliError;
use crate::output::{
    print_output, print_receipt, print_receipt_no_resource, print_single, OutputFormat,
    ReceiptNextStep,
};

use super::CommandContext;

/// Routes command.
#[derive(Debug, Args)]
pub struct RoutesCommand {
    #[command(subcommand)]
    command: RoutesSubcommand,
}

#[derive(Debug, Subcommand)]
enum RoutesSubcommand {
    /// List routes for the current environment.
    List(ListRoutesArgs),

    /// Get a single route.
    Get(GetRouteArgs),

    /// Create a route.
    Create(CreateRouteArgs),

    /// Update a route.
    Update(UpdateRouteArgs),

    /// Delete a route.
    Delete(DeleteRouteArgs),
}

#[derive(Debug, Args)]
struct ListRoutesArgs {
    /// Maximum number of items to return (1-200).
    #[arg(long, default_value = "50")]
    limit: i64,

    /// Pagination cursor (opaque).
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Debug, Args)]
struct GetRouteArgs {
    /// Route ID.
    route: String,
}

#[derive(Debug, Args)]
struct CreateRouteArgs {
    /// Hostname to bind (globally unique).
    hostname: String,

    /// Frontend listen port.
    #[arg(long)]
    listen_port: i32,

    /// Protocol hint: tls_passthrough or tcp_raw.
    #[arg(long)]
    protocol_hint: String,

    /// Backend process type.
    #[arg(long)]
    backend_process_type: String,

    /// Backend port.
    #[arg(long)]
    backend_port: i32,

    /// Proxy Protocol mode: off or v2.
    #[arg(long, default_value = "off")]
    proxy_protocol: String,

    /// Whether the backend expects Proxy Protocol (required when proxy_protocol=v2).
    #[arg(long, default_value_t = false)]
    backend_expects_proxy_protocol: bool,

    /// Require a dedicated IPv4 allocation for this route.
    #[arg(long, default_value_t = false)]
    ipv4_required: bool,
}

#[derive(Debug, Args)]
struct UpdateRouteArgs {
    /// Route ID.
    route: String,

    /// Expected current resource version (optimistic concurrency).
    #[arg(long)]
    expected_version: i32,

    /// Backend process type.
    #[arg(long)]
    backend_process_type: Option<String>,

    /// Backend port.
    #[arg(long)]
    backend_port: Option<i32>,

    /// Proxy Protocol mode: off or v2.
    #[arg(long)]
    proxy_protocol: Option<String>,

    /// Whether the backend expects Proxy Protocol.
    #[arg(long)]
    backend_expects_proxy_protocol: Option<bool>,

    /// Whether IPv4 is required.
    #[arg(long)]
    ipv4_required: Option<bool>,
}

#[derive(Debug, Args)]
struct DeleteRouteArgs {
    /// Route ID.
    route: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct RouteResponse {
    #[tabled(rename = "ID")]
    id: String,

    #[tabled(rename = "Env")]
    env_id: String,

    #[tabled(rename = "Hostname")]
    hostname: String,

    #[tabled(rename = "Listen")]
    listen_port: i32,

    #[tabled(rename = "Proto")]
    protocol_hint: String,

    #[tabled(rename = "Backend")]
    backend_process_type: String,

    #[tabled(rename = "Port")]
    backend_port: i32,

    #[tabled(rename = "PP")]
    proxy_protocol: String,

    #[tabled(rename = "IPv4")]
    ipv4_required: bool,

    #[tabled(rename = "Ver")]
    resource_version: i32,

    #[tabled(rename = "Updated")]
    updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListRoutesResponse {
    items: Vec<RouteResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateRouteRequest {
    hostname: String,
    listen_port: i32,
    protocol_hint: String,
    backend_process_type: String,
    backend_port: i32,
    proxy_protocol: String,
    backend_expects_proxy_protocol: bool,
    ipv4_required: bool,
}

#[derive(Debug, Serialize)]
struct UpdateRouteRequest {
    expected_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_process_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_port: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_expects_proxy_protocol: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ipv4_required: Option<bool>,
}

impl RoutesCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            RoutesSubcommand::List(args) => list_routes(ctx, args).await,
            RoutesSubcommand::Get(args) => get_route(ctx, args).await,
            RoutesSubcommand::Create(args) => create_route(ctx, args).await,
            RoutesSubcommand::Update(args) => update_route(ctx, args).await,
            RoutesSubcommand::Delete(args) => delete_route(ctx, args).await,
        }
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

async fn list_routes(ctx: CommandContext, args: ListRoutesArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let mut path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/routes?limit={}",
        org_id, app_id, env_id, args.limit
    );
    if let Some(cursor) = args.cursor.as_deref() {
        path.push_str(&format!("&cursor={cursor}"));
    }

    let response: ListRoutesResponse = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Table => print_output(&response.items, ctx.format),
        OutputFormat::Json => print_single(&response, ctx.format),
    }

    Ok(())
}

async fn get_route(ctx: CommandContext, args: GetRouteArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let response: RouteResponse = client
        .get(&format!(
            "/v1/orgs/{}/apps/{}/envs/{}/routes/{}",
            org_id, app_id, env_id, args.route
        ))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Route '{}' not found", args.route))
            }
            other => other,
        })?;

    print_single(&response, ctx.format);
    Ok(())
}

async fn create_route(ctx: CommandContext, args: CreateRouteArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let request = CreateRouteRequest {
        hostname: args.hostname.clone(),
        listen_port: args.listen_port,
        protocol_hint: args.protocol_hint.clone(),
        backend_process_type: args.backend_process_type.clone(),
        backend_port: args.backend_port,
        proxy_protocol: args.proxy_protocol.clone(),
        backend_expects_proxy_protocol: args.backend_expects_proxy_protocol,
        ipv4_required: args.ipv4_required,
    };
    let path = format!("/v1/orgs/{}/apps/{}/envs/{}/routes", org_id, app_id, env_id);
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("routes.create", &path, &request)?,
    };

    let response: RouteResponse = client
        .post_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let route_id = response.id.clone();
    let hostname = response.hostname.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} routes get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone(),
                route_id.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} routes list",
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
        &format!(
            "Created route '{}' ({}) -> {}:{}",
            hostname,
            route_id.as_str(),
            response.backend_process_type.as_str(),
            response.backend_port
        ),
        "accepted",
        "routes.create",
        "route",
        &response,
        serde_json::json!({
            "route_id": route_id,
            "env_id": env_id_str,
            "app_id": app_id_str,
            "org_id": org_id_str
        }),
        &next,
    );

    Ok(())
}

async fn update_route(ctx: CommandContext, args: UpdateRouteArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let request = UpdateRouteRequest {
        expected_version: args.expected_version,
        backend_process_type: args.backend_process_type.clone(),
        backend_port: args.backend_port,
        proxy_protocol: args.proxy_protocol.clone(),
        backend_expects_proxy_protocol: args.backend_expects_proxy_protocol,
        ipv4_required: args.ipv4_required,
    };
    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/routes/{}",
        org_id, app_id, env_id, args.route
    );
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("routes.update", &path, &request)?,
    };

    let response: RouteResponse = client
        .patch_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Route '{}' not found", args.route))
            }
            other => other,
        })?;

    let route_id = response.id.clone();
    let hostname = response.hostname.clone();
    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} routes get {}",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone(),
                route_id.clone()
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
        &format!("Updated route '{}' ({})", hostname, route_id.as_str()),
        "accepted",
        "routes.update",
        "route",
        &response,
        serde_json::json!({
            "route_id": route_id,
            "env_id": env_id_str,
            "app_id": app_id_str,
            "org_id": org_id_str
        }),
        &next,
    );

    Ok(())
}

async fn delete_route(ctx: CommandContext, args: DeleteRouteArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/routes/{}",
        org_id, app_id, env_id, args.route
    );
    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key_no_body("routes.delete", &path),
    };

    client
        .delete_with_idempotency_key(&path, Some(idempotency_key.as_str()))
        .await
        .map_err(|e| match e {
            CliError::Api { status: 404, .. } => {
                CliError::NotFound(format!("Route '{}' not found", args.route))
            }
            other => other,
        })?;

    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let route_id = args.route.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} routes list",
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

    print_receipt_no_resource(
        ctx.format,
        &format!("Deleted route '{}'", route_id),
        "accepted",
        "routes.delete",
        serde_json::json!({
            "route_id": route_id,
            "env_id": env_id_str,
            "app_id": app_id_str,
            "org_id": org_id_str
        }),
        &next,
    );

    Ok(())
}
