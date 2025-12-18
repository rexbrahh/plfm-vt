//! Error handling and display for the CLI.

use colored::Colorize;
use thiserror::Error;

/// CLI-specific errors.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("Not authenticated. Run `vt auth login` to authenticate.")]
    NotAuthenticated,

    #[error("API error: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CliError {
    /// Create an API error from response details.
    pub fn api(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Api {
            status,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Print an error in a user-friendly format.
pub fn print_error(err: &anyhow::Error) {
    eprintln!("{} {}", "Error:".red().bold(), err);
    
    // Check for specific error types and provide hints
    if let Some(cli_err) = err.downcast_ref::<CliError>() {
        match cli_err {
            CliError::NotAuthenticated => {
                eprintln!("\n{}", "Hint: Run `vt auth login` to authenticate.".yellow());
            }
            CliError::Api { status, .. } if *status == 401 => {
                eprintln!("\n{}", "Hint: Your session may have expired. Run `vt auth login`.".yellow());
            }
            CliError::Api { status, .. } if *status == 403 => {
                eprintln!("\n{}", "Hint: You may not have permission for this operation.".yellow());
            }
            CliError::Network(_) => {
                eprintln!("\n{}", "Hint: Check your network connection and API endpoint.".yellow());
            }
            _ => {}
        }
    }
}

/// Result type for CLI operations.
pub type CliResult<T> = Result<T, CliError>;
