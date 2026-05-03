//! rupu-cli — the `rupu` binary.
//!
//! `pub async fn run(args)` is the testable entry point: it parses
//! the command line via clap, dispatches to a subcommand handler in
//! [`cmd`], and returns an `ExitCode`. The binary's `main.rs` is a
//! one-line wrapper that calls into here.

pub mod cmd;
pub mod crash;
pub mod logging;
pub mod paths;
pub mod provider_factory;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "rupu", version, about = "Agentic code-development CLI", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// One-shot agent run.
    Run(cmd::run::Args),
    /// Manage agents.
    Agent {
        #[command(subcommand)]
        action: cmd::agent::Action,
    },
    /// Manage workflows.
    Workflow {
        #[command(subcommand)]
        action: cmd::workflow::Action,
    },
    /// Browse transcripts.
    Transcript {
        #[command(subcommand)]
        action: cmd::transcript::Action,
    },
    /// Get / set configuration values.
    Config {
        #[command(subcommand)]
        action: cmd::config::Action,
    },
    /// Manage provider credentials.
    Auth {
        #[command(subcommand)]
        action: cmd::auth::Action,
    },
    /// List or refresh available models.
    Models {
        #[command(subcommand)]
        action: cmd::models::Action,
    },
}

/// Testable entrypoint. Parses `args` (typically from `std::env::args`),
/// dispatches, and returns an `ExitCode`. Tests pass synthetic argv.
pub async fn run(args: Vec<String>) -> ExitCode {
    logging::init();
    crash::install_panic_hook();

    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            // clap handles --help / --version with its own non-zero codes;
            // surface them faithfully.
            e.exit();
        }
    };
    match cli.command {
        Cmd::Run(args) => cmd::run::handle(args).await,
        Cmd::Agent { action } => cmd::agent::handle(action).await,
        Cmd::Workflow { action } => cmd::workflow::handle(action).await,
        Cmd::Transcript { action } => cmd::transcript::handle(action).await,
        Cmd::Config { action } => cmd::config::handle(action).await,
        Cmd::Auth { action } => cmd::auth::handle(action).await,
        Cmd::Models { action } => cmd::models::handle(action).await,
    }
}
