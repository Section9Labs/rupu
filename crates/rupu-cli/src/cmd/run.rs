//! `rupu run <agent> [prompt]` — one-shot agent run.
//!
//! Wires together: paths → agent loader → config layering → permission
//! resolution → workspace upsert → auth backend → provider factory →
//! `rupu_agent::run_agent`. Prints a one-line summary on success.
//!
use crate::cmd::ui::{LiveViewMode, UiPrefs};
use crate::paths;
use crate::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
use clap::{Args as ClapArgs, Parser};
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_agent::{load_agent, parse_mode, resolve_mode, PermissionDecision};
use rupu_runtime::provider_factory;
use rupu_runtime::WorkerKind;
use rupu_tools::{PermissionMode, ToolContext};
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::warn;
use ulid::Ulid;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Agent name (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional target reference, e.g. `github:owner/repo#42`.
    /// See `docs/scm.md#target-syntax` for the full grammar.
    /// Distinguished from `prompt` by parsing as a RunTarget.
    pub target: Option<String>,
    /// Optional initial user message. Defaults to "go" if omitted.
    pub prompt: Option<String>,
    /// Explicit prompt flag (takes precedence over the positional `prompt`).
    /// Use this when the prompt should not be confused with a target string.
    #[arg(long = "prompt")]
    pub prompt_flag: Option<String>,
    /// Override permission mode (`ask` | `bypass` | `readonly`).
    #[arg(long)]
    pub mode: Option<String>,
    /// Don't render tokens as they arrive; show each response once it is
    /// complete. The request still streams on the wire, so long responses
    /// can't hit the request timeout.
    #[arg(long)]
    pub no_stream: bool,
    /// Control live output density (`focused` | `full`).
    #[arg(long, value_enum)]
    pub view: Option<LiveViewMode>,
    /// For `<platform>:<owner>/<repo>` targets: clone into this
    /// directory instead of the default `./<repo>/`. The directory
    /// must not already exist (refuse-by-default — pass an explicit
    /// path you own). Mutually exclusive with `--tmp`.
    #[arg(long, value_name = "PATH", conflicts_with = "tmp")]
    pub into: Option<std::path::PathBuf>,
    /// For `<platform>:<owner>/<repo>` targets: clone into a
    /// temporary directory that is auto-deleted on exit. Useful for
    /// one-shot agents that produce findings without modifying the
    /// repo. Mutually exclusive with `--into`.
    #[arg(long, conflicts_with = "into")]
    pub tmp: bool,
    /// Pre-assign the run id (so a caller can reference the run before it starts).
    #[arg(long)]
    pub run_id: Option<String>,
    /// Findings contract for this run (`full` | `summary`), overriding the
    /// agent's `findingsProfile`. A placed workflow unit's coordinator
    /// passes its step's resolved profile this way.
    #[arg(long, value_name = "PROFILE", value_parser = parse_findings_profile)]
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
    /// Engagement profile(s) for this run — the asset domain(s) findings are
    /// validated against (e.g. `network`, `binary`, or a composite like
    /// `pentest`). Repeatable or comma-separated. Empty = the `code` path.
    #[arg(
        long = "engagement-profile",
        visible_alias = "engagement-profiles",
        value_name = "ID",
        value_delimiter = ','
    )]
    pub engagement_profiles: Vec<String>,
    /// Continue an interrupted run of this agent from its transcript instead
    /// of starting fresh: the earlier conversation is rebuilt and the agent
    /// is told it was interrupted. A run that had already finished prints
    /// its recorded answer without calling the model. Run it from the same
    /// project as the original run.
    #[arg(
        long = "continue",
        value_name = "AGENT_RUN_ID",
        conflicts_with_all = ["target", "prompt", "prompt_flag", "into", "tmp"]
    )]
    pub continue_from: Option<String>,
}

fn parse_findings_profile(s: &str) -> Result<rupu_coverage::FindingProfile, String> {
    s.parse()
}

/// The standalone run's findings profile: `--findings-profile` → the agent's
/// `findingsProfile` → `full`. The flag sits in the step slot of
/// [`rupu_coverage::FindingProfile::resolve`] — it is how a placed unit's
/// step/workflow-default override reaches the host that runs the agent.
fn resolve_findings_profile(
    flag: Option<rupu_coverage::FindingProfile>,
    agent: Option<rupu_coverage::FindingProfile>,
) -> rupu_coverage::FindingProfile {
    rupu_coverage::FindingProfile::resolve(flag, None, agent)
}

/// `rupu run <agent> [target] [prompt] …` — OR `rupu run pause|resume
/// <run_id> …` — dispatched from a RAW-ARGV pre-pass, not a clap
/// `#[command(subcommand)]` enum.
///
/// ## Why not a clap subcommand (T7 regression, fixed here)
///
/// Task 7 originally modeled this as `Cmd::Run { action: RunCommand }`
/// where `RunCommand` was a `Subcommand` enum with named `Pause`/`Resume`
/// variants plus an `external_subcommand` catch-all (`Launch(Vec<String>)`)
/// for the agent-run case. That broke every FLAG-FIRST launcher
/// invocation — `rupu run --tmp github:owner/repo`, `rupu run --mode
/// bypass echo hi`, etc. — with `error: unexpected argument '--tmp'
/// found`.
///
/// Root cause: clap's derived subcommand dispatcher decides whether a
/// token is "a subcommand name" or "an option of the current command"
/// BEFORE the `external_subcommand` catch-all ever runs, and it makes
/// that call by looking at the token's syntax, not its position. A
/// leading `--flag` is always tried against the parent's own arg set
/// first; since the `Run` variant that held `action: RunCommand` had no
/// args of its own, `--tmp` had nothing to match and was rejected
/// outright. This reproduces identically even with `RunCommand` parsed
/// as its own top-level `Parser` (verified directly) — it isn't an
/// artifact of nesting under `Cmd::Run`, it's how clap resolves
/// subcommand-vs-option for any hyphen-leading token, full stop. No
/// arrangement of `Subcommand`/`external_subcommand` fixes this while
/// `pause`/`resume` remain real subcommand variants sharing the dispatch
/// point with the launcher.
///
/// ## The fix
///
/// `Cmd::Run` (see `lib.rs`) now captures its trailing tokens as a raw,
/// unparsed `Vec<String>` (`trailing_var_arg = true, allow_hyphen_values
/// = true` — verified this accepts a leading `--flag` fine, since it
/// isn't matched against any of the parent's own args). [`classify`]
/// then inspects those raw tokens *before* any clap re-parse: if the
/// first token is exactly `pause` or `resume`, it's the corresponding
/// control action; otherwise it's the launcher, re-parsed into [`Args`]
/// by [`parse_launch_args`] with byte-for-byte pre-Task-7 semantics
/// (agent-first AND flag-first).
///
/// Reserved-name caveat (mirrors `cargo`'s reserved subcommand names):
/// an agent literally named `pause`, `resume`, or `list` cannot be launched
/// via `rupu run pause`/`rupu run resume`/`rupu run list` — those tokens
/// always resolve to the control actions below (`list` added alongside
/// [`RunAction::List`], the JSON run-listing surface rupu-cp's SSH host
/// connector shells out to).
#[derive(Debug, PartialEq, Eq)]
pub enum RunAction {
    /// Cooperatively pause a running standalone-agent or workflow run at
    /// its next safe boundary. Delegates to the exact same primitive as
    /// `rupu workflow pause <run_id>` ([`crate::cmd::workflow::pause`]:
    /// `RunStore::pause` + the pause marker) — resume with
    /// `rupu run resume <run_id>` (or `rupu workflow resume`).
    Pause {
        /// Full run id (`run_<ULID>`) as printed by `rupu run` / `rupu
        /// workflow run`.
        run_id: String,
    },
    /// Resume a paused/failed run. Delegates to the exact same primitive
    /// as `rupu workflow resume <run_id>` ([`crate::cmd::workflow::resume_run`]):
    /// re-launches from the last checkpoint, skipping already-completed
    /// steps/units.
    Resume {
        /// Full run id (`run_<ULID>`) of the run to resume.
        run_id: String,
        /// Override permission mode for the resumed run
        /// (`ask` | `bypass` | `readonly`).
        mode: Option<String>,
        /// Use the plain line printer instead of the live graph view.
        plain: bool,
    },
    /// One-shot agent run: `rupu run <agent> [target] [prompt] …` (the
    /// default `rupu run` behavior — see [`Args`] for the full flag set).
    /// Carries the raw argv so the caller can re-parse it into [`Args`]
    /// via [`parse_launch_args`].
    Launch(Vec<String>),
    /// `rupu run list` — enumerate the run store as JSON.
    ///
    /// `list` is a reserved first token, like `pause` / `resume`: an agent
    /// literally named `list` is unreachable via `rupu run list`. This is the
    /// same accepted trade-off those two already carry.
    List {
        limit: usize,
        status: Option<String>,
    },
    /// `rupu run show <id>` — one run's detail, as rupu-cp's wire shape.
    ///
    /// `show` is a reserved first token, like `pause` / `resume` / `list`: an
    /// agent literally named `show` is unreachable via `rupu run show`.
    Show { run_id: String },
}

/// Wrapper so [`Args`] (a `clap::Args` flatten target, not itself a
/// `clap::Parser`) can be parsed standalone from the raw argv captured
/// by [`RunAction::Launch`].
#[derive(Parser, Debug)]
#[command(name = "rupu run")]
struct LaunchParser {
    #[command(flatten)]
    args: Args,
}

/// Re-parse the argv captured by [`RunAction::Launch`] into [`Args`] —
/// identical positional/flag semantics to the pre-Task-7 `rupu run
/// <agent> …` parse (agent-first AND flag-first both work, since `Args`
/// is plain `clap::Args`, not a subcommand).
pub(crate) fn parse_launch_args(argv: Vec<String>) -> Result<Args, clap::Error> {
    LaunchParser::try_parse_from(std::iter::once("rupu run".to_string()).chain(argv))
        .map(|wrapper| wrapper.args)
}

/// Standalone parser for just the `resume` control action's flags,
/// re-parsed from the tail of the raw argv by [`classify`].
#[derive(Parser, Debug)]
#[command(name = "rupu run resume")]
struct ResumeArgsParser {
    run_id: String,
    #[arg(long)]
    mode: Option<String>,
    #[arg(long)]
    plain: bool,
}

/// Classify the raw argv captured by `Cmd::Run` (everything after the
/// `run` token — see the module doc above for why this is a raw
/// pre-pass rather than a clap subcommand). Pure / no I/O, so it's
/// unit-testable exactly like the arg-parse tests were pre-fix.
pub fn classify(argv: Vec<String>) -> Result<RunAction, clap::Error> {
    match argv.first().map(String::as_str) {
        Some("pause") => {
            // Reuses `Args`-style clap parsing for just `<run_id>` so
            // arity/help/error messages stay consistent with the rest
            // of the CLI, without dragging `Args`'s much larger flag
            // set into the pause path.
            #[derive(Parser, Debug)]
            #[command(name = "rupu run pause")]
            struct PauseArgsParser {
                run_id: String,
            }
            let parsed = PauseArgsParser::try_parse_from(
                std::iter::once("rupu run pause".to_string()).chain(argv.into_iter().skip(1)),
            )?;
            Ok(RunAction::Pause {
                run_id: parsed.run_id,
            })
        }
        Some("resume") => {
            let parsed = ResumeArgsParser::try_parse_from(
                std::iter::once("rupu run resume".to_string()).chain(argv.into_iter().skip(1)),
            )?;
            Ok(RunAction::Resume {
                run_id: parsed.run_id,
                mode: parsed.mode,
                plain: parsed.plain,
            })
        }
        Some("list") => {
            #[derive(Parser, Debug)]
            #[command(name = "rupu run list")]
            struct ListArgsParser {
                /// Return at most N runs, newest first.
                #[arg(long, default_value_t = 10_000)]
                limit: usize,
                /// Filter by status (`running`, `completed`, `failed`, …).
                #[arg(long)]
                status: Option<String>,
            }
            let parsed = ListArgsParser::try_parse_from(
                std::iter::once("rupu run list".to_string()).chain(argv.into_iter().skip(1)),
            )?;
            Ok(RunAction::List {
                limit: parsed.limit,
                status: parsed.status,
            })
        }
        Some("show") => {
            #[derive(Parser, Debug)]
            #[command(name = "rupu run show")]
            struct ShowArgsParser {
                run_id: String,
            }
            let parsed = ShowArgsParser::try_parse_from(
                std::iter::once("rupu run show".to_string()).chain(argv.into_iter().skip(1)),
            )?;
            Ok(RunAction::Show {
                run_id: parsed.run_id,
            })
        }
        _ => Ok(RunAction::Launch(argv)),
    }
}

pub async fn handle(
    argv: Vec<String>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> ExitCode {
    match classify(argv) {
        Ok(RunAction::Launch(argv)) => match parse_launch_args(argv) {
            Ok(args) => match run_inner(args).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => crate::output::diag::fail(e),
            },
            Err(e) => e.exit(),
        },
        Ok(RunAction::Pause { run_id }) => match crate::cmd::workflow::pause(&run_id).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
        Ok(RunAction::Resume {
            run_id,
            mode,
            plain,
        }) => {
            match crate::cmd::workflow::resume_run(&run_id, mode.as_deref(), plain, false).await {
                Ok(()) => ExitCode::from(0),
                Err(e) => crate::output::diag::fail(e),
            }
        }
        Ok(RunAction::List { limit, status }) => match list(limit, status, global_format).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
        Ok(RunAction::Show { run_id }) => match show(run_id, global_format).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
        Err(e) => e.exit(),
    }
}

/// `rupu run list` — enumerate the run store.
///
/// Sorts **before** truncating. (`rupu workflow runs` does the reverse —
/// `.take(limit)` on unsorted `store.list()` output — so a small `--limit`
/// there returns an arbitrary subset rather than the newest N. Do not
/// replicate that.)
///
/// Emits `rupu_cp::api::runs::RunListRow` verbatim (via
/// [`rupu_cp::api::runs::RunListRow::with_usage`]) — do NOT hand-roll a
/// parallel row shape here. A previous version of this command had its own
/// `RunListJsonRow` that omitted `usage` / `turns` / `duration_ms`; the SSH
/// host connector shells this command and returns its rows unmodified, and
/// the web UI reads `usage.input_tokens` unguarded, so the omission crashed
/// the whole runs list for any remote SSH host. See
/// `crates/rupu-cp/src/api/runs.rs`'s doc comment on `RunListRow`.
///
/// Deliberately does NOT delegate its filter/sort/truncate/build sequence to
/// `rupu_cp::api::runs::query_run_rows` even though `query_run_rows` is now
/// `pub` and does the same four steps: the two functions' status filters are
/// different VOCABULARIES, not just different spellings of the same idea.
/// `--status` here does an EXACT `RunStatus::as_str()` match (any of the 8
/// values); `query_run_rows`'s `lifecycle` parameter is a 3-value GROUP
/// (`active` | `completed` | `failed`, see `rupu_cp::api::runs::in_lifecycle`).
/// Passing `--status` straight through as `lifecycle` would silently change
/// behavior rather than just move code: `--status failed` would start
/// matching `Rejected`/`Cancelled` runs too (folded into the `failed` group),
/// and `--status running`/`paused`/`pending`/`awaiting_approval` would stop
/// filtering at all, because `in_lifecycle`'s `_ => true` fallback treats any
/// unrecognized group name as "no filter". Forcing a shared call here would
/// require either narrowing `--status` to the 3 group names (a breaking CLI
/// change) or reimplementing the exact-match filter locally anyway — which is
/// exactly what this function already does. `RunListRow::with_usage` (the
/// part that genuinely IS shared) is reused; only the filter predicate is
/// not.
async fn list(
    limit: usize,
    status: Option<String>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    // Resolve pricing exactly the way `show()` (below) does: global-only.
    // A MISSING config.toml is fine — `layer_files_locked` already treats
    // that as an empty layer and resolves `PricingConfig::default()` with
    // no error. A PRESENT but malformed one (ISSUES.md I-21) must fail the
    // command instead: these rows carry `usage`-derived cost figures a
    // user reads and trusts, so a config.toml typo must never silently
    // substitute default rates and print a wrong dollar amount as if it
    // were authoritative. (Contrast the `[ui]`-prefs fallback in
    // `cmd/workflow.rs` / `cmd/cron.rs`, which is fine to swallow — a
    // wrong pager/theme default has no correctness stakes.)
    let global_cfg_path = global.join("config.toml");
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), None).map_err(|e| {
        tracing::warn!(
            path = %global_cfg_path.display(),
            error = %e,
            "config.toml failed to parse; refusing to compute run costs from default pricing"
        );
        anyhow::anyhow!(
            "failed to load {}: {e} (run cost figures cannot be trusted from a malformed config; fix or remove the file)",
            global_cfg_path.display()
        )
    })?;

    let mut all: Vec<_> = store
        .list()?
        .into_iter()
        .filter(|r| match &status {
            None => true,
            Some(s) => r.status.as_str() == s.as_str(),
        })
        .collect();

    all.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    all.truncate(limit);

    let rows: Vec<rupu_cp::api::runs::RunListRow> = all
        .iter()
        .map(|r| rupu_cp::api::runs::RunListRow::with_usage(r, &store, &cfg.pricing))
        .collect();

    let report = RunListReport {
        kind: "run_list",
        version: 1,
        summary: RunListSummary {
            count: rows.len(),
            limit,
            status_filter: status,
        },
        rows,
    };

    // `rupu run` has no table renderer for this view; JSON is the contract
    // consumed by rupu-cp's SshHostConnector::list_runs.
    match global_format.unwrap_or(crate::output::formats::OutputFormat::Table) {
        crate::output::formats::OutputFormat::Json => {
            println!("{}", serde_json::to_string(&report)?);
        }
        _ => {
            for row in &report.rows {
                println!(
                    "{}  {}  {}  {}",
                    row.id,
                    row.status.as_str(),
                    row.trigger,
                    row.started_at.to_rfc3339()
                );
            }
        }
    }
    Ok(())
}

/// `rupu run show <id>` — one run's detail, as rupu-cp's wire shape.
///
/// Resolves `PricingConfig` global-only (no project layering), mirroring
/// [`list`]'s global-only `RunStore` resolution. A MISSING `config.toml`
/// resolves to `PricingConfig::default()` with no error — that's the
/// expected fresh-install case. A PRESENT but malformed one FAILS the
/// command (ISSUES.md I-21): this run's cost figures are numbers a user
/// reads and trusts, so a config.toml typo must never silently substitute
/// default rates and print a wrong dollar amount as if it were
/// authoritative.
async fn show(
    run_id: String,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    // Resolve a fragment (compact form / bare suffix / unambiguous prefix)
    // through the SAME resolver `rupu workflow show-run` uses
    // (`cmd::workflow::resolve_run_fragment`), so both commands accept
    // identical identifiers instead of `run show` requiring an exact id
    // while `workflow show-run` accepts a 6-char suffix. Resolved once,
    // right here, then the full id flows downstream.
    let run_id = crate::cmd::workflow::resolve_run_fragment(&store, &run_id)?;
    let global_cfg_path = global.join("config.toml");
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), None).map_err(|e| {
        tracing::warn!(
            path = %global_cfg_path.display(),
            error = %e,
            "config.toml failed to parse; refusing to compute run cost from default pricing"
        );
        anyhow::anyhow!(
            "failed to load {}: {e} (run cost figures cannot be trusted from a malformed config; fix or remove the file)",
            global_cfg_path.display()
        )
    })?;

    // Emit rupu-cp's own detail payload verbatim — do NOT re-shape it here.
    // See `query_run_detail`'s doc comment (crates/rupu-cp/src/api/runs.rs)
    // for why: the SSH `get_run` path shells this exact command and returns
    // the result, so byte-identical output here is what keeps the remote
    // path in sync with the local `mirror_get_run` path (both call
    // `query_run_detail`).
    let item = rupu_cp::api::runs::query_run_detail(&store, &run_id, &cfg.pricing)
        .map_err(|e| anyhow::anyhow!("run {run_id}: {e}"))?;

    match global_format.unwrap_or(crate::output::formats::OutputFormat::Table) {
        crate::output::formats::OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "kind": "run_show",
                    "version": 1,
                    "item": item,
                }))?
            );
        }
        _ => println!("{}", serde_json::to_string_pretty(&item)?),
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct RunListSummary {
    count: usize,
    limit: usize,
    status_filter: Option<String>,
}

/// Contract note: `rows` is `rupu_cp::api::runs::RunListRow`, whose
/// `started_at` / `finished_at` serialize via serde (RFC-3339 with a `Z`
/// suffix), NOT `.to_rfc3339()` (which emits `+00:00`). rupu-cp's fan-out
/// merge sorts these fields with a LEXICOGRAPHIC string compare
/// (`sort_values_newest_first`), and `'+'` (0x2B) sorts before `'Z'` (0x5A) —
/// so a `.to_rfc3339()` row would silently sort as older than it is. Do not
/// "tidy" this into a hand-formatted string; `rupu workflow runs` does that
/// (`%Y-%m-%d %H:%M:%S`) and its rows consequently cannot be merge-sorted
/// against local ones.
#[derive(serde::Serialize)]
struct RunListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<rupu_cp::api::runs::RunListRow>,
    summary: RunListSummary,
}

pub(crate) async fn run_inner(args: Args) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Load the agent (project shadows global). `load_agent` takes the
    // parent of `agents/`, so pass `<global>` and `<project_root>`
    // (not `<project_root>/.rupu`) — but the project layout mounts
    // agents under `<project_root>/.rupu/agents/`, so the parent is
    // `<project_root>/.rupu`.
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let spec = load_agent(&global, project_agents_parent.as_deref(), &args.agent)?;

    // Resolve config (global + project).
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let prefs = UiPrefs::resolve(&cfg.ui, false, None, None, args.view);

    // Resolve permission mode.
    let cli_mode = args.mode.as_deref().and_then(parse_mode);
    let agent_mode = spec.permission_mode.as_deref().and_then(parse_mode);
    // Project-level mode override is rare; v0 reads only the cli/agent/global path.
    let project_mode = None;
    let global_mode = cfg.permission_mode.as_deref().and_then(parse_mode);
    let mode = resolve_mode(cli_mode, agent_mode, project_mode, global_mode);

    // Non-TTY + Ask = abort (spec rule).
    if matches!(mode, PermissionMode::Ask) && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "non-tty + ask mode: rerun with `--mode bypass` or `--mode readonly`, \
             or run from an interactive terminal"
        );
    }

    // Workspace upsert.
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &pwd) {
        warn!(path = %pwd.display(), error = %err, "failed to auto-track checkout");
    }

    // Transcript path. Computed here — earlier than it logically "belongs"
    // relative to the provider build below — because the netflow sink
    // installed right after it needs a concrete path to stream into, and
    // that install must land before `build_for_provider_with_config` (a
    // few lines down) constructs the provider client. See the netflow
    // comment below for why the ordering is load-bearing.
    let run_id = args
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run_{}", Ulid::new()));
    let codename = standalone_codename(
        &run_id,
        &spec.name,
        placed_codename_override(args.run_id.is_some(), std::env::var("RUPU_CODENAME").ok()),
    );
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));

    // `--continue <agent_run_id>`: rebuild the interrupted run's conversation
    // from its transcript (recover-on-interrupt spec §1). Same transcripts
    // dir as the new run, so run it from the same project.
    let mut resume_from: Option<(Vec<rupu_providers::types::Message>, std::path::PathBuf)> =
        match args.continue_from.as_deref() {
            None => None,
            Some(prev) => {
                use rupu_agent::continuation::{
                    prepare_continuation, transcript_agent, Continuation,
                };
                let prev_path = transcripts.join(format!("{prev}.jsonl"));
                // The runner truncates its transcript when it starts, so a
                // `--run-id` that already has one would wipe it — the run
                // being continued (which the continued run seeds from, and
                // whose `Seed` would then reference itself) or any other
                // run, such as an ancestor further up a continuation chain.
                if transcript_path.exists() {
                    anyhow::bail!(
                        "--continue {prev} needs a new run id; --run-id {run_id} already has a transcript and the new run would overwrite it"
                    );
                }
                let prev_agent = transcript_agent(&prev_path)?;
                if prev_agent != spec.name {
                    anyhow::bail!("run {prev} was agent `{prev_agent}`, not `{}`", spec.name);
                }
                match prepare_continuation(&prev_path)? {
                    Continuation::Finished { output } => {
                        println!("{output}");
                        eprintln!(
                            "run {prev} had already finished — printed its recorded answer without calling the model"
                        );
                        return Ok(());
                    }
                    Continuation::Failed { error, seeded_from } => {
                        // A failed continuation's source run may still be
                        // continuable; name it so the user can.
                        let source = seeded_from
                            .as_deref()
                            .and_then(|p| p.file_stem())
                            .and_then(|s| s.to_str())
                            .map(|src| {
                                format!("; run {src}, which it continued, may still be continued with `--continue {src}`")
                            })
                            .unwrap_or_default();
                        anyhow::bail!(
                            "run {prev} ended in failure{} — that is not an interruption, so there is nothing to pick up; start a fresh run instead{source}",
                            error.map(|e| format!(" ({e})")).unwrap_or_default()
                        )
                    }
                    Continuation::Resume {
                        messages,
                        seed_source,
                    } => Some((messages, seed_source)),
                }
            }
        };

    // Netflow capture. Two destinations: this run's own ledger FILE
    // (`<run_id>.jsonl`, rooted at the project when one already has a
    // `.rupu/netflow/` directory, global otherwise — see
    // `crate::netflow_sink::for_run` / `paths::netflow_dir`'s doc
    // comments; a ledger's lifecycle now matches this ONE run, it does
    // not persist traffic across other runs the way the pre-per-run-plan
    // shared ledger did) and this run's transcript (streams live).
    // `Origin::System` egress (auth/oauth, the update checker, ASN
    // refresh, CP fleet traffic) is NOT captured here or anywhere —
    // every production `Origin::System`/`Update`/`Cp` construction site
    // is wired to `Arc::new(NullSink)`; see
    // `crates/rupu-cp/web/src/components/netflow/ScopeDisclosure.tsx`
    // for the full accounting of what is and is not captured.
    //
    // MUST be built before `build_for_provider_with_config` below —
    // this run's provider client and SCM registry take the sink
    // explicitly at construction time, so building it after either of
    // those would leave this run's outbound HTTP unrecorded.
    //
    // `netflow_handle` is kept alive (not just the `Arc<dyn FlowSink>`
    // handed to the provider/registry below) so the tail of this
    // function can `shutdown()` it — see the comment down at
    // `body_result`'s `.await` for why that matters (Fix 2, Task 10
    // review round 2: nothing else in production ever calls
    // `shutdown()`, so the writer task's own periodic ticker is the
    // primary safety net, but a clean CLI exit should still flush
    // promptly rather than rely on that ticker's multi-second cadence).
    let (netflow_sink, netflow_handle) =
        crate::netflow_sink::for_run(&global, project_root.as_deref(), &run_id, &transcript_path);

    // Everything below that can generate outbound HTTP (provider build,
    // SCM registry discovery, repo clone, the agent run itself) is wrapped
    // in this block so `netflow_handle.shutdown()` below runs whether the
    // run below succeeds OR fails (a failed run must still flush its
    // ledger — that's the whole reason invariant 2 exists). Plain `?`
    // inside would otherwise short-circuit straight out of `run_inner`
    // and skip the shutdown entirely.
    let body_result: anyhow::Result<()> = async move {
        // Provider build via CredentialResolver. Wrapped in an `Arc` so the
        // same resolver instance can also be handed to `CliAgentDispatcher`
        // below without a second construction; existing call sites that need
        // `&dyn CredentialResolver` now go through `resolver.as_ref()`.
        let resolver = Arc::new(crate::accounts::resolver_for(&cfg));

        // Build the SCM/issue registry from the same resolver + config the
        // LLM provider factory uses. Cheap when no platforms are configured;
        // missing credentials are skipped with INFO logs. `netflow_sink`
        // was built above, before this run's provider/registry — see the
        // comment there for why the ordering is load-bearing.
        let scm_registry = Arc::new(
            rupu_scm::Registry::discover(resolver.as_ref(), &cfg, netflow_sink.clone()).await,
        );

        let provider_name = provider_factory::resolve_provider_name(
            spec.provider.as_deref(),
            cfg.default_provider.as_deref(),
        );
        let oai_params = provider_factory::openai_compatible_params(&provider_name, &cfg.providers);
        // Pre-flight, before any credential lookup, so a typo'd provider name
        // fails with a config error rather than a confusing auth one.
        //
        // Asks the factory (`is_dispatchable_provider`) rather than
        // re-deriving the rule: this check used to be
        // `!is_builtin_provider(name) && oai_params.is_none()`, which tests
        // the *account name* against the builtin vendor list and so rejected
        // every named account — `[providers.anthropic-work] kind =
        // "anthropic"`, exactly what `rupu auth login --account
        // anthropic-work --kind anthropic` writes and what `rupu auth
        // status`'s KIND column reads back. The factory has dispatched on
        // the resolved *kind* since multi-account landed; only this gate,
        // the one copy of the rule that lived outside it, still looked at
        // the name.
        if !provider_factory::is_dispatchable_provider(&provider_name, &cfg.providers) {
            anyhow::bail!(
                "provider '{provider_name}' is not a built-in provider, is not a declared \
                 account (no [providers.{provider_name}] with a vendor `kind` — declare one \
                 with `rupu auth login --account {provider_name} --kind <vendor>`), and is \
                 not declared as [providers.{provider_name}] with kind = \"openai-compatible\" \
                 and a base_url in config.toml"
            );
        }
        // For an openai-compatible provider, prefer its configured default_model
        // when the agent/spec didn't pin one.
        let model = provider_factory::resolve_model(
            spec.model.as_deref(),
            cfg.default_model.as_deref(),
            oai_params.as_ref().map(|p| p.default_model.as_str()),
        );
        let auth_hint = spec.auth;
        let provider_config = provider_factory::ProviderConfig {
            anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
            anthropic_prompt_cache: spec.anthropic_prompt_cache,
            openai_compatible: oai_params,
            tuning: Some(provider_factory::provider_tuning(
                &provider_name,
                &cfg.providers,
            )),
            kind: provider_factory::resolve_kind(&provider_name, &cfg.providers),
        };
        let (_resolved_auth, mut provider) = provider_factory::build_for_provider_with_config(
            &provider_name,
            &model,
            auth_hint,
            resolver.as_ref(),
            &provider_config,
            netflow_sink.clone(),
        )
        .await?;

        // Print the agent header via the line-stream printer.
        // `run_id`/`transcript_path` were resolved earlier (see the netflow
        // comment above) so they're already in scope here.
        let agent_header_name = spec.name.clone();
        let agent_header_provider = provider_name.clone();
        let agent_header_model = model.clone();

        // Construct ONE LineStreamPrinter for the whole run. Keeping a
        // single instance means a single MultiProgress + ticker — earlier
        // we built one for the header and another for the tail loop, and
        // their indicatif draw targets stomped on each other (visible as
        // two stale spinner rows under heavy tool-call traffic).
        let mut printer = crate::output::LineStreamPrinter::new();
        printer.agent_header(
            &agent_header_name,
            Some(&codename.to_string()),
            &agent_header_provider,
            &agent_header_model,
            &run_id,
        );

        // Tool context config (the path is filled in after target resolution below).
        let bash_timeout = cfg.bash.timeout_secs.unwrap_or(120);
        let bash_allowlist = cfg.bash.env_allowlist.clone().unwrap_or_default();

        // The --prompt flag takes precedence over the positional `prompt` argument.
        // This avoids the positional prompt being mis-parsed as a RunTarget when
        // the caller passes a prompt but no target (e.g. `rupu run agent "github:org/repo ..."`
        // would bind the string to `target` and attempt to parse it as a repo ref).
        let effective_prompt = args.prompt_flag.clone().or_else(|| args.prompt.clone());

        // Disambiguate: if `args.target` parses as a RunTarget, it's a target.
        // Otherwise treat it (plus the remainder) as part of the user prompt.
        let (run_target, user_message) = match args.target.as_deref() {
            None => (None, effective_prompt.unwrap_or_else(|| "go".into())),
            Some(s) => match crate::run_target::parse_run_target(s) {
                Ok(t) => (Some(t), effective_prompt.unwrap_or_else(|| "go".into())),
                Err(_) => {
                    // Not a target → it's the leading word(s) of the prompt.
                    let combined = match effective_prompt.as_deref() {
                        Some(p) => format!("{s} {p}"),
                        None => s.to_string(),
                    };
                    (None, combined)
                }
            },
        };

        // Preload `## Run target` into the agent system prompt when a target is set.
        let agent_system_prompt = match run_target.as_ref() {
            Some(t) => format!(
                "{}\n\n## Run target\n\n{}",
                spec.system_prompt,
                crate::run_target::format_run_target_for_prompt(t),
            ),
            None => spec.system_prompt.clone(),
        };

        // Clone the target repo for Repo/Pr targets. Three destination
        // modes, in priority order:
        //   1. `--tmp`           → tempfile::TempDir, auto-deleted on exit
        //   2. `--into <path>`   → that path, persistent. Refuse if it exists.
        //   3. (no flag)         → `./<repo>/` in cwd, persistent. Refuse if it exists.
        //
        // Refuse-by-default on existing paths to protect uncommitted work
        // and prevent surprising clobbers. The error message points at the
        // available escape hatches.
        //
        // _clone_guard holds the TempDir handle in mode 1 so Drop runs on
        // function exit, keeping the directory alive for the run. Modes 2
        // and 3 set it to None — the user owns cleanup.
        let _clone_guard: Option<tempfile::TempDir>;
        let workspace_path: std::path::PathBuf = match run_target.as_ref() {
            Some(crate::run_target::RunTarget::Repo {
                platform,
                owner,
                repo,
                ..
            })
            | Some(crate::run_target::RunTarget::Pr {
                platform,
                owner,
                repo,
                ..
            }) => {
                let r = rupu_scm::RepoRef {
                    platform: *platform,
                    owner: owner.clone(),
                    repo: repo.clone(),
                };
                let (_account, conn) = scm_registry.repo_for(&r, Some(pwd.as_path()), None)?;

                let (dest, guard) = resolve_clone_dest(&pwd, repo, args.into.as_deref(), args.tmp)?;
                // Brief progress line on stderr so the user knows where the
                // clone is landing — the LineStreamPrinter rail has already
                // printed `▶ <agent>` on stdout and we don't want to break
                // its visual flow with a clone-progress line in the middle.
                eprintln!("  cloning {}/{} → {}", owner, repo, dest.display());
                conn.clone_to(&r, &dest).await?;
                _clone_guard = guard;
                dest
            }
            _ => {
                _clone_guard = None;
                pwd.clone()
            }
        };

        let mode_str = match mode {
            PermissionMode::Ask => "ask",
            PermissionMode::Bypass => "bypass",
            PermissionMode::Readonly => "readonly",
        };

        // Run-store, hoisted above the tool context so it can back both the
        // dispatcher (below) and the run.json write further down — a single
        // `RunStore` instance for the whole `run_inner` call, never two.
        let runs_root = global.join("runs");
        let run_store = Arc::new(rupu_orchestrator::RunStore::new(runs_root.clone()));

        // Build tool context now that the resolved workspace_path is known.
        // Sub-agent dispatch is now wired for bare `rupu run` too, mirroring
        // `rupu workflow run`: `parent_run_id` is this run's id, so any
        // `dispatch_agent`/`dispatch_agents_parallel` tool call anchors its
        // child sub-run(s) under `<run>/sub/`. No event sink is threaded
        // through — bare `rupu run` uses the `LineStreamPrinter`, which
        // renders dispatch children post-hoc from the parent transcript's
        // tool_call/tool_result entries rather than tailing `events.jsonl`.
        let mut findings_base = crate::findings_opts::base_options(&global, &cfg.findings);
        // Engagement selection: resolve the chosen profile(s) into the active
        // set and carry it on the write options, so report_finding routes/gates
        // and asset_mark is offered. Empty selection = the native code path.
        findings_base.engagement = crate::findings_opts::resolve_engagement(
            &global,
            &workspace_path,
            &args.engagement_profiles,
        )?;
        let limits_ctx = rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global);
        let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
            global.clone(),
            project_root.clone(),
            ws.id.clone(),
            workspace_path.clone(),
            Arc::clone(&resolver),
            mode_str.to_string(),
            Arc::clone(&scm_registry),
            Arc::clone(&run_store),
            None,
            cfg.default_provider.clone(),
            cfg.default_model.clone(),
            provider_factory::openai_compatible_map(&cfg.providers),
            provider_factory::provider_tuning_map(&cfg.providers),
            provider_factory::resolve_kind_map(&cfg.providers),
            findings_base.clone(),
            // Standalone `rupu run` has no workflow run ledger to charge;
            // dispatched children are counted by the CP's fallback over
            // sub-run transcripts.
            None,
            limits_ctx.clone(),
        );
        dispatcher.set_namer(rupu_codename::SharedNamer::open_or_init(
            runs_root.join(&run_id).join("codenames.json"),
            || {
                let mut n = rupu_codename::CrewNamer::new(codename.crew.clone());
                if let Some(seg) = codename.segments.last() {
                    n.seed_role(&spec.name, &seg.role);
                }
                n
            },
        ));
        let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;

        let tool_context = ToolContext {
            findings: Some(findings_base.with_profile(resolve_findings_profile(
                args.findings_profile,
                spec.findings_profile,
            ))),
            workspace_path: workspace_path.clone(),
            bash_env_allowlist: bash_allowlist,
            bash_timeout_secs: bash_timeout,
            dispatcher: Some(dispatcher_dyn),
            dispatchable_agents: spec.dispatchable_agents.clone(),
            parent_run_id: Some(run_id.clone()),
            depth: 0,
            coverage_writer: None,
            surface_tag: None,
            run_id: None,
            model: None,
            tool_mappings: None,
            codename: Some(codename.to_string()),
            agent: None,
            provider: None,
        };

        let backend_id = "local_checkout".to_string();
        let repo_ref = standalone_repo_ref(run_target.as_ref(), &workspace_path);
        let issue_ref = standalone_issue_ref(run_target.as_ref());
        let workspace_strategy =
            standalone_workspace_strategy(run_target.as_ref(), &workspace_path, args.tmp);
        let worker_ctx =
            crate::cmd::workflow::default_execution_worker_context(WorkerKind::Cli, None);
        let worker_record = crate::cmd::workflow::upsert_worker_record(
            &global,
            &worker_ctx,
            &backend_id,
            mode_str,
            repo_ref.as_deref(),
        )?;
        let metadata = StandaloneRunMetadata {
            version: StandaloneRunMetadata::VERSION,
            run_id: run_id.clone(),
            session_id: None,
            archived_at: None,
            workspace_path: canonicalize_if_exists(&workspace_path),
            project_root: project_root.clone(),
            repo_ref,
            issue_ref,
            backend_id,
            worker_id: Some(worker_record.worker_id.clone()),
            trigger_source: "run_cli".into(),
            target: if run_target.is_some() {
                args.target.clone()
            } else {
                None
            },
            workspace_strategy,
            // Captured here, before the agent loop starts — the liveness signal
            // `rupu transcript archive|delete` checks (I4: an in-flight run must
            // not be labelled done and deleted mid-write).
            pid: Some(std::process::id()),
        };
        write_metadata(&metadata_path_for_run(&transcripts, &run_id), &metadata)?;

        let decider: Arc<dyn PermissionDecider> = pick_decider(mode, Some(printer.multi_handle()));

        // Discover the model's real limits (spec 2026-09-30 §6.1): agent pin →
        // config → live model list → unknown. Resolved through the same
        // provider the run is about to use, before it moves into the opts.
        let limits = rupu_runtime::model_limits::resolve(
            rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
            &provider_name,
            &model,
            provider.as_mut(),
            &limits_ctx,
        )
        .await;

        let mut opts = AgentRunOpts {
            seed_source: None,
            agent_name: spec.name.clone(),
            agent_system_prompt,
            agent_tools: spec.tools.clone(),
            provider,
            provider_name,
            model,
            run_id: run_id.clone(),
            workspace_id: ws.id.clone(),
            workspace_path: workspace_path.clone(),
            transcript_path: transcript_path.clone(),
            max_turns: spec.max_turns.unwrap_or(50),
            decider,
            tool_context,
            user_message,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: mode_str.to_string(),
            no_stream: args.no_stream,
            // Suppress the agent runner's inline stdout writes; the CLI's
            // line-stream printer reads tokens from the JSONL transcript
            // instead. This prevents duplicate output when the printer is
            // active and ensures clean output when stdout is piped.
            suppress_stream_stdout: true,
            mcp_registry: Some(scm_registry),
            effort: spec.effort,
            context_window: spec.context_window,
            output_format: spec.output_format,
            output_schema: spec.output_schema.clone(),
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
            // Top-level `rupu run` invocation — no parent, depth 0,
            // dispatch surface taken from the agent's frontmatter.
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: spec.dispatchable_agents.clone(),
            step_id: String::new(),
            on_tool_call: None,
            on_stream_event: None,
            on_usage: None,
            concerns: spec.concerns.clone(),
            limits,
            scope_name: None,
            surface_tag: None,
            pause: None,
            codename: Some(codename.to_string()),
        };
        if let Some((messages, seed_source)) = resume_from.take() {
            rupu_agent::continuation::apply_continuation(&mut opts, messages, seed_source);
        }

        // Spawn the agent in a background task and tail the transcript with
        // the line-stream printer while it runs.
        let transcript_path_for_printer = transcript_path.clone();
        let run_id_for_printer = run_id.clone();
        let spec_name_for_printer = spec.name.clone();

        // Capture the run start time before spawning so run.json has an accurate
        // started_at regardless of how long setup takes.
        let started_at = chrono::Utc::now();

        // Run the agent. The printer reads from the JSONL transcript file;
        // since the agent is async and the printer is sync, we run the
        // printer in a background thread that polls the transcript while
        // the tokio task drives the agent.
        let agent_task = tokio::spawn(rupu_agent::run_agent(opts));

        // Tail the transcript in this thread, reusing the printer from
        // the agent_header above so we don't construct a second
        // MultiProgress that would double-render the bottom-row ticker.
        {
            printer.step_start(&spec_name_for_printer, None, None, None);
            let mut tailer = crate::output::TranscriptTailer::new(&transcript_path_for_printer);
            // Live `⇡in ⇣out · $cost` status from the transcript's `Usage`
            // events (TTY only — `usage_live` prints nothing to a pipe),
            // then a priced closing footer.
            let mut usage = LiveUsageTally::default();
            let mut footer_printed = false;

            loop {
                // Sampled BEFORE draining, so the drain below sees every
                // event the finished task wrote — the closing `RunComplete`
                // included — before the loop exits.
                let finished = agent_task.is_finished();
                for ev in tailer.drain() {
                    if footer_printed {
                        continue;
                    }
                    if usage.add(&cfg.pricing, &spec_name_for_printer, &ev) {
                        printer.usage_live(usage.input, usage.output, usage.cost);
                        continue;
                    }
                    match &ev {
                        rupu_transcript::Event::AssistantMessage { content, .. }
                            if !content.trim().is_empty() =>
                        {
                            render_assistant_output(&mut printer, content, prefs.live_view);
                        }
                        rupu_transcript::Event::ToolCall { tool, input, .. } => {
                            let summary =
                                crate::output::workflow_printer::tool_summary(tool, input);
                            printer.tool_call(tool, &summary);
                        }
                        rupu_transcript::Event::RunComplete {
                            status,
                            total_tokens,
                            duration_ms,
                            error,
                            ..
                        } => {
                            footer_printed = true;
                            let dur = std::time::Duration::from_millis(*duration_ms);
                            match status {
                                rupu_transcript::RunStatus::Ok => {
                                    printer.step_done_priced(
                                        &run_id_for_printer,
                                        dur,
                                        usage.footer(*total_tokens),
                                    );
                                }
                                _ => {
                                    let reason = error.as_deref().unwrap_or("unknown");
                                    printer.step_failed(&run_id_for_printer, reason);
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if finished {
                    if !footer_printed {
                        // The task ended without a `RunComplete` (an error
                        // before the loop wrote one): close the frame with
                        // whatever spend was seen.
                        printer.step_done_priced(
                            &run_id_for_printer,
                            std::time::Duration::ZERO,
                            usage.footer(0),
                        );
                    }
                    break;
                }

                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        // Await the agent task; flatten only the JoinError so run.json is written
        // for both success and failure before propagating.
        let run_result = agent_task
            .await
            .map_err(|e| anyhow::anyhow!("agent task panicked: {e}"))?;
        let success = run_result.is_ok();

        // Write run.json so the run is observable via RunStore and the mirror
        // can carry final_output back to the central control-plane.
        {
            let store = &run_store;
            let final_output = if success {
                Some(rupu_orchestrator::read_final_assistant_text(
                    &transcript_path,
                    true,
                    &run_id,
                    "agent",
                ))
            } else {
                None
            };
            let finished_at = chrono::Utc::now();
            // RunStatus is Copy so `status` remains usable after the struct literal.
            let status = if success {
                rupu_orchestrator::RunStatus::Completed
            } else {
                rupu_orchestrator::RunStatus::Failed
            };
            let error_message = run_result.as_ref().err().map(|e| e.to_string());
            let rec = rupu_orchestrator::RunRecord {
                id: run_id.clone(),
                workflow_name: format!("agent:{}", spec.name),
                status,
                inputs: std::collections::BTreeMap::new(),
                event: None,
                workspace_id: ws.id.clone(),
                workspace_path: workspace_path.clone(),
                transcript_dir: transcripts.clone(),
                started_at,
                finished_at: Some(finished_at),
                final_output: final_output.clone(),
                error_message: error_message.clone(),
                awaiting: Vec::new(),
                awaiting_step_id: None,
                approval_prompt: None,
                awaiting_since: None,
                expires_at: None,
                issue_ref: None,
                issue: None,
                parent_run_id: None,
                // "local_checkout" is intentional for standalone agent runs;
                // workflow runs use "local_worktree".
                backend_id: Some(metadata.backend_id.clone()),
                worker_id: Some(worker_record.worker_id.clone()),
                artifact_manifest_path: None,
                runner_pid: None,
                source_wake_id: None,
                active_step_id: None,
                active_step_kind: None,
                active_step_agent: None,
                active_step_transcript_path: None,
                resume_requested_at: None,
                resume_claimed_at: None,
                resume_claimed_by: None,
                resume_mode: None,
                resume_gate_id: None,
                resume_approver: None,
                resume_rerequested_at: None,
                reject_cleanup_pending: None,
                // ISSUES.md I-24: `rupu run` has no on_reject cleanup path of
                // its own, but recording the launch mode here keeps this
                // record consistent with the workflow-run creation site.
                permission_mode: Some(mode_str.to_string()),
                loop_progress: Default::default(),
                gate_decisions: Vec::new(),
                codename: Some(codename.crew.clone()),
            };
            match store.create(rec, "") {
                Ok(_) => {}
                Err(rupu_orchestrator::RunStoreError::AlreadyExists(_)) => {
                    // --run-id was pre-assigned and create already wrote the stub;
                    // update it in-place with the terminal status + final_output.
                    match store.load(&run_id) {
                        Ok(mut loaded) => {
                            loaded.status = status;
                            loaded.finished_at = Some(finished_at);
                            loaded.final_output = final_output;
                            loaded.error_message = error_message;
                            // Under the run lock, on the blocking pool: a cancel
                            // that landed since the load is kept.
                            match store
                                .blocking(move |s| s.update_unless_cancelled(&loaded))
                                .await
                            {
                                Ok(true) => {}
                                Ok(false) => warn!(
                                    "the agent run was cancelled on disk while finishing; keeping that status"
                                ),
                                Err(e) => warn!(error = %e, "failed to update agent run.json"),
                            }
                        }
                        Err(e) => warn!(error = %e, "failed to load agent run for update"),
                    }
                }
                Err(e) => warn!(error = %e, "failed to write agent run.json"),
            }
        }

        // Print a brief footer.
        println!("transcript: {}", transcript_path.display());
        // Propagate agent failure so the CLI exits non-zero on a failed run.
        run_result?;
        Ok(())
    }
    .await;

    // Flush the ledger — including the `Dropped` accounting line if
    // anything overflowed the channel — before the process exits, on
    // BOTH the success and failure paths above. `shutdown()` is the only
    // thing in this binary that guarantees the write lands before exit;
    // the writer task's periodic ticker (see `writer.rs`) is a safety net
    // for long-running daemons like `rupu cp serve`, not a substitute for
    // this for a one-shot CLI process that may exit within milliseconds
    // of the ticker's last check.
    if let Some(handle) = netflow_handle {
        handle.shutdown().await;
    }

    body_result
}

fn render_assistant_output(
    printer: &mut crate::output::LineStreamPrinter,
    content: &str,
    view_mode: LiveViewMode,
) {
    match view_mode {
        LiveViewMode::Full => printer.assistant_chunk(content),
        LiveViewMode::Focused => printer.sideband_event(
            crate::output::palette::Status::Active,
            "assistant output",
            Some(&truncate_single_line(content, 100)),
        ),
        LiveViewMode::Compact => {}
    }
}

fn truncate_single_line(content: &str, max_chars: usize) -> String {
    let trimmed = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut out = String::new();
    for ch in trimmed.chars().take(max_chars) {
        out.push(ch);
    }
    if trimmed.chars().count() > max_chars {
        out.push('…');
    }
    out
}

pub(crate) fn canonicalize_if_exists(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn standalone_repo_ref(
    run_target: Option<&crate::run_target::RunTarget>,
    workspace_path: &Path,
) -> Option<String> {
    match run_target {
        Some(crate::run_target::RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        })
        | Some(crate::run_target::RunTarget::Pr {
            platform,
            owner,
            repo,
            ..
        }) => Some(format!("{platform}:{owner}/{repo}")),
        Some(crate::run_target::RunTarget::Issue {
            tracker, project, ..
        }) => Some(format!("{tracker}:{project}")),
        None => crate::cmd::issues::autodetect_repo_from_path(workspace_path)
            .ok()
            .map(|repo| crate::cmd::issues::canonical_repo_ref(&repo)),
    }
}

pub(crate) fn standalone_issue_ref(
    run_target: Option<&crate::run_target::RunTarget>,
) -> Option<String> {
    match run_target {
        Some(crate::run_target::RunTarget::Issue {
            tracker,
            project,
            number,
        }) => Some(format!("{tracker}:{project}/issues/{number}")),
        _ => None,
    }
}

pub(crate) fn standalone_workspace_strategy(
    run_target: Option<&crate::run_target::RunTarget>,
    workspace_path: &Path,
    tmp: bool,
) -> Option<String> {
    let value = match run_target {
        Some(crate::run_target::RunTarget::Repo { .. })
        | Some(crate::run_target::RunTarget::Pr { .. })
            if tmp =>
        {
            "temporary_clone"
        }
        Some(crate::run_target::RunTarget::Repo { .. })
        | Some(crate::run_target::RunTarget::Pr { .. }) => "direct_clone",
        _ if crate::cmd::issues::autodetect_repo_from_path(workspace_path).is_ok() => {
            "direct_checkout"
        }
        _ => "direct_workspace",
    };
    Some(value.into())
}

pub(crate) fn pick_decider(
    mode: PermissionMode,
    multi: Option<indicatif::MultiProgress>,
) -> Arc<dyn PermissionDecider> {
    match mode {
        PermissionMode::Bypass => Arc::new(BypassDecider),
        PermissionMode::Readonly => Arc::new(ReadonlyDecider),
        PermissionMode::Ask => Arc::new(AskDecider { multi }),
    }
}

/// Resolve where to clone a `<platform>:<owner>/<repo>` target. Returns
/// `(destination_path, optional_tempdir_guard)`. The guard, when
/// `Some`, must be held alive for the duration of the run so its Drop
/// (which deletes the directory) doesn't fire early.
///
/// Three modes, in priority order:
///   1. `tmp == true`              → fresh `tempfile::TempDir`
///   2. `into = Some(p)`           → `p`, persistent. Must not exist.
///   3. (default)                  → `cwd / <repo>`, persistent. Must not exist.
///
/// Modes 2 and 3 refuse-by-default on existing paths to protect
/// uncommitted work; the error message points at `--into`, `--tmp`,
/// or removing the existing directory as escape hatches.
pub(crate) fn resolve_clone_dest(
    cwd: &std::path::Path,
    repo: &str,
    into: Option<&std::path::Path>,
    tmp: bool,
) -> anyhow::Result<(std::path::PathBuf, Option<tempfile::TempDir>)> {
    if tmp {
        let td = tempfile::tempdir()?;
        let path = td.path().to_path_buf();
        return Ok((path, Some(td)));
    }
    let dest = match into {
        Some(p) => p.to_path_buf(),
        None => cwd.join(repo),
    };
    if dest.exists() {
        anyhow::bail!(
            "{} already exists; pass `--into <dir>` to clone elsewhere, \
             `--tmp` for a throwaway clone, or remove the directory first",
            dest.display()
        );
    }
    Ok((dest, None))
}

/// Readonly: deny writers (bash/write_file/edit_file), allow readers.
pub(crate) struct ReadonlyDecider;
impl PermissionDecider for ReadonlyDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        _input: &serde_json::Value,
        _workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        match tool {
            "bash" | "write_file" | "edit_file" => Ok(PermissionDecision::Deny),
            _ => Ok(PermissionDecision::Allow),
        }
    }
}

/// A `rupu run` agent's running spend, accumulated from the transcript
/// `Usage` events the tail loop already reads — compaction summariser calls
/// included, since they are billed. Each event is priced on its own
/// provider/model (a mid-run model swap prices correctly) and cache read /
/// write counts, with the agent as the pricing fallback key.
#[derive(Debug, Default, Clone, PartialEq)]
struct LiveUsageTally {
    input: u64,
    output: u64,
    /// Sum over priced events; `None` until one has a price.
    cost: Option<f64>,
}

impl LiveUsageTally {
    /// Fold `ev` in when it is a `Usage` event; `true` when it was.
    fn add(
        &mut self,
        pricing: &rupu_config::PricingConfig,
        agent: &str,
        ev: &rupu_transcript::Event,
    ) -> bool {
        let rupu_transcript::Event::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_write_tokens,
            ..
        } = ev
        else {
            return false;
        };
        let (input, output, cached, cache_write) = (
            u64::from(*input_tokens),
            u64::from(*output_tokens),
            u64::from(*cached_tokens),
            u64::from(*cache_write_tokens),
        );
        self.input += input;
        self.output += output;
        if let Some(price) = rupu_config::pricing::lookup(pricing, provider, model, agent) {
            self.cost =
                Some(self.cost.unwrap_or(0.0) + price.cost_usd(input, output, cached, cache_write));
        }
        true
    }

    /// The closing footer's spend. `run_complete_tokens` (the transcript's
    /// own total) stands in only when no `Usage` event was seen.
    fn footer(&self, run_complete_tokens: u64) -> crate::output::printer::StepUsage {
        let tokens = self.input + self.output;
        crate::output::printer::StepUsage {
            tokens: if tokens > 0 {
                tokens
            } else {
                run_complete_tokens
            },
            cost: self.cost,
        }
    }
}

/// Ask: stdin-driven prompt for writers; readers always allowed.
///
/// Prompts via [`rupu_agent::PermissionPrompt::for_stdio`], which writes
/// to stderr and reads from stdin. We re-take the stderr lock for each
/// decision so back-to-back prompts don't deadlock.
struct AskDecider {
    /// Clone of the printer's `MultiProgress`. When `Some`, the
    /// permission prompt suspends the spinner via
    /// `MultiProgress::suspend` so the spinner's `\r`-based redraw
    /// on stdout doesn't clobber the prompt the agent runtime just
    /// wrote to stderr. (`\r` is a cursor-level operation; it moves
    /// the same physical cursor that stderr writes are using.)
    /// `None` = no spinner active (non-TTY / test) — prompt directly.
    multi: Option<indicatif::MultiProgress>,
}
impl PermissionDecider for AskDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        input: &serde_json::Value,
        workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        if !matches!(tool, "bash" | "write_file" | "edit_file") {
            return Ok(PermissionDecision::Allow);
        }
        let do_prompt = || -> Result<PermissionDecision, rupu_agent::runner::RunError> {
            let mut stderr = std::io::stderr();
            let mut prompt = rupu_agent::PermissionPrompt::for_stdio(&mut stderr);
            prompt
                .ask(tool, input, workspace)
                .map_err(|e| rupu_agent::runner::RunError::Provider(format!("ask prompt io: {e}")))
        };
        match &self.multi {
            Some(m) => m.suspend(do_prompt),
            None => do_prompt(),
        }
    }
}

/// `RUPU_CODENAME` counts only for a placed launch, which always passes
/// the coordinator-minted `--run-id` too. A bare `rupu run` ignores it, so
/// an ambient `RUPU_CODENAME` left in a user's shell can't stamp one name on
/// every run they start.
pub(crate) fn placed_codename_override(
    run_id_supplied: bool,
    env: Option<String>,
) -> Option<String> {
    env.filter(|_| run_id_supplied)
}

/// Codename for a standalone `rupu run`: a placed unit's coordinator passes
/// its minted name via `RUPU_CODENAME` (see [`placed_codename_override`]);
/// otherwise the run is its own crew.
pub(crate) fn standalone_codename(
    run_id: &str,
    agent: &str,
    env_override: Option<String>,
) -> rupu_codename::Codename {
    env_override
        .and_then(|s| s.parse::<rupu_codename::Codename>().ok())
        .filter(|c| !c.segments.is_empty())
        .unwrap_or_else(|| {
            rupu_codename::Codename::crew_only(rupu_codename::crew_for(run_id))
                .child(rupu_codename::role_word(agent), None)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch_args(argv: &[&str]) -> Result<Args, clap::Error> {
        parse_launch_args(argv.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn findings_profile_flag_parses_in_either_position() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        let args = launch_args(&["sec", "--findings-profile", "summary", "--prompt", "p"]).unwrap();
        assert_eq!(args.findings_profile, Some(Summary));
        let args = launch_args(&["--findings-profile", "full", "sec"]).unwrap();
        assert_eq!(args.findings_profile, Some(Full));
        assert_eq!(args.agent, "sec");
        assert_eq!(launch_args(&["sec"]).unwrap().findings_profile, None);
    }

    #[test]
    fn findings_profile_flag_rejects_an_unknown_profile() {
        let err = launch_args(&["sec", "--findings-profile", "brief"]).unwrap_err();
        assert!(err.to_string().contains("`full` or `summary`"), "{err}");
    }

    #[test]
    fn findings_profile_flag_beats_the_agent_frontmatter() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        assert_eq!(resolve_findings_profile(Some(Full), Some(Summary)), Full);
        assert_eq!(resolve_findings_profile(Some(Summary), Some(Full)), Summary);
        assert_eq!(resolve_findings_profile(None, Some(Summary)), Summary);
        assert_eq!(resolve_findings_profile(None, None), Full);
    }

    fn usage_event(
        model: &str,
        input: u32,
        output: u32,
        purpose: Option<&str>,
    ) -> rupu_transcript::Event {
        rupu_transcript::Event::Usage {
            provider: "anthropic".into(),
            model: model.into(),
            served_model: None,
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: purpose.map(str::to_string),
        }
    }

    #[test]
    fn live_usage_tally_sums_every_usage_event_and_prices_each() {
        // Built-in price: $3 in / $15 out per Mtok.
        let pricing = rupu_config::PricingConfig::default();
        let mut tally = LiveUsageTally::default();
        assert!(tally.add(
            &pricing,
            "coder",
            &usage_event("claude-sonnet-4-6", 1_000_000, 0, None)
        ));
        // A compaction summariser call is billed spend: it counts too.
        assert!(tally.add(
            &pricing,
            "coder",
            &usage_event("claude-sonnet-4-6", 0, 1_000_000, Some("compaction"))
        ));
        assert_eq!((tally.input, tally.output), (1_000_000, 1_000_000));
        assert_eq!(tally.cost, Some(18.0));
        assert_eq!(
            tally.footer(0),
            crate::output::printer::StepUsage {
                tokens: 2_000_000,
                cost: Some(18.0)
            }
        );
    }

    #[test]
    fn live_usage_tally_prices_cache_writes_at_the_write_rate() {
        // Sonnet 4.6 built-in: $3 in / $15 out, $0.30 read, $3.75 write.
        // 1M prompt = 500k read + 300k write + 200k uncached; 100k output.
        let pricing = rupu_config::PricingConfig::default();
        let mut tally = LiveUsageTally::default();
        let ev = rupu_transcript::Event::Usage {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            served_model: None,
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cached_tokens: 500_000,
            cache_write_tokens: 300_000,
            purpose: None,
        };
        assert!(tally.add(&pricing, "coder", &ev));
        let want = 0.2 * 3.0 + 0.5 * 0.30 + 0.3 * 3.75 + 0.1 * 15.0;
        assert!(
            (tally.cost.unwrap() - want).abs() < 1e-9,
            "{:?}",
            tally.cost
        );
    }

    #[test]
    fn live_usage_tally_unpriced_model_is_dash_not_zero() {
        let pricing = rupu_config::PricingConfig::default();
        let mut tally = LiveUsageTally::default();
        tally.add(
            &pricing,
            "coder",
            &usage_event("nonesuch-model-9", 500, 20, None),
        );
        assert_eq!(tally.cost, None);
        assert_eq!(tally.footer(0).tokens, 520);
        // Non-usage events are ignored.
        let other = rupu_transcript::Event::AssistantDelta {
            content: "hi".into(),
        };
        assert!(!tally.add(&pricing, "coder", &other));
    }

    #[test]
    fn live_usage_tally_footer_falls_back_to_run_complete_total() {
        // A transcript that carried no `Usage` events still reports the
        // `RunComplete` total — unpriced, never a made-up `$0.00`.
        let tally = LiveUsageTally::default();
        assert_eq!(
            tally.footer(321),
            crate::output::printer::StepUsage {
                tokens: 321,
                cost: None
            }
        );
    }

    #[test]
    fn standalone_codename_derives_or_honours_env() {
        let id = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W";
        let derived = standalone_codename(id, "triage", None);
        assert_eq!(derived.to_string(), "jade-reef/numbat");
        let placed = standalone_codename(id, "triage", Some("cobalt-harbor/heron#412".into()));
        assert_eq!(placed.to_string(), "cobalt-harbor/heron#412");
        let junk = standalone_codename(id, "triage", Some("junk".into()));
        assert_eq!(junk.to_string(), "jade-reef/numbat");
    }

    #[test]
    fn rupu_codename_env_is_honoured_only_with_an_explicit_run_id() {
        let id = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W";
        let env = || Some("cobalt-harbor/heron#412".to_string());
        // Ambient env, no --run-id: ignored, the run is its own crew.
        let bare = standalone_codename(id, "triage", placed_codename_override(false, env()));
        assert_eq!(bare.to_string(), "jade-reef/numbat");
        // Placed launch (--run-id + RUPU_CODENAME): the coordinator's name.
        let placed = standalone_codename(id, "triage", placed_codename_override(true, env()));
        assert_eq!(placed.to_string(), "cobalt-harbor/heron#412");
        assert_eq!(placed_codename_override(true, None), None);
    }

    #[test]
    fn classify_routes_list_to_list_action() {
        let action = classify(vec!["list".to_string()]).unwrap();
        assert!(
            matches!(action, RunAction::List { .. }),
            "`rupu run list` must classify as List, not Launch"
        );
    }

    #[test]
    fn classify_list_accepts_limit_flag() {
        let action = classify(vec![
            "list".to_string(),
            "--limit".to_string(),
            "10".to_string(),
        ])
        .unwrap();
        match action {
            RunAction::List { limit, .. } => assert_eq!(limit, 10),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn classify_still_launches_bare_agent_name() {
        let action = classify(vec!["my-agent".to_string()]).unwrap();
        assert!(
            matches!(action, RunAction::Launch(_)),
            "a bare agent name must still Launch — `list` is the only new reserved token"
        );
    }

    #[test]
    fn classify_routes_show_to_show_action() {
        let action = classify(vec!["show".to_string(), "run_abc".to_string()]).unwrap();
        match action {
            RunAction::Show { run_id } => assert_eq!(run_id, "run_abc"),
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn classify_still_launches_an_agent_named_like_a_verb_prefix() {
        // `show` joins pause/resume/list as a reserved FIRST token, but a bare
        // agent name must still Launch.
        let action = classify(vec!["my-agent".to_string()]).unwrap();
        assert!(matches!(action, RunAction::Launch(_)));
    }

    /// Build a `RunListRow` for tests without going through
    /// `RunListRow::with_usage` (which needs a `RunStore` + transcripts on
    /// disk) — `RunListRow::from(&RunRecord)` gives zeroed usage/turns/
    /// duration, which is fine for the serialization-shape assertions here.
    fn sample_row(
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rupu_cp::api::runs::RunListRow {
        let rec = rupu_orchestrator::RunRecord {
            id: "run_01".into(),
            workflow_name: "nightly".into(),
            status: rupu_orchestrator::RunStatus::Completed,
            inputs: Default::default(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: std::path::PathBuf::new(),
            transcript_dir: std::path::PathBuf::new(),
            started_at,
            finished_at,
            final_output: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            codename: None,
        };
        rupu_cp::api::runs::RunListRow::from(&rec)
    }

    #[test]
    fn run_list_row_serializes_rfc3339_and_trigger() {
        let started_at: chrono::DateTime<chrono::Utc> = "2026-07-16T14:02:11Z".parse().unwrap();
        let finished_at: chrono::DateTime<chrono::Utc> = "2026-07-16T14:09:02Z".parse().unwrap();
        let row = sample_row(started_at, Some(finished_at));
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["id"], "run_01");
        // No trigger/event/source_wake_id set on the record → "manual".
        assert_eq!(v["trigger"], "manual");
        // RFC-3339 is required for the lexicographic merge sort in rupu-cp.
        assert!(
            v["started_at"].as_str().unwrap().contains('T'),
            "started_at must be RFC-3339, not space-separated"
        );
        // usage/turns/duration_ms must be present (not omitted) — a row
        // missing these blanks the whole web UI (it reads them unguarded).
        assert!(v.get("usage").is_some(), "usage field must be present");
        assert!(v.get("turns").is_some(), "turns field must be present");
        assert!(
            v.as_object().unwrap().contains_key("duration_ms"),
            "duration_ms field must be present"
        );
    }

    #[test]
    fn run_list_row_timestamps_match_rupu_cp_wire_format() {
        // rupu-cp merges local + remote rows with a LEXICOGRAPHIC compare on
        // started_at. If this emits `+00:00` while rupu-cp's RunListRow emits
        // `Z`, every remote row sorts older than it is. Pin the format.
        //
        // `RunListRow.started_at` is `DateTime<Utc>` serialized by serde
        // (which emits a `Z` suffix), not `.to_rfc3339()` (which emits
        // `+00:00`) — so this should hold for free now that the CLI emits
        // `rupu_cp::api::runs::RunListRow` directly instead of a hand-rolled
        // parallel shape. Assert it still does.
        let t: chrono::DateTime<chrono::Utc> = "2026-07-16T07:00:59.397407Z".parse().unwrap();
        let row = sample_row(t, Some(t));
        let v = serde_json::to_value(&row).unwrap();
        let started = v["started_at"].as_str().unwrap();
        assert!(
            started.ends_with('Z'),
            "must serialize with a Z suffix, got {started}"
        );
        assert!(
            !started.contains("+00:00"),
            "must NOT use .to_rfc3339()'s +00:00 offset — it sorts before 'Z': {started}"
        );
    }

    /// I-21: a malformed `config.toml` must fail `rupu run list`/`rupu run
    /// show` rather than silently substituting `PricingConfig::default()`
    /// and printing a cost figure computed from the wrong rates as if it
    /// were authoritative.
    #[tokio::test]
    async fn malformed_config_surfaces_on_the_pricing_path() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join("config.toml"), "not valid toml [[[").unwrap();

        let old_home = std::env::var_os("RUPU_HOME");
        std::env::set_var("RUPU_HOME", &global);

        let list_result = list(10, None, None).await;
        let show_result = show("run_missing".to_string(), None).await;

        match old_home {
            Some(v) => std::env::set_var("RUPU_HOME", v),
            None => std::env::remove_var("RUPU_HOME"),
        }

        assert!(
            list_result.is_err(),
            "`rupu run list` must fail on a malformed config.toml, not fall back to default \
             pricing: {list_result:?}"
        );
        assert!(
            show_result.is_err(),
            "`rupu run show` must fail on a malformed config.toml, not fall back to default \
             pricing: {show_result:?}"
        );
    }

    /// Regression guard: a config.toml that simply doesn't exist yet (the
    /// common fresh-install case) must still resolve pricing defaults with
    /// no error — only a PRESENT-but-malformed file should fail the
    /// command.
    #[tokio::test]
    async fn missing_config_file_still_falls_back_without_erroring() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(&global).unwrap();
        // Deliberately no config.toml written under `global`.

        let old_home = std::env::var_os("RUPU_HOME");
        std::env::set_var("RUPU_HOME", &global);

        let list_result = list(10, None, None).await;

        match old_home {
            Some(v) => std::env::set_var("RUPU_HOME", v),
            None => std::env::remove_var("RUPU_HOME"),
        }

        assert!(
            list_result.is_ok(),
            "a missing (not malformed) config.toml must not fail the command: {list_result:?}"
        );
    }
}

#[cfg(test)]
mod resolve_clone_dest_tests {
    use super::resolve_clone_dest;
    use std::path::Path;

    #[test]
    fn tmp_returns_a_guarded_tmpdir() {
        let cwd = Path::new("/tmp");
        let (dest, guard) = resolve_clone_dest(cwd, "rupu", None, true).unwrap();
        assert!(
            guard.is_some(),
            "tmp mode should hand back the TempDir guard"
        );
        assert!(dest.exists(), "TempDir should already exist on disk");
    }

    #[test]
    fn default_uses_cwd_repo_and_refuses_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (dest, guard) = resolve_clone_dest(tmp.path(), "myrepo", None, false).unwrap();
        assert_eq!(dest, tmp.path().join("myrepo"));
        assert!(
            guard.is_none(),
            "default mode does not hold a TempDir guard"
        );
        std::fs::create_dir(&dest).unwrap();
        let err = resolve_clone_dest(tmp.path(), "myrepo", None, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("already exists"), "got: {msg}");
        assert!(msg.contains("--into"), "error should hint at --into: {msg}");
        assert!(msg.contains("--tmp"), "error should hint at --tmp: {msg}");
    }

    #[test]
    fn into_uses_explicit_path_and_refuses_when_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("custom-name");
        let (dest, guard) = resolve_clone_dest(tmp.path(), "rupu", Some(&target), false).unwrap();
        assert_eq!(dest, target);
        assert!(guard.is_none());
        std::fs::create_dir(&target).unwrap();
        assert!(resolve_clone_dest(tmp.path(), "rupu", Some(&target), false).is_err());
    }
}
