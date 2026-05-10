//! rupu-cli — the `rupu` binary.
//!
//! `pub async fn run(args)` is the testable entry point: it parses
//! the command line via clap, dispatches to a subcommand handler in
//! [`cmd`], and returns an `ExitCode`. The binary's `main.rs` is a
//! one-line wrapper that calls into here.

pub mod cmd;
pub mod crash;
pub mod logging;
pub mod output;
pub mod paths;
pub mod pricing;
pub mod provider_factory;
pub mod run_target;
pub mod templates;

#[cfg(test)]
pub(crate) mod test_support {
    use tokio::sync::Mutex;

    pub static ENV_LOCK: Mutex<()> = Mutex::const_new(());
}

use clap::{CommandFactory, Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "rupu", version, about = "Agentic code-development CLI", long_about = None)]
pub struct Cli {
    /// Structured output format for commands that support tabular/report views.
    #[arg(long, global = true)]
    pub format: Option<output::formats::OutputFormat>,
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
    /// Run autonomous workflows against persistent issue state.
    Autoflow {
        #[command(subcommand)]
        action: cmd::autoflow::Action,
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
    /// SCM repository operations.
    Repos {
        #[command(subcommand)]
        action: cmd::repos::Action,
    },
    /// Issue-tracker operations (list / show / run a workflow).
    Issues {
        #[command(subcommand)]
        action: cmd::issues::Action,
    },
    /// Bootstrap a new rupu project (`.rupu/agents`, `.rupu/workflows`, config).
    Init(cmd::init::InitArgs),
    /// MCP server operations.
    Mcp {
        #[command(subcommand)]
        action: cmd::mcp::Action,
    },
    /// Schedule-driven workflow firing (designed for system cron).
    Cron {
        #[command(subcommand)]
        action: cmd::cron::Action,
    },
    /// Webhook receiver for event-triggered workflows (GitHub / GitLab).
    Webhook {
        #[command(subcommand)]
        action: cmd::webhook::Action,
    },
    /// Aggregate token spend across persisted transcripts.
    Usage(cmd::usage::UsageArgs),
    /// Re-attach TUI to an existing run.
    Watch(cmd::watch::WatchArgs),
    /// Generate or install shell-completion scripts.
    Completions {
        #[command(subcommand)]
        action: cmd::completions::Action,
    },
}

/// Testable entrypoint. Parses `args` (typically from `std::env::args`),
/// dispatches, and returns an `ExitCode`. Tests pass synthetic argv.
pub async fn run(args: Vec<String>) -> ExitCode {
    // Dynamic shell-completion entrypoint. When the `COMPLETE` env var
    // is set (the protocol used by the bootstrap script that
    // `rupu completions` installs), `complete()` prints the candidate
    // list or registration script and exits before any normal CLI
    // processing. No-op when the env var is unset.
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    crash::install_panic_hook();

    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            // clap handles --help / --version with its own non-zero codes;
            // surface them faithfully.
            e.exit();
        }
    };

    if let Some(format) = cli.format {
        match &cli.command {
            Cmd::Usage(_) => {}
            _ => {
                eprintln!(
                    "`rupu {}` does not support structured `--format {format}` output yet",
                    command_name(&cli.command)
                );
                return ExitCode::from(2);
            }
        }
    }

    // Run / Workflow Run / Watch write to stdout (either line-stream or
    // alt-screen TUI canvas). Either way, tracing on stderr would bleed
    // through and corrupt the output. Route logs to the rupu log file
    // for all three commands; everything else keeps stderr.
    let is_output_cmd = matches!(
        cli.command,
        Cmd::Run(_)
            | Cmd::Watch(_)
            | Cmd::Workflow {
                action: cmd::workflow::Action::Run { .. }
            }
            | Cmd::Autoflow {
                action: cmd::autoflow::Action::Run { .. }
            }
    );
    if is_output_cmd {
        logging::init_to_file();
    } else {
        logging::init();
    }

    match cli.command {
        Cmd::Run(args) => cmd::run::handle(args).await,
        Cmd::Agent { action } => cmd::agent::handle(action).await,
        Cmd::Workflow { action } => cmd::workflow::handle(action).await,
        Cmd::Autoflow { action } => cmd::autoflow::handle(action).await,
        Cmd::Transcript { action } => cmd::transcript::handle(action).await,
        Cmd::Config { action } => cmd::config::handle(action).await,
        Cmd::Auth { action } => cmd::auth::handle(action).await,
        Cmd::Models { action } => cmd::models::handle(action).await,
        Cmd::Repos { action } => cmd::repos::handle(action).await,
        Cmd::Issues { action } => cmd::issues::handle(action).await,
        Cmd::Init(args) => cmd::init::handle(args).await,
        Cmd::Mcp { action } => cmd::mcp::handle(action).await,
        Cmd::Cron { action } => cmd::cron::handle(action).await,
        Cmd::Webhook { action } => cmd::webhook::handle(action).await,
        Cmd::Usage(args) => cmd::usage::handle(args, cli.format).await,
        Cmd::Watch(args) => cmd::watch::handle(args).await,
        Cmd::Completions { action } => cmd::completions::handle(action).await,
    }
}

fn command_name(command: &Cmd) -> &'static str {
    match command {
        Cmd::Run(_) => "run",
        Cmd::Agent { .. } => "agent",
        Cmd::Workflow { .. } => "workflow",
        Cmd::Autoflow { .. } => "autoflow",
        Cmd::Transcript { .. } => "transcript",
        Cmd::Config { .. } => "config",
        Cmd::Auth { .. } => "auth",
        Cmd::Models { .. } => "models",
        Cmd::Repos { .. } => "repos",
        Cmd::Issues { .. } => "issues",
        Cmd::Init(_) => "init",
        Cmd::Mcp { .. } => "mcp",
        Cmd::Cron { .. } => "cron",
        Cmd::Webhook { .. } => "webhook",
        Cmd::Usage(_) => "usage",
        Cmd::Watch(_) => "watch",
        Cmd::Completions { .. } => "completions",
    }
}
