//! CLI commands.

mod apps;
mod auth;
mod deploys;
mod envs;
mod instances;
mod logs;
mod nodes;
mod orgs;
mod releases;
mod scale;

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

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Authenticate with the platform.
    Auth(auth::AuthCommand),

    /// Manage organizations.
    Orgs(orgs::OrgsCommand),

    /// Manage applications.
    Apps(apps::AppsCommand),

    /// Manage environments.
    Envs(envs::EnvsCommand),

    /// Manage releases (versioned artifacts).
    Releases(releases::ReleasesCommand),

    /// Manage deploys (release to environment).
    Deploys(deploys::DeploysCommand),

    /// Manage nodes (infrastructure).
    Nodes(nodes::NodesCommand),

    /// Manage instances (VM instances).
    Instances(instances::InstancesCommand),

    /// Set process scaling.
    Scale(scale::ScaleCommand),

    /// View application logs.
    Logs(logs::LogsCommand),

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
        };

        match self.command {
            Commands::Auth(cmd) => cmd.run(ctx).await,
            Commands::Orgs(cmd) => cmd.run(ctx).await,
            Commands::Apps(cmd) => cmd.run(ctx).await,
            Commands::Envs(cmd) => cmd.run(ctx).await,
            Commands::Releases(cmd) => cmd.run(ctx).await,
            Commands::Deploys(cmd) => cmd.run(ctx).await,
            Commands::Nodes(cmd) => cmd.run(ctx).await,
            Commands::Instances(cmd) => cmd.run(ctx).await,
            Commands::Scale(cmd) => cmd.run(ctx).await,
            Commands::Logs(cmd) => cmd.run(ctx).await,
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
}

impl CommandContext {
    /// Get an authenticated API client.
    pub fn client(&self) -> Result<ApiClient> {
        ApiClient::new(&self.config, self.credentials.as_ref())
    }

    /// Get an unauthenticated API client.
    pub fn client_unauthenticated(&self) -> Result<ApiClient> {
        ApiClient::unauthenticated(&self.config)
    }

    /// Resolve the current org, preferring flag over context.
    pub fn resolve_org(&self) -> Option<&str> {
        self.org
            .as_deref()
            .or(self.config.context.org.as_deref())
    }

    /// Resolve the current app, preferring flag over context.
    pub fn resolve_app(&self) -> Option<&str> {
        self.app
            .as_deref()
            .or(self.config.context.app.as_deref())
    }

    /// Resolve the current env, preferring flag over context.
    pub fn resolve_env(&self) -> Option<&str> {
        self.env
            .as_deref()
            .or(self.config.context.env.as_deref())
    }

    /// Require an org to be specified.
    pub fn require_org(&self) -> Result<&str> {
        self.resolve_org()
            .ok_or_else(|| anyhow::anyhow!("No organization specified. Use --org or set a default context."))
    }

    /// Require an app to be specified.
    pub fn require_app(&self) -> Result<&str> {
        self.resolve_app()
            .ok_or_else(|| anyhow::anyhow!("No application specified. Use --app or set a default context."))
    }
}
