//! CLI commands.

mod apply;
mod apps;
mod auth;
mod context;
mod deploys;
mod envs;
mod events;
mod exec;
mod instances;
mod logs;
mod manifest;
mod nodes;
mod orgs;
mod projects;
mod releases;
mod routes;
mod scale;
mod secrets;
mod volumes;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::client::ApiClient;
use crate::config::{Config, Credentials};
use crate::output::OutputFormat;

/// plfm-vt CLI - Deploy and manage applications on the platform.
#[derive(Debug, Parser)]
#[command(name = "vt")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Output format (table or json).
    #[arg(long, global = true, default_value = "table")]
    format: String,

    /// Organization ID or name.
    #[arg(long, global = true, env = "VT_ORG")]
    org: Option<String>,

    /// Application ID or name.
    #[arg(long, global = true, env = "VT_APP")]
    app: Option<String>,

    /// Environment ID or name.
    #[arg(long, global = true, env = "VT_ENV")]
    env: Option<String>,

    /// Idempotency key to use for write operations.
    ///
    /// If omitted, the CLI generates a deterministic key per request body.
    #[arg(long, global = true)]
    idempotency_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Authenticate with the platform.
    Auth(auth::AuthCommand),

    /// Show or clear saved CLI context.
    Context(context::ContextCommand),

    /// Manage organizations.
    Orgs(orgs::OrgsCommand),

    /// Manage projects.
    Projects(projects::ProjectsCommand),

    /// Manage applications.
    Apps(apps::AppsCommand),

    /// Manage environments.
    Envs(envs::EnvsCommand),

    /// Manage releases (versioned artifacts).
    Releases(releases::ReleasesCommand),

    /// Manage deploys (release to environment).
    Deploys(deploys::DeploysCommand),

    /// Apply a manifest (create release + deploy).
    Apply(apply::ApplyCommand),

    /// Manage nodes (infrastructure).
    Nodes(nodes::NodesCommand),

    /// Manage instances (VM instances).
    Instances(instances::InstancesCommand),

    /// Set process scaling.
    Scale(scale::ScaleCommand),

    /// View application logs.
    Logs(logs::LogsCommand),

    /// Create an exec session grant for an instance.
    Exec(exec::ExecCommand),

    /// Validate and inspect local manifests.
    Manifest(manifest::ManifestCommand),

    /// Query or tail org-scoped events.
    Events(events::EventsCommand),

    /// Manage routes (hostname bindings).
    Routes(routes::RoutesCommand),

    /// Manage environment secrets.
    Secrets(secrets::SecretsCommand),

    /// Manage volumes, attachments, and snapshots.
    Volumes(volumes::VolumesCommand),

    /// Show CLI version.
    Version,
}

impl Cli {
    /// Run the CLI command.
    pub async fn run(self) -> Result<()> {
        let format = match self.format.as_str() {
            "json" => OutputFormat::Json,
            _ => OutputFormat::Table,
        };

        let config = Config::load()?;
        let credentials = Credentials::load()?;

        // Build context from flags and config
        let ctx = CommandContext {
            config,
            credentials,
            format,
            org: self.org,
            app: self.app,
            env: self.env,
            idempotency_key: self.idempotency_key,
        };

        match self.command {
            Commands::Auth(cmd) => cmd.run(ctx).await,
            Commands::Context(cmd) => cmd.run(ctx).await,
            Commands::Orgs(cmd) => cmd.run(ctx).await,
            Commands::Projects(cmd) => cmd.run(ctx).await,
            Commands::Apps(cmd) => cmd.run(ctx).await,
            Commands::Envs(cmd) => cmd.run(ctx).await,
            Commands::Releases(cmd) => cmd.run(ctx).await,
            Commands::Deploys(cmd) => cmd.run(ctx).await,
            Commands::Apply(cmd) => cmd.run(ctx).await,
            Commands::Nodes(cmd) => cmd.run(ctx).await,
            Commands::Instances(cmd) => cmd.run(ctx).await,
            Commands::Scale(cmd) => cmd.run(ctx).await,
            Commands::Logs(cmd) => cmd.run(ctx).await,
            Commands::Exec(cmd) => cmd.run(ctx).await,
            Commands::Manifest(cmd) => cmd.run(ctx).await,
            Commands::Events(cmd) => cmd.run(ctx).await,
            Commands::Routes(cmd) => cmd.run(ctx).await,
            Commands::Secrets(cmd) => cmd.run(ctx).await,
            Commands::Volumes(cmd) => cmd.run(ctx).await,
            Commands::Version => {
                println!("vt {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        }
    }
}

/// Shared command context.
pub struct CommandContext {
    pub config: Config,
    pub credentials: Option<Credentials>,
    pub format: OutputFormat,
    pub org: Option<String>,
    pub app: Option<String>,
    pub env: Option<String>,
    pub idempotency_key: Option<String>,
}

impl CommandContext {
    /// Get an authenticated API client.
    pub fn client(&self) -> Result<ApiClient> {
        ApiClient::new(&self.config, self.credentials.as_ref())
    }

    /// Resolve the current org, preferring flag over context.
    pub fn resolve_org(&self) -> Option<&str> {
        self.org.as_deref().or(self.config.context.org.as_deref())
    }

    /// Resolve the current app, preferring flag over context.
    pub fn resolve_app(&self) -> Option<&str> {
        self.app.as_deref().or(self.config.context.app.as_deref())
    }

    /// Resolve the current env, preferring flag over context.
    pub fn resolve_env(&self) -> Option<&str> {
        self.env.as_deref().or(self.config.context.env.as_deref())
    }

    /// Require an org to be specified.
    pub fn require_org(&self) -> Result<&str> {
        self.resolve_org().ok_or_else(|| {
            anyhow::anyhow!("No organization specified. Use --org or set a default context.")
        })
    }

    /// Require an app to be specified.
    pub fn require_app(&self) -> Result<&str> {
        self.resolve_app().ok_or_else(|| {
            anyhow::anyhow!("No application specified. Use --app or set a default context.")
        })
    }
}
