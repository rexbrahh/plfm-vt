//! ghostctl (vt) - CLI for plfm-vt platform
//!
//! The primary interface to the platform for developers and CI systems.
//! See: docs/cli/

use anyhow::Result;
use clap::Parser;

mod client;
mod commands;
mod config;
mod error;
mod idempotency;
mod manifest;
mod output;
mod resolve;

use commands::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Run the command
    if let Err(e) = cli.run().await {
        // Print error in a user-friendly way
        error::print_error(&e);
        std::process::exit(1);
    }

    Ok(())
}
