//! Apply command (manifest-first workflow).
//!
//! v1: `vt apply` creates a release from the local manifest + image digest,
//! then creates a deploy for the selected environment.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::manifest::ManifestValidationError;
use crate::output::{print_info, print_single, print_success, OutputFormat};

use super::CommandContext;

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
}

#[derive(Debug, Serialize)]
struct CreateReleaseRequest {
    image_ref: String,
    image_digest: String,
    manifest_schema_version: i32,
    manifest_hash: String,
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

#[derive(Debug, Deserialize)]
struct DeployResponse {
    id: String,
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
            };

            match ctx.format {
                OutputFormat::Json => print_single(&plan, ctx.format),
                OutputFormat::Table => {
                    print_info("Plan (dry-run):");
                    println!("- validate manifest: ok");
                    println!(
                        "- create release: image_ref={}, manifest_hash={}",
                        image_ref, manifest_hash
                    );
                    println!(
                        "- create deploy: env_id={}, process_types={}",
                        env_ident,
                        process_types.join(",")
                    );
                }
            }
            return Ok(());
        }

        let client = ctx.client()?;

        let org_id = crate::resolve::resolve_org_id(&client, org_ident).await?;
        let app_id = crate::resolve::resolve_app_id(&client, org_id, app_ident).await?;
        let env_id = crate::resolve::resolve_env_id(&client, org_id, app_id, env_ident).await?;

        // 1) Create release from (image digest + manifest hash).
        let release_path = format!("/v1/orgs/{}/apps/{}/releases", org_id, app_id);
        let release_req = CreateReleaseRequest {
            image_ref: image_ref.clone(),
            image_digest: image_digest.clone(),
            manifest_schema_version: 1,
            manifest_hash: manifest_hash.clone(),
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

        match ctx.format {
            OutputFormat::Json => {
                let out = serde_json::json!({
                    "manifest_hash": manifest_hash,
                    "image_ref": image_ref,
                    "image_digest": image_digest,
                    "process_types": process_types,
                    "release_id": release.id,
                    "deploy_id": deploy.id,
                });
                print_single(&out, ctx.format);
            }
            OutputFormat::Table => {
                print_success(&format!(
                    "Applied manifest (release {}, deploy {})",
                    release.id, deploy.id
                ));
                print_info(&format!(
                    "Next: vt deploys get {}  # (or vt deploys list)",
                    deploy.id
                ));
                print_info("Next: vt instances list");
                print_info("Next: vt logs stream");
                print_info("Debug: vt events tail");
            }
        }

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
