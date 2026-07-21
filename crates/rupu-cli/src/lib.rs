//! rupu-cli — the `rupu` binary.
//!
//! `pub async fn run(args)` is the testable entry point: it parses
//! the command line via clap, dispatches to a subcommand handler in
//! [`cmd`], and returns an `ExitCode`. The binary's `main.rs` is a
//! one-line wrapper that calls into here.

pub mod build_info;
pub mod cmd;
pub mod cp_agent_launcher;
pub mod cp_definition_generator;
pub mod cp_launcher;
pub mod cp_repos;
pub mod cp_session_mutator;
pub mod cp_session_sender;
pub mod cp_session_starter;
pub mod crash;
pub mod fleet_unit_dispatcher;
pub mod logging;
pub mod output;
pub mod paths;
pub mod resume;
pub mod run_target;
pub mod standalone_run_metadata;
pub mod templates;
pub mod update_notice;

#[cfg(test)]
pub(crate) mod test_support {
    use tokio::sync::Mutex;

    pub static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    /// Installs the process-level rustls `CryptoProvider` once per test
    /// binary. `main.rs` does this at real-binary startup (the dep tree
    /// enables both the `aws-lc-rs` and `ring` rustls providers, so rustls
    /// 0.23 can't auto-select one), but `cargo test --lib` never runs
    /// `main()`. Any test that builds a real `RepoConnector` (github/gitlab/
    /// jira/linear reqwest clients) needs a provider installed before its
    /// first TLS-capable request or it panics. Idempotent and safe to call
    /// from every test, even ones that don't touch a connector.
    pub fn ensure_crypto_provider() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        });
    }
}

use clap::{CommandFactory, Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "rupu",
    version = build_info::version_line_static(),
    about = "Agentic code-development CLI",
    long_about = None
)]
pub struct Cli {
    /// Structured output format for commands that support tabular/report views.
    #[arg(long, global = true)]
    pub format: Option<output::formats::OutputFormat>,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// One-shot agent run, or pause/resume a run by id.
    // Not a clap `#[command(subcommand)]`: nesting `pause`/`resume` as
    // named subcommand variants alongside the agent-run launcher broke
    // flag-first invocations (`rupu run --tmp <ref>`). See `cmd::run`'s
    // module doc for the full root-cause writeup. Instead this captures
    // its trailing tokens verbatim and hands them to `cmd::run::classify`
    // for the actual pause/resume-vs-launch dispatch.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        argv: Vec<String>,
    },
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
    /// Manage UI themes and palette imports.
    Ui {
        #[command(subcommand)]
        action: cmd::ui::Action,
    },
    /// Prune archived local sessions and transcripts.
    Cleanup(cmd::cleanup::Args),
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
    /// Inspect agentic coverage ledgers and concern catalogs.
    Coverage {
        #[command(subcommand)]
        action: cmd::coverage::Action,
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
    /// Control-plane HTTP server for the rupu web UI.
    Cp {
        #[command(subcommand)]
        action: cmd::cp::Action,
    },
    /// Aggregate token spend across persisted transcripts.
    Usage(Box<cmd::usage::UsageArgs>),
    /// Re-attach to an existing run.
    Watch(cmd::watch::WatchArgs),
    /// Generate or install shell-completion scripts.
    Completions {
        #[command(subcommand)]
        action: cmd::completions::Action,
    },
    /// Manage named rupu-cp hosts (add / list / remove).
    Host {
        #[command(subcommand)]
        action: cmd::host::Action,
    },
    /// Dial-home tunnel agent + node enrollment.
    Node(cmd::node::NodeArgs),
    /// Download and install the latest release for the configured channel.
    Update(cmd::update::UpdateArgs),
    /// Internal: privileged install step invoked by `rupu update` via
    /// `sudo` when the install directory isn't user-writable.
    #[command(name = "__apply-update", hide = true)]
    ApplyUpdate(cmd::apply_update::ApplyUpdateArgs),
    /// Internal: remote workspace stage/collect helper (SSH workspace sync).
    #[command(name = "__workspace", hide = true)]
    Workspace {
        #[command(subcommand)]
        action: cmd::workspace_helper::WorkspaceHelperAction,
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

    // Run / Workflow Run / Watch / Session Attach own a live stdout view.
    // Tracing on stderr would bleed through and corrupt that output.
    // Route logs to the rupu log file for those commands; everything
    // else keeps stderr.
    let is_output_cmd = matches!(
        cli.command,
        Cmd::Run { .. }
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

    // Passive "update available" notice: interactive, non-structured
    // invocations only, and never for `rupu update`/`rupu
    // __apply-update` themselves (those already report update status
    // directly).
    let is_update_cmd = matches!(cli.command, Cmd::Update(_) | Cmd::ApplyUpdate(_));
    if !is_update_cmd {
        let structured = cli
            .format
            .map(|format| format != output::formats::OutputFormat::Table)
            .unwrap_or(false);
        let is_tty = update_notice::stderr_is_tty();
        let cfg = cmd::update::load_cli_config();
        let channel = cfg
            .update
            .channel
            .clone()
            .unwrap_or_else(|| "stable".to_string());
        update_notice::maybe_print(
            cfg.update.check,
            &channel,
            build_info::RELEASE_VERSION,
            is_tty,
            structured,
        );
    }

    match cli.command {
        Cmd::Run { argv } => cmd::run::handle(argv, cli.format).await,
        Cmd::Agent { action } => cmd::agent::handle(action, cli.format).await,
        Cmd::Workflow { action } => cmd::workflow::handle(action, cli.format).await,
        Cmd::Autoflow { action } => cmd::autoflow::handle(action, cli.format).await,
        Cmd::Transcript { action } => cmd::transcript::handle(action, cli.format).await,
        Cmd::Config { action } => cmd::config::handle(action).await,
        Cmd::Ui { action } => cmd::ui::handle(action, cli.format).await,
        Cmd::Cleanup(args) => cmd::cleanup::handle(args, cli.format).await,
        Cmd::Auth { action } => cmd::auth::handle(action, cli.format).await,
        Cmd::Models { action } => cmd::models::handle(action, cli.format).await,
        Cmd::Repos { action } => cmd::repos::handle(action, cli.format).await,
        Cmd::Session { action } => cmd::session::handle(action, cli.format).await,
        Cmd::Issues { action } => cmd::issues::handle(action, cli.format).await,
        Cmd::Init(args) => cmd::init::handle(args).await,
        Cmd::Mcp { action } => cmd::mcp::handle(action).await,
        Cmd::Coverage { action } => cmd::coverage::handle(action, cli.format).await,
        Cmd::Cron { action } => cmd::cron::handle(action, cli.format).await,
        Cmd::Webhook { action } => cmd::webhook::handle(action).await,
        Cmd::Cp { action } => cmd::cp::handle(action).await,
        Cmd::Usage(args) => cmd::usage::handle(*args, cli.format).await,
        Cmd::Watch(args) => cmd::watch::handle(args).await,
        Cmd::Completions { action } => cmd::completions::handle(action).await,
        Cmd::Host { action } => cmd::host::handle(action).await,
        Cmd::Node(args) => cmd::node::handle(args).await,
        Cmd::Update(args) => cmd::update::handle(args).await,
        Cmd::ApplyUpdate(args) => cmd::apply_update::handle(args),
        Cmd::Workspace { action } => cmd::workspace_helper::handle(action).await,
    }
}

fn ensure_output_format_supported(
    command: &Cmd,
    format: output::formats::OutputFormat,
) -> anyhow::Result<()> {
    match command {
        Cmd::Run { argv } => {
            // `rupu run list` / `rupu run show` emit JSON (they are rupu-cp's
            // SSH run-listing / run-detail contracts); every other `rupu run`
            // form is Table-only.
            let allowed: &[output::formats::OutputFormat] =
                if matches!(argv.first().map(String::as_str), Some("list") | Some("show")) {
                    &[
                        output::formats::OutputFormat::Table,
                        output::formats::OutputFormat::Json,
                    ]
                } else {
                    &[output::formats::OutputFormat::Table]
                };
            output::formats::ensure_supported("run", format, allowed)
        }
        Cmd::Agent { action } => cmd::agent::ensure_output_format(action, format),
        Cmd::Workflow { action } => cmd::workflow::ensure_output_format(action, format),
        Cmd::Autoflow { action } => cmd::autoflow::ensure_output_format(action, format),
        Cmd::Transcript { action } => cmd::transcript::ensure_output_format(action, format),
        Cmd::Config { .. } => output::formats::ensure_supported(
            "config",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Ui { action } => cmd::ui::ensure_output_format(action, format),
        Cmd::Cleanup(_) => cmd::cleanup::ensure_output_format(format),
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
        Cmd::Coverage { action } => cmd::coverage::ensure_output_format(action, format),
        Cmd::Cron { action } => cmd::cron::ensure_output_format(action, format),
        Cmd::Webhook { .. } => output::formats::ensure_supported(
            "webhook",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Cp { .. } => {
            output::formats::ensure_supported("cp", format, &[output::formats::OutputFormat::Table])
        }
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
        Cmd::Host { .. } => output::formats::ensure_supported(
            "host",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Node(_) => output::formats::ensure_supported(
            "node",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Update(_) => output::formats::ensure_supported(
            "update",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::ApplyUpdate(_) => output::formats::ensure_supported(
            "__apply-update",
            format,
            &[output::formats::OutputFormat::Table],
        ),
        Cmd::Workspace { .. } => output::formats::ensure_supported(
            "__workspace",
            format,
            &[output::formats::OutputFormat::Table],
        ),
    }
}

/// Arg-parse tests (Task 7, fixed): `rupu run pause|resume <id>` and
/// `rupu workflow pause|resume <id>` parse to the expected typed
/// variants. These are pure clap-derive + `cmd::run::classify` checks —
/// no I/O, no run-store — so they stay fast and independent of the
/// heavier end-to-end tests in `tests/cli_run.rs` / `tests/cli_workflow.rs`.
///
/// The `run_*` tests here cover the T7 regression directly: `Cmd::Run`
/// captures raw argv (not a clap subcommand — see `cmd::run`'s module
/// doc), so both the pause/resume routing AND flag-first launcher
/// parsing are exercised as two stages (`Cli::try_parse_from` captures
/// raw tokens, then `cmd::run::classify` + `parse_launch_args` do the
/// real interpretation).
#[cfg(test)]
mod arg_parse_tests {
    use super::*;

    /// Parse `rupu run <argv...>` down to the raw `Cmd::Run { argv }`
    /// tokens, mirroring exactly what `cmd::run::handle` receives.
    fn parse_run_argv(tail: &[&str]) -> Vec<String> {
        let mut full = vec!["rupu", "run"];
        full.extend_from_slice(tail);
        let cli = Cli::try_parse_from(full).unwrap();
        match cli.command {
            Cmd::Run { argv } => argv,
            other => panic!("expected Cmd::Run, got {other:?}"),
        }
    }

    #[test]
    fn run_pause_parses_run_id() {
        let argv = parse_run_argv(&["pause", "run_01ABC"]);
        match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Pause { run_id } => assert_eq!(run_id, "run_01ABC"),
            other => panic!("expected RunAction::Pause, got {other:?}"),
        }
    }

    #[test]
    fn run_resume_parses_run_id_and_flags() {
        let argv = parse_run_argv(&["resume", "run_01ABC", "--mode", "bypass", "--plain"]);
        match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Resume {
                run_id,
                mode,
                plain,
            } => {
                assert_eq!(run_id, "run_01ABC");
                assert_eq!(mode.as_deref(), Some("bypass"));
                assert!(plain);
            }
            other => panic!("expected RunAction::Resume, got {other:?}"),
        }
    }

    #[test]
    fn run_launch_still_captures_agent_invocation() {
        // Back-compat: `rupu run <agent> ...` (agent name != "pause"/"resume")
        // must still fall through to the launcher, unchanged from before
        // Task 7 introduced the `pause`/`resume` control actions.
        let argv = parse_run_argv(&["echo", "--mode", "bypass", "say hi"]);
        match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Launch(argv) => {
                assert_eq!(argv, vec!["echo", "--mode", "bypass", "say hi"]);
            }
            other => panic!("expected RunAction::Launch, got {other:?}"),
        }
    }

    /// THE T7 regression this task fixes: flag-first launcher
    /// invocations must parse (not hard-fail with "unexpected
    /// argument"), exactly as they did before Task 7 restructured
    /// `Cmd::Run` into a clap subcommand enum. `--tmp`/`--mode`/`--view`/
    /// `--no-stream` all need to land in the fully-parsed `Args`, with
    /// `agent` populated from wherever it falls in argv.
    #[test]
    fn run_flag_first_tmp_parses_into_launcher_args() {
        let argv = parse_run_argv(&["--tmp", "github:owner/repo"]);
        let args = match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Launch(argv) => cmd::run::parse_launch_args(argv)
                .unwrap_or_else(|e| panic!("flag-first `--tmp` should parse, got: {e}")),
            other => panic!("expected RunAction::Launch, got {other:?}"),
        };
        assert!(args.tmp);
        assert_eq!(args.agent, "github:owner/repo");
    }

    #[test]
    fn run_flag_first_mode_parses_into_launcher_args() {
        let argv = parse_run_argv(&["--mode", "bypass", "echo"]);
        let args = match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Launch(argv) => cmd::run::parse_launch_args(argv)
                .unwrap_or_else(|e| panic!("flag-first `--mode` should parse, got: {e}")),
            other => panic!("expected RunAction::Launch, got {other:?}"),
        };
        assert_eq!(args.mode.as_deref(), Some("bypass"));
        assert_eq!(args.agent, "echo");
    }

    #[test]
    fn run_flag_first_view_parses_into_launcher_args() {
        let argv = parse_run_argv(&["--view", "full", "echo"]);
        let args = match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Launch(argv) => cmd::run::parse_launch_args(argv)
                .unwrap_or_else(|e| panic!("flag-first `--view` should parse, got: {e}")),
            other => panic!("expected RunAction::Launch, got {other:?}"),
        };
        assert_eq!(args.view, Some(cmd::ui::LiveViewMode::Full));
        assert_eq!(args.agent, "echo");
    }

    #[test]
    fn run_flag_first_no_stream_parses_into_launcher_args() {
        let argv = parse_run_argv(&["--no-stream", "echo", "hi there"]);
        let args = match cmd::run::classify(argv).unwrap() {
            cmd::run::RunAction::Launch(argv) => cmd::run::parse_launch_args(argv)
                .unwrap_or_else(|e| panic!("flag-first `--no-stream` should parse, got: {e}")),
            other => panic!("expected RunAction::Launch, got {other:?}"),
        };
        assert!(args.no_stream);
        assert_eq!(args.agent, "echo");
        assert_eq!(args.target.as_deref(), Some("hi there"));
    }

    #[test]
    fn workflow_pause_parses_run_id() {
        let cli = Cli::try_parse_from(["rupu", "workflow", "pause", "run_01ABC"]).unwrap();
        match cli.command {
            Cmd::Workflow {
                action: cmd::workflow::Action::Pause { run_id },
            } => assert_eq!(run_id, "run_01ABC"),
            other => panic!("expected Workflow(Pause), got {other:?}"),
        }
    }

    #[test]
    fn workflow_resume_parses_run_id_and_flags() {
        let cli = Cli::try_parse_from([
            "rupu",
            "workflow",
            "resume",
            "run_01ABC",
            "--mode",
            "ask",
            "--plain",
        ])
        .unwrap();
        match cli.command {
            Cmd::Workflow {
                action:
                    cmd::workflow::Action::Resume {
                        run_id,
                        mode,
                        plain,
                    },
            } => {
                assert_eq!(run_id, "run_01ABC");
                assert_eq!(mode.as_deref(), Some("ask"));
                assert!(plain);
            }
            other => panic!("expected Workflow(Resume), got {other:?}"),
        }
    }
}
