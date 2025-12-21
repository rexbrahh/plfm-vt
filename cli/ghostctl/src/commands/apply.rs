//! Apply command (manifest-first workflow).
//!
//! v1: `vt deploy` creates a release from the local manifest + image digest,
//! then creates a deploy for the selected environment.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, Instant};

use crate::client::ApiClient;
use crate::manifest::ManifestValidationError;
use crate::output::{
    print_info, print_receipt, print_single, OutputFormat, Receipt, ReceiptNextStep,
};

use super::CommandContext;

/// Default timeout for waiting on deploy convergence.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Polling interval for deploy status checks.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Terminal deploy statuses that indicate the deploy is done.
const TERMINAL_STATUSES: &[&str] = &["succeeded", "failed"];

/// Apply a manifest (create release + deploy).
#[derive(Debug, Args)]
pub struct ApplyCommand {
    /// Manifest file path (TOML). Defaults to ./vt.toml.
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<PathBuf>,

    /// Image digest (sha256:...). If omitted, `image.ref` must be a digest reference (contains `@sha256:...`).
    #[arg(long)]
    pub image_digest: Option<String>,

    /// Deploy only these process types (repeatable). Defaults to all manifest process types.
    #[arg(long = "process-type")]
    pub process_type: Vec<String>,

    /// Print the plan without making any API calls.
    #[arg(long)]
    pub dry_run: bool,

    /// Wait for deploy to complete before returning.
    #[arg(long)]
    pub wait: bool,

    /// Timeout for waiting (e.g., "5m", "300s"). Default is 5 minutes.
    #[arg(long, value_name = "DURATION")]
    pub wait_timeout: Option<String>,

    /// Do not wait for deploy (default behavior, explicit flag for clarity).
    #[arg(long, conflicts_with = "wait")]
    pub no_wait: bool,
}

#[derive(Debug, Serialize)]
struct CreateReleaseRequest {
    image_ref: String,
    image_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    id: String,
}

#[derive(Debug, Serialize)]
struct CreateDeployRequest {
    release_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_types: Option<Vec<String>>,
    strategy: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeployResponse {
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApplyPlan {
    dry_run: bool,
    org_id: String,
    app_id: String,
    env_id: String,
    manifest_path: String,
    manifest_hash: String,
    image_ref: String,
    image_digest: String,
    process_types: Vec<String>,
    command: Vec<String>,
    strategy: String,
}

#[derive(Debug, Serialize)]
struct ApplyReceipt {
    release_id: String,
    deploy_id: String,
    manifest_hash: String,
    image_ref: String,
    image_digest: String,
    process_types: Vec<String>,
}

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
            if response.status == "succeeded" {
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
                "Timeout waiting for deploy {} to finish (last status: {})",
                deploy_id,
                response.status
            );
        }

        sleep(POLL_INTERVAL).await;
    }
}

impl ApplyCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        let org_ident = ctx.require_org()?;
        let app_ident = ctx.require_app()?;
        let env_ident = require_env(&ctx)?;

        let manifest_path = self.manifest.unwrap_or_else(|| PathBuf::from("vt.toml"));
        let contents = std::fs::read_to_string(&manifest_path).map_err(|e| {
            anyhow::anyhow!("failed to read manifest {}: {e}", manifest_path.display())
        })?;

        let errors = crate::manifest::validate_manifest_toml_str(&contents)?;
        if !errors.is_empty() {
            print_manifest_errors(&errors);
            anyhow::bail!("Manifest validation failed ({} error(s))", errors.len());
        }

        let manifest_hash = crate::manifest::manifest_hash_from_toml_str(&contents)?;
        let manifest_json = crate::manifest::manifest_json_from_toml_str(&contents)?;

        let image_ref = image_ref_from_manifest(&manifest_json)?;
        let image_digest = match self.image_digest.as_deref() {
            Some(d) => normalize_image_digest(d)?,
            None => digest_from_image_ref(&image_ref)?,
        };

        let manifest_process_types = process_types_from_manifest(&manifest_json)?;
        let process_types = select_process_types(&manifest_process_types, &self.process_type)?;
        let primary_process = process_types
            .first()
            .ok_or_else(|| anyhow::anyhow!("manifest must include at least one process type"))?;
        let command = command_from_manifest(&manifest_json, primary_process)?;

        if self.dry_run {
            let plan = ApplyPlan {
                dry_run: self.dry_run,
                org_id: org_ident.to_string(),
                app_id: app_ident.to_string(),
                env_id: env_ident.to_string(),
                manifest_path: manifest_path.display().to_string(),
                manifest_hash: manifest_hash.clone(),
                image_ref: image_ref.clone(),
                image_digest: image_digest.clone(),
                process_types: process_types.clone(),
                command: command.clone(),
                strategy: "rolling".to_string(),
            };

            match ctx.format {
                OutputFormat::Json => print_single(&plan, ctx.format),
                OutputFormat::Table => {
                    let process_list = process_types.join(",");
                    let command_list = if command.is_empty() {
                        "(none)".to_string()
                    } else {
                        command.join(" ")
                    };
                    print_info("Preview (dry-run):");
                    println!("- org: {}", org_ident);
                    println!("- app: {}", app_ident);
                    println!("- env: {}", env_ident);
                    println!("- manifest: {}", manifest_path.display());
                    println!("- manifest_hash: {}", manifest_hash);
                    println!("- image_ref: {}", image_ref);
                    println!("- image_digest: {}", image_digest);
                    println!("- process_types: {}", process_list);
                    println!("- command: {}", command_list);
                    println!("- actions:");
                    println!("  - create release (schema=v1)");
                    println!("  - create deploy (strategy=rolling)");
                }
            }
            return Ok(());
        }

        let client = ctx.client()?;

        let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
        let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
        let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

        super::secrets::ensure_secrets_configured(&client, org_id, app_id, env_id).await?;

        // 1) Create release from (image digest + manifest hash).
        let release_path = format!("/v1/orgs/{}/apps/{}/releases", org_id, app_id);
        let release_req = CreateReleaseRequest {
            image_ref: image_ref.clone(),
            image_digest: image_digest.clone(),
            manifest_schema_version: 1,
            manifest_hash: manifest_hash.clone(),
            command: command.clone(),
        };
        let release_idem = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key(
                "releases.create",
                &release_path,
                &release_req,
            )?,
        };

        let release: ReleaseResponse = client
            .post_with_idempotency_key(&release_path, &release_req, Some(release_idem.as_str()))
            .await?;

        // 2) Create deploy for selected process types.
        let deploy_path = format!(
            "/v1/orgs/{}/apps/{}/envs/{}/deploys",
            org_id, app_id, env_id
        );
        let deploy_req = CreateDeployRequest {
            release_id: release.id.clone(),
            process_types: Some(process_types.clone()),
            strategy: "rolling".to_string(),
        };
        let deploy_idem = match ctx.idempotency_key.as_deref() {
            Some(key) => key.to_string(),
            None => crate::idempotency::default_idempotency_key(
                "deploys.create",
                &deploy_path,
                &deploy_req,
            )?,
        };

        let deploy: DeployResponse = client
            .post_with_idempotency_key(&deploy_path, &deploy_req, Some(deploy_idem.as_str()))
            .await?;

        let release_id = release.id.clone();
        let deploy_id = deploy.id.clone();

        let receipt_payload = ApplyReceipt {
            release_id: release_id.clone(),
            deploy_id: deploy_id.clone(),
            manifest_hash: manifest_hash.clone(),
            image_ref: image_ref.clone(),
            image_digest: image_digest.clone(),
            process_types: process_types.clone(),
        };

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
                label: "Next",
                cmd: format!(
                    "vt --org {} --app {} --env {} logs --follow",
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

        let ids = serde_json::json!({
            "org_id": org_id_str.clone(),
            "app_id": app_id_str.clone(),
            "env_id": env_id_str.clone(),
            "release_id": release_id,
            "deploy_id": deploy_id.clone()
        });

        // Parse wait timeout if provided
        let wait_timeout = match self.wait_timeout.as_deref() {
            Some(t) => parse_duration(t)?,
            None => DEFAULT_WAIT_TIMEOUT,
        };

        if self.wait {
            let final_deploy = wait_for_deploy(
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
                        "Deploy {} finished with status {}",
                        deploy_id.as_str(),
                        final_deploy.status.as_str()
                    ),
                    status: final_deploy.status.as_str(),
                    kind: "deploys.apply",
                    resource_key: "apply",
                    resource: &receipt_payload,
                    ids,
                    next: &next,
                },
            );

            return Ok(());
        }

        print_receipt(
            ctx.format,
            Receipt {
                message: format!(
                    "Applied manifest (release {}, deploy {})",
                    receipt_payload.release_id.as_str(),
                    deploy_id.as_str()
                ),
                status: "accepted",
                kind: "deploys.apply",
                resource_key: "apply",
                resource: &receipt_payload,
                ids,
                next: &next,
            },
        );

        Ok(())
    }
}

fn require_env(ctx: &CommandContext) -> Result<&str> {
    ctx.resolve_env().ok_or_else(|| {
        anyhow::anyhow!("No environment specified. Use --env or set a default context.")
    })
}

fn print_manifest_errors(errors: &[ManifestValidationError]) {
    for err in errors {
        println!(
            "invalid at {} (schema {})",
            err.instance_path, err.schema_path
        );
    }
}

fn image_ref_from_manifest(manifest_json: &serde_json::Value) -> Result<String> {
    let Some(image) = manifest_json.get("image") else {
        anyhow::bail!("manifest missing [image] section");
    };
    let Some(image_ref) = image.get("ref").and_then(|v| v.as_str()) else {
        anyhow::bail!("manifest missing image.ref");
    };
    let image_ref = image_ref.trim();
    if image_ref.is_empty() {
        anyhow::bail!("manifest image.ref cannot be empty");
    }
    Ok(image_ref.to_string())
}

fn process_types_from_manifest(manifest_json: &serde_json::Value) -> Result<Vec<String>> {
    let Some(processes) = manifest_json.get("processes").and_then(|v| v.as_object()) else {
        anyhow::bail!("manifest missing [processes] section (at least one process type required)");
    };

    let mut out: Vec<String> = processes.keys().cloned().collect();
    out.sort();
    if out.is_empty() {
        anyhow::bail!("manifest [processes] must include at least one process type");
    }
    Ok(out)
}

fn default_command() -> Vec<String> {
    vec!["./start".to_string()]
}

fn command_from_manifest(
    manifest_json: &serde_json::Value,
    process_type: &str,
) -> Result<Vec<String>> {
    let Some(processes) = manifest_json.get("processes").and_then(|v| v.as_object()) else {
        anyhow::bail!("manifest missing [processes] section (at least one process type required)");
    };
    let Some(process) = processes.get(process_type) else {
        anyhow::bail!("manifest missing process type '{process_type}'");
    };
    let command = process
        .get("command")
        .and_then(|value| value.as_array())
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

fn select_process_types(
    manifest_process_types: &[String],
    selected: &[String],
) -> Result<Vec<String>> {
    if selected.is_empty() {
        return Ok(manifest_process_types.to_vec());
    }

    let allowed: BTreeSet<&str> = manifest_process_types.iter().map(|s| s.as_str()).collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for p in selected {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if !allowed.contains(p) {
            anyhow::bail!("process type '{p}' not found in manifest");
        }
        out.insert(p.to_string());
    }

    if out.is_empty() {
        anyhow::bail!("at least one --process-type must be provided");
    }

    Ok(out.into_iter().collect())
}

fn normalize_image_digest(digest: &str) -> Result<String> {
    let d = digest.trim();
    if d.is_empty() {
        anyhow::bail!("--image-digest cannot be empty");
    }
    if !d.starts_with("sha256:") {
        anyhow::bail!("--image-digest must start with 'sha256:'");
    }
    Ok(d.to_string())
}

fn digest_from_image_ref(image_ref: &str) -> Result<String> {
    let Some((_, digest)) = image_ref.split_once('@') else {
        anyhow::bail!(
            "image.ref must be a digest reference (contain '@sha256:...') or provide --image-digest"
        );
    };
    normalize_image_digest(digest)
}
