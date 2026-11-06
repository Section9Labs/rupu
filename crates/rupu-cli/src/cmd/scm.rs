//! `rupu scm bind` / `rupu scm accounts` — account-selection rules and
//! the multi-account roster (design spec §6.6).
//!
//! `bind` appends one `[[scm.rules]]` entry; `accounts` lists configured
//! accounts, their kind/base_url, and which rules point at them — the
//! command `AccountError::NoRuleMatched`'s ambiguity-error message tells
//! a user to run (`rupu-scm/src/error.rs`'s `fix:` line names `scm
//! bind`; this module is the other half of that contract).

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use serde::Serialize;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Add an account-selection rule: route repos under an owner glob,
    /// or checkouts under a path glob, to a named account.
    Bind(BindArgs),
    /// List configured SCM accounts: kind, base_url, and which rules
    /// point at them.
    Accounts,
}

/// `owner`/`path` as an explicit `ArgGroup(required = true)` rather than
/// the equivalent `conflicts_with`/`required_unless_present` pairing
/// the first version of this struct used: that pairing enforces the
/// same "exactly one" rule at parse time, but clap's auto-generated
/// usage synopsis only reflects a real `ArgGroup` — with the pairing it
/// rendered as `rupu scm bind [OPTIONS] --account <ACCOUNT>`, which
/// reads as if `--owner`/`--path` were both optional. The `ArgGroup`
/// form renders `<--owner <OWNER>|--path <PATH>> --account <ACCOUNT>`.
#[derive(ClapArgs, Debug)]
#[command(group(clap::ArgGroup::new("selector").required(true).args(["owner", "path"])))]
pub struct BindArgs {
    /// Owner glob this rule matches (e.g. `acme/*` or `acme`). Matched
    /// against `RepoRef.owner` — the tier that works for daemons, which
    /// know the owner but have no cwd. Mutually exclusive with `--path`;
    /// exactly one is required.
    #[arg(long)]
    pub owner: Option<String>,
    /// Filesystem-path glob this rule matches against the caller's cwd
    /// (e.g. `~/Code/work/*`). Mutually exclusive with `--owner`.
    #[arg(long)]
    pub path: Option<String>,
    /// The account this rule selects.
    #[arg(long)]
    pub account: String,
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::Bind(args) => bind_inner(args).await,
        Action::Accounts => accounts_inner(global_format).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Bind(_) => ("scm bind", report::TABLE_ONLY),
        Action::Accounts => ("scm accounts", report::TABLE_JSON_CSV),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

async fn bind_inner(args: BindArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let cfg_path = global.join("config.toml");

    warn_if_account_unknown(&args.account);

    rupu_cp::config_write::append_scm_rule(
        &cfg_path,
        args.owner.as_deref(),
        args.path.as_deref(),
        &args.account,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    match (&args.owner, &args.path) {
        (Some(o), _) => println!(
            "rupu: added [[scm.rules]] owner = \"{o}\" -> account = \"{}\"",
            args.account
        ),
        (None, Some(p)) => println!(
            "rupu: added [[scm.rules]] path = \"{p}\" -> account = \"{}\"",
            args.account
        ),
        (None, None) => unreachable!("clap requires exactly one of --owner/--path"),
    }
    Ok(())
}

/// `rupu scm bind`'s validation (Arc 2 final review item 4:
/// `registry.rs`'s `repo_by_account` doc comment claims this exists —
/// it didn't; this makes the claim true). Reads the same layered
/// global+project config `scm accounts` reads, and warns — does not
/// refuse to write — when `account` is neither a declared `[scm.*]`
/// table nor a bare vendor name (`github`/`gitlab`). Non-blocking on
/// purpose: `scm bind` then `auth login --account <name>` is a
/// legitimate forward-declaration order (spec doesn't require the
/// account to exist before a rule names it), and this reads a
/// best-effort snapshot of config that may not exactly match what
/// `Registry::discover` sees at rule-match time. What it closes is the
/// silent gap: today a typo here produces no signal until the next
/// config load's WARN, deep in an unrelated command's log output — see
/// item 1's compounding note: a typo'd bind under one live account
/// silently routes everything to that account.
fn warn_if_account_unknown(account: &str) {
    let Ok(pwd) = std::env::current_dir() else {
        return;
    };
    let project_root = paths::project_root_for(&pwd).ok().flatten();
    let Ok(global) = paths::global_dir() else {
        return;
    };
    let cfg = paths::load_config_for_display(&global, project_root.as_deref(), &pwd, true);

    if !rupu_runtime::account_status::is_known_scm_account(&cfg, account) {
        eprintln!(
            "rupu: warning: --account '{account}' is not a declared [scm.*] account or a bare vendor name (github/gitlab) — this rule may never match until one exists.\n  run `rupu scm accounts` to see what's configured, or `rupu auth login --account {account} --kind <github|gitlab>` to declare it."
        );
    }
}

#[derive(Debug, Clone, Serialize)]
struct ScmAccountRow {
    account: String,
    kind: String,
    base_url: String,
    rules: String,
}

#[derive(Debug, Clone, Serialize)]
struct ScmAccountsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ScmAccountRow>,
}

struct ScmAccountsOutput {
    report: ScmAccountsReport,
}

impl CollectionOutput for ScmAccountsOutput {
    type JsonReport = ScmAccountsReport;
    type CsvRow = ScmAccountRow;

    fn command_name(&self) -> &'static str {
        "scm accounts"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["account", "kind", "base_url", "rules"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["ACCOUNT", "KIND", "BASE URL", "RULES"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.account),
                Cell::new(&row.kind),
                Cell::new(&row.base_url),
                Cell::new(&row.rules),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

/// One row per configured account of a repo platform (github/gitlab) —
/// the two platforms `[[scm.rules]]` can route between (design spec
/// §6.2/§6.3). Linear/Jira are out of scope: `Registry::discover` never
/// treats them as multi-account (they're always looked up under the
/// bare tracker name), and `[[scm.rules]]` never names them, so there is
/// nothing to list for them here.
///
/// Deliberately config-derived rather than `Registry::discover`-derived:
/// this answers "what does my config say", the exact question a user
/// hitting the ambiguity error (`AccountError::NoRuleMatched`) needs
/// answered, and it works with no live credentials and no network call.
/// Mirrors `Registry::discover`'s own account-name/kind resolution
/// (declared `kind`, else the account name itself parsed as the vendor)
/// for WHICH NAMES QUALIFY as a repo account — that part cannot drift.
///
/// It is **not** the same set as `NoRuleMatched`'s `candidates`, though,
/// and does not claim to be: `candidates` comes from
/// `Registry::accounts_for`, filtered to accounts with an actually-built
/// `RepoConnector` (i.e. valid, currently-working credentials) —
/// `github`/`gitlab` and any declared `[scm.<name>]` table can appear
/// here with no credential at all (or a broken one), listed because the
/// config names them, not because a connector exists. A row here is a
/// "what my config declares" answer; a row in the ambiguity error's
/// candidate list is a stronger "and it's live right now" answer.
async fn accounts_inner(global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = paths::load_config_for_display(&global, project_root.as_deref(), &pwd, true);

    let rows = rupu_runtime::account_status::scm_accounts(&cfg)
        .into_iter()
        .map(|a| ScmAccountRow {
            kind: a.kind.to_string(),
            base_url: a.base_url.unwrap_or_else(|| "-".to_string()),
            rules: if a.rules.is_empty() {
                "-".to_string()
            } else {
                a.rules
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            account: a.account,
        })
        .collect();

    let output = ScmAccountsOutput {
        report: ScmAccountsReport {
            kind: "scm_accounts",
            version: 1,
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}
