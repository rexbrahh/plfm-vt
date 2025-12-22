//! Manifest commands.
//!
//! These commands operate purely on local manifest files (offline).

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::output::{print_info, print_single, print_success, OutputFormat};

use super::CommandContext;

/// Manifest commands.
#[derive(Debug, Args)]
pub struct ManifestCommand {
    #[command(subcommand)]
    command: ManifestSubcommand,
}

#[derive(Debug, Subcommand)]
enum ManifestSubcommand {
    /// Validate a manifest file against the v1 schema (offline).
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
struct ValidateArgs {
    /// Manifest file path (TOML). Defaults to ./vt.toml.
    #[arg(long, value_name = "PATH")]
    manifest: Option<PathBuf>,
}

impl ManifestCommand {
    pub async fn run(self, ctx: CommandContext) -> Result<()> {
        match self.command {
            ManifestSubcommand::Validate(args) => validate_manifest(ctx, args),
        }
    }
}

fn validate_manifest(ctx: CommandContext, args: ValidateArgs) -> Result<()> {
    let path = args.manifest.unwrap_or_else(|| PathBuf::from("vt.toml"));
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read manifest {}: {e}", path.display()))?;

    let errors = crate::manifest::validate_manifest_toml_str(&contents)?;
    if !errors.is_empty() {
        let count = errors.len();
        for err in &errors {
            println!(
                "invalid at {} (schema {})",
                err.instance_path, err.schema_path
            );
        }
        anyhow::bail!("Manifest validation failed ({} error(s))", count);
    }

    let hash = crate::manifest::manifest_hash_from_toml_str(&contents)?;

    match ctx.format {
        OutputFormat::Json => {
            let out = serde_json::json!({
                "valid": true,
                "manifest_hash": hash,
            });
            print_single(&out, OutputFormat::Json);
        }
        OutputFormat::Table => {
            print_success(&format!("Manifest is valid: {}", path.display()));
            print_info(&format!("manifest_hash: {}", hash));
        }
    }

    Ok(())
}
