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
pub mod run_target;
pub mod standalone_run_metadata;
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
    /// Persistent agent sessions.
    Session {
        #[command(subcommand)]
        action: cmd::session::Action,
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
    Usage(Box<cmd::usage::UsageArgs>),
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

    if let Some(format) = cli
        .format
        .filter(|format| *format != output::formats::OutputFormat::Table)
    {
        if let Err(error) = ensure_output_format_supported(&cli.command, format) {
            eprintln!("{error}");
            return ExitCode::from(2);
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
            | Cmd::Session {
                action: cmd::session::Action::Start(_)
                    | cmd::session::Action::Send(_)
                    | cmd::session::Action::Attach { .. }
                    | cmd::session::Action::RunTurn(_)
            }
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
        Cmd::Agent { action } => cmd::agent::handle(action, cli.format).await,
        Cmd::Workflow { action } => cmd::workflow::handle(action, cli.format).await,
        Cmd::Autoflow { action } => cmd::autoflow::handle(action, cli.format).await,
        Cmd::Transcript { action } => cmd::transcript::handle(action, cli.format).await,
        Cmd::Config { action } => cmd::config::handle(action).await,
        Cmd::Auth { action } => cmd::auth::handle(action, cli.format).await,
        Cmd::Models { action } => cmd::models::handle(action, cli.format).await,
        Cmd::Repos { action } => cmd::repos::handle(action, cli.format).await,
        Cmd::Session { action } => cmd::session::handle(action, cli.format).await,
        Cmd::Issues { action } => cmd::issues::handle(action, cli.format).await,
        Cmd::Init(args) => cmd::init::handle(args).await,
        Cmd::Mcp { action } => cmd::mcp::handle(action).await,
        Cmd::Cron { action } => cmd::cron::handle(action, cli.format).await,
        Cmd::Webhook { action } => cmd::webhook::handle(action).await,
        Cmd::Usage(args) => cmd::usage::handle(*args, cli.format).await,
        Cmd::Watch(args) => cmd::watch::handle(args).await,
        Cmd::Completions { action } => cmd::completions::handle(action).await,
    }
}

fn ensure_output_format_supported(
    command: &Cmd,
    format: output::formats::OutputFormat,
) -> anyhow::Result<()> {
    match command {
        Cmd::Run(_) => output::formats::ensure_supported(
            "run",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Agent { action } => cmd::agent::ensure_output_format(action, format),
        Cmd::Workflow { action } => cmd::workflow::ensure_output_format(action, format),
        Cmd::Autoflow { action } => cmd::autoflow::ensure_output_format(action, format),
        Cmd::Transcript { action } => cmd::transcript::ensure_output_format(action, format),
        Cmd::Config { .. } => output::formats::ensure_supported(
            "config",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Auth { action } => cmd::auth::ensure_output_format(action, format),
        Cmd::Models { action } => cmd::models::ensure_output_format(action, format),
        Cmd::Repos { action } => cmd::repos::ensure_output_format(action, format),
        Cmd::Session { action } => cmd::session::ensure_output_format(action, format),
        Cmd::Issues { action } => cmd::issues::ensure_output_format(action, format),
        Cmd::Init(_) => output::formats::ensure_supported(
            "init",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Mcp { .. } => output::formats::ensure_supported(
            "mcp",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Cron { action } => cmd::cron::ensure_output_format(action, format),
        Cmd::Webhook { .. } => output::formats::ensure_supported(
            "webhook",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Usage(args) => cmd::usage::ensure_output_format(args, format),
        Cmd::Watch(_) => output::formats::ensure_supported(
            "watch",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Completions { .. } => output::formats::ensure_supported(
            "completions",
            format,
            &[output::formats::OutputFormat::Table],
        ),
    }
}
