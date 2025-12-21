//! Secrets commands.
//!
//! Secrets are env-scoped and versioned. The CLI never prints secret values.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use tabled::Tabled;

use crate::output::{print_receipt, print_single, OutputFormat, ReceiptNextStep};

use super::CommandContext;

/// Secrets commands.
#[derive(Debug, Args)]
pub struct SecretsCommand {
    #[command(subcommand)]
    command: SecretsSubcommand,
}

#[derive(Debug, Subcommand)]
enum SecretsSubcommand {
    /// Get secrets metadata for the current environment.
    Get,

    /// Set secrets for the current environment (creates a new version).
    Set(SetSecretsArgs),

    /// Confirm that this environment has no secrets (creates an empty version).
    Confirm(ConfirmSecretsArgs),
}

#[derive(Debug, Args)]
struct SetSecretsArgs {
    /// Set secrets from a platform secrets env file.
    #[arg(long, value_name = "PATH", conflicts_with = "values")]
    env_file: Option<PathBuf>,

    /// Set secrets from key/value pairs (repeatable): --value KEY=VALUE
    #[arg(long = "value", value_name = "KEY=VALUE")]
    values: Vec<String>,
}

#[derive(Debug, Args)]
struct ConfirmSecretsArgs {
    /// Acknowledge that this environment has no secrets.
    #[arg(long)]
    none: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
struct SecretsMetadata {
    #[tabled(rename = "Env ID")]
    env_id: String,
    #[tabled(rename = "Bundle ID")]
    bundle_id: String,
    #[tabled(rename = "Version ID")]
    current_version_id: String,
    #[tabled(rename = "Updated")]
    updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum PutSecretsRequest {
    EnvFile(PutSecretsEnvFileRequest),
    Map(PutSecretsMapRequest),
}

#[derive(Debug, Serialize)]
struct PutSecretsEnvFileRequest {
    format: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct PutSecretsMapRequest {
    values: BTreeMap<String, String>,
}

impl SecretsCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            SecretsSubcommand::Get => get_secrets(ctx).await,
            SecretsSubcommand::Set(args) => set_secrets(ctx, args).await,
            SecretsSubcommand::Confirm(args) => confirm_secrets_none(ctx, args).await,
        }
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

async fn get_secrets(ctx: CommandContext) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/secrets",
        org_id, app_id, env_id
    );
    let metadata: SecretsMetadata = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => print_single(&metadata, ctx.format),
        OutputFormat::Table => print_single(&metadata, ctx.format),
    }

    Ok(())
}

pub(super) async fn ensure_secrets_configured(
    client: &crate::client::ApiClient,
    org_id: plfm_id::OrgId,
    app_id: plfm_id::AppId,
    env_id: plfm_id::EnvId,
) -> Result<(), crate::error::CliError> {
    let path = format!("/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets");
    let _: SecretsMetadata = client.get(&path).await.map_err(|e| match e {
        crate::error::CliError::Api { status: 404, .. } => crate::error::CliError::NotFound(
            format!(
                "No secrets configured for env {}. Run `vt secrets set ...` or `vt secrets confirm --none`.",
                env_id
            ),
        ),
        other => other,
    })?;
    Ok(())
}

async fn set_secrets(ctx: CommandContext, args: SetSecretsArgs) -> Result<()> {
    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let path = format!(
        "/v1/orgs/{}/apps/{}/envs/{}/secrets",
        org_id, app_id, env_id
    );

    let request = if let Some(env_file) = args.env_file {
        let data = std::fs::read_to_string(&env_file)
            .with_context(|| format!("failed to read secrets env file: {}", env_file.display()))?;
        PutSecretsRequest::EnvFile(PutSecretsEnvFileRequest {
            format: "platform_env_v1".to_string(),
            data,
        })
    } else if !args.values.is_empty() {
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for kv in args.values {
            let Some((k, v)) = kv.split_once('=') else {
                anyhow::bail!("Invalid --value '{kv}'. Expected KEY=VALUE");
            };
            values.insert(k.to_string(), v.to_string());
        }
        PutSecretsRequest::Map(PutSecretsMapRequest { values })
    } else {
        anyhow::bail!("Provide either --env-file or at least one --value KEY=VALUE");
    };

    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => crate::idempotency::default_idempotency_key("secrets.put", &path, &request)?,
    };

    let response: SecretsMetadata = client
        .put_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let bundle_id = response.bundle_id.clone();
    let version_id = response.current_version_id.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} secrets get",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} deploy",
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
            "Updated secrets for {}/{}/{} (version {})",
            org_id_str.as_str(),
            app_id_str.as_str(),
            env_id_str.as_str(),
            version_id
        ),
        "accepted",
        "secrets.set",
        "secrets",
        &response,
        serde_json::json!({
            "org_id": org_id_str,
            "app_id": app_id_str,
            "env_id": env_id_str,
            "bundle_id": bundle_id,
            "version_id": version_id
        }),
        &next,
    );

    Ok(())
}

async fn confirm_secrets_none(ctx: CommandContext, args: ConfirmSecretsArgs) -> Result<()> {
    if !args.none {
        anyhow::bail!("Only `--none` is supported (use: vt secrets confirm --none)");
    }

    let client = ctx.client()?;
    let org_id = crate::resolve::resolve_org_id(&client, ctx.require_org()?).await?;
    let app_id = crate::resolve::resolve_app_id(&client, org_id, ctx.require_app()?).await?;
    let env_id =
        crate::resolve::resolve_env_id(&client, org_id, app_id, require_env(&ctx)?).await?;

    let path = format!("/v1/orgs/{org_id}/apps/{app_id}/envs/{env_id}/secrets");
    let request = PutSecretsRequest::Map(PutSecretsMapRequest {
        values: BTreeMap::new(),
    });

    let idempotency_key = match ctx.idempotency_key.as_deref() {
        Some(key) => key.to_string(),
        None => {
            crate::idempotency::default_idempotency_key("secrets.confirm_none", &path, &request)?
        }
    };

    let response: SecretsMetadata = client
        .put_with_idempotency_key(&path, &request, Some(idempotency_key.as_str()))
        .await?;

    let org_id_str = org_id.to_string();
    let app_id_str = app_id.to_string();
    let env_id_str = env_id.to_string();
    let bundle_id = response.bundle_id.clone();
    let version_id = response.current_version_id.clone();
    let next = vec![
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} secrets get",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone()
            ),
        },
        ReceiptNextStep {
            label: "Next",
            cmd: format!(
                "vt --org {} --app {} --env {} deploy",
                org_id_str.clone(),
                app_id_str.clone(),
                env_id_str.clone()
            ),
        },
    ];

    print_receipt(
        ctx.format,
        &format!(
            "Confirmed no secrets for {}/{}/{} (version {})",
            org_id_str.as_str(),
            app_id_str.as_str(),
            env_id_str.as_str(),
            version_id
        ),
        "accepted",
        "secrets.confirm_none",
        "secrets",
        &response,
        serde_json::json!({
            "org_id": org_id_str,
            "app_id": app_id_str,
            "env_id": env_id_str,
            "bundle_id": bundle_id,
            "version_id": version_id
        }),
        &next,
    );

    Ok(())
}
