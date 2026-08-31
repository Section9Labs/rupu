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
use std::path::Path;
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

    append_scm_rule(
        &cfg_path,
        args.owner.as_deref(),
        args.path.as_deref(),
        &args.account,
    )?;

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
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg), project_cfg.as_deref())
        .unwrap_or_default();

    let declared = cfg.scm.platforms.contains_key(account);
    let bare_vendor = account.parse::<rupu_scm::Platform>().is_ok();
    if !declared && !bare_vendor {
        eprintln!(
            "rupu: warning: --account '{account}' is not a declared [scm.*] account or a bare vendor name (github/gitlab) — this rule may never match until one exists.\n  run `rupu scm accounts` to see what's configured, or `rupu auth login --account {account} --kind <github|gitlab>` to declare it."
        );
    }
}

/// Append one `[[scm.rules]]` entry to the config file at `cfg_path`,
/// preserving comments and formatting exactly as
/// `cmd::auth::declare_account_in_config` does for `[providers.*]` /
/// `[scm.*]` account declarations — same `toml_edit::DocumentMut` +
/// `rupu_cp::config_write::write_atomic` technique, reused rather than
/// hand-rolled (see that function's doc comment for why: `toml::Value`
/// has no concept of comments, and `write_atomic` re-validates the
/// resulting config against the schema before it lands).
///
/// Refuses to write when the existing config does not parse: an
/// unparseable config left untouched is safer than "helpfully"
/// replacing it with a file that silently drops whatever was already
/// there.
fn append_scm_rule(
    cfg_path: &Path,
    owner: Option<&str>,
    path: Option<&str>,
    account: &str,
) -> anyhow::Result<()> {
    let text = if cfg_path.exists() {
        std::fs::read_to_string(cfg_path)?
    } else {
        String::new()
    };

    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        anyhow::anyhow!(
            "refusing to write: {} is not valid TOML ({e}). \
             Fix or move the file first — writing would discard its contents.",
            cfg_path.display()
        )
    })?;

    let root = doc.as_table_mut();
    if root.get("scm").is_none() {
        root.insert("scm", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let scm = root
        .get_mut("scm")
        .and_then(|item| item.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("`scm` is already a value, not a table"))?;

    if scm.get("rules").is_none() {
        scm.insert(
            "rules",
            toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
        );
    }
    let rules = scm
        .get_mut("rules")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`scm.rules` is already declared as something other than an array of tables"
            )
        })?;

    let mut entry = toml_edit::Table::new();
    if let Some(o) = owner {
        entry["owner"] = toml_edit::value(o);
    }
    if let Some(p) = path {
        entry["path"] = toml_edit::value(p);
    }
    entry["account"] = toml_edit::value(account);
    rules.push(entry);

    rupu_cp::config_write::write_atomic(cfg_path, &doc.to_string())
        .map_err(|e| anyhow::anyhow!("{e}"))
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
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg), project_cfg.as_deref())
        .unwrap_or_default();

    // Built-in bare vendor names always listed first (mirrors `auth
    // status`'s convention) so a single-account user — who wrote no
    // `[scm.*]` config at all — still sees their account here, not just
    // an empty table.
    let mut names: Vec<String> = vec!["github".to_string(), "gitlab".to_string()];
    for name in cfg.scm.platforms.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }

    let mut rows = Vec::new();
    for name in names {
        let platform_cfg = cfg.scm.platforms.get(&name);
        // Same two-branch kind resolution as `Registry::discover`
        // (design spec §3.1): declared `kind`, else the account name
        // itself parsed as the vendor. An account that resolves to
        // neither isn't a repo account at all (a Linear/Jira table, or
        // a typo) — skip, mirroring `Registry::discover`'s own skip.
        let kind = platform_cfg
            .and_then(|p| p.kind.as_deref())
            .and_then(|k| k.parse::<rupu_scm::Platform>().ok())
            .or_else(|| name.parse::<rupu_scm::Platform>().ok());
        let Some(kind) = kind else { continue };

        let base_url = platform_cfg
            .and_then(|p| p.base_url.clone())
            .unwrap_or_else(|| "-".to_string());

        let mut rule_strs: Vec<String> = Vec::new();
        for rule in &cfg.scm.rules {
            if rule.account != name {
                continue;
            }
            if let Some(o) = &rule.owner {
                rule_strs.push(format!("owner={o}"));
            } else if let Some(p) = &rule.path {
                rule_strs.push(format!("path={p}"));
            }
        }
        let rules = if rule_strs.is_empty() {
            "-".to_string()
        } else {
            rule_strs.join(", ")
        };

        rows.push(ScmAccountRow {
            account: name,
            kind: kind.to_string(),
            base_url,
            rules,
        });
    }

    let output = ScmAccountsOutput {
        report: ScmAccountsReport {
            kind: "scm_accounts",
            version: 1,
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_scm_rule_writes_owner_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["owner"].as_str(), Some("acme/*"));
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
        assert!(rules[0].get("path").is_none());
    }

    #[test]
    fn append_scm_rule_writes_path_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, None, Some("~/Code/work/*"), "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["path"].as_str(), Some("~/Code/work/*"));
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
    }

    /// Two `bind` calls append two entries, not overwrite one — `scm
    /// bind` is additive, matching the walkthrough's three consecutive
    /// binds.
    #[test]
    fn append_scm_rule_appends_rather_than_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap();
        append_scm_rule(&path, Some("MrBrutti/*"), None, "gh-personal").unwrap();
        append_scm_rule(&path, None, Some("~/Code/work/*"), "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        let rules = v["scm"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0]["account"].as_str(), Some("gh-work"));
        assert_eq!(rules[1]["account"].as_str(), Some("gh-personal"));
        assert_eq!(rules[2]["path"].as_str(), Some("~/Code/work/*"));
    }

    /// A comment in the existing config must survive a rule append —
    /// same contract `cmd::auth::declare_account_in_config` pins for
    /// account declarations, and the reason this reuses its
    /// `toml_edit`-based technique rather than a plain `toml::Value`
    /// round-trip.
    #[test]
    fn append_scm_rule_preserves_a_comment_in_the_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# do not remove this note\ndefault_provider = \"anthropic\"\n\n[scm.gh-work]\nkind = \"github\"\n",
        )
        .unwrap();

        append_scm_rule(&path, Some("acme/*"), None, "gh-work").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# do not remove this note"),
            "comment was dropped by the config write:\n{text}"
        );
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            v["scm"]["gh-work"]["kind"].as_str(),
            Some("github"),
            "existing [scm.gh-work] table was dropped:\n{text}"
        );
        assert_eq!(v["scm"]["rules"][0]["owner"].as_str(), Some("acme/*"));
    }

    #[test]
    fn append_scm_rule_refuses_to_clobber_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();

        assert!(append_scm_rule(&path, Some("acme/*"), None, "gh-work").is_err());
        // Erroring is not enough on its own — pin that the file is
        // untouched rather than truncated-then-errored.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not = = toml"
        );
    }

    /// `write_atomic` re-validates the whole document against
    /// `Config::validate` before it lands (`rupu_cp::config_write`) — a
    /// rule with both `owner` and `path` would trip the Task 7 item-3
    /// validation this module's `bind` otherwise can't produce via its
    /// own CLI args (clap already makes them mutually exclusive), but a
    /// pre-existing hand-edited config could. Pin that the write path
    /// rejects it rather than silently landing an invalid rule.
    #[test]
    fn append_scm_rule_rejects_a_result_that_would_fail_config_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Seed a rule that already has both owner and path — appending
        // a second, valid rule still re-validates the WHOLE document,
        // so this must fail.
        std::fs::write(
            &path,
            "[[scm.rules]]\nowner = \"a/*\"\npath = \"~/x\"\naccount = \"gh-work\"\n",
        )
        .unwrap();

        assert!(append_scm_rule(&path, Some("acme/*"), None, "gh-personal").is_err());
    }
}
