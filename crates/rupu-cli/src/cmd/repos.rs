//! `rupu repos ...` — remote repo listing plus local repo-registry management.

use crate::cmd::issues::{
    autodetect_repo_from_path, canonical_repo_ref, resolve_repo_or_autodetect,
};
use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use rupu_scm::{Platform, Registry};
use rupu_workspace::RepoRegistryStore;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tracing::debug;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List repositories accessible via configured SCM platforms.
    List(ListArgs),
    /// Attach a local checkout to a repo ref in the local registry.
    Attach(TrackArgs),
    /// Mark one tracked checkout as the preferred path for a repo.
    Prefer(TrackArgs),
    /// List locally tracked repo checkouts.
    Tracked(TrackedArgs),
    /// Forget a tracked repo or one tracked path.
    Forget(ForgetArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ListArgs {
    /// Filter to one platform (`github` | `gitlab`). Default: all.
    #[arg(long)]
    pub platform: Option<String>,
    /// Disable colored output (also honored: `NO_COLOR` env,
    /// `[ui].color = "never"` in config).
    #[arg(long)]
    pub no_color: bool,
}

#[derive(ClapArgs, Debug)]
pub struct TrackArgs {
    /// Repo target in the run-target syntax (e.g. `github:Section9Labs/rupu`).
    pub repo: String,
    /// Local checkout path. Defaults to the current working directory.
    pub path: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub struct TrackedArgs {
    /// Disable colored output.
    #[arg(long)]
    pub no_color: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ForgetArgs {
    /// Repo target in the run-target syntax (e.g. `github:Section9Labs/rupu`).
    pub repo: String,
    /// Remove only this local path from the tracked repo. When omitted,
    /// the entire tracked repo record is deleted.
    #[arg(long)]
    pub path: Option<String>,
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List(args) => list_inner(args, global_format).await,
        Action::Attach(args) => attach_inner(args).await,
        Action::Prefer(args) => prefer_inner(args).await,
        Action::Tracked(args) => tracked_inner(args, global_format).await,
        Action::Forget(args) => forget_inner(args).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List(_) => ("repos list", report::TABLE_JSON_CSV),
        Action::Tracked(_) => ("repos tracked", report::TABLE_JSON_CSV),
        Action::Attach(_) => ("repos attach", report::TABLE_ONLY),
        Action::Prefer(_) => ("repos prefer", report::TABLE_ONLY),
        Action::Forget(_) => ("repos forget", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Debug, Clone, Serialize)]
struct RepoListRow {
    platform: String,
    repo: String,
    default_branch: String,
    visibility: String,
}

#[derive(Debug, Clone, Serialize)]
struct RepoListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<RepoListRow>,
}

struct RepoListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: RepoListReport,
}

impl CollectionOutput for RepoListOutput {
    type JsonReport = RepoListReport;
    type CsvRow = RepoListRow;

    fn command_name(&self) -> &'static str {
        "repos list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["platform", "repo", "default_branch", "visibility"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "Platform",
            "Owner/Repo",
            "Default branch",
            "Visibility",
        ]);
        for row in &self.report.rows {
            let visibility_cell = if !self.prefs.use_color() {
                Cell::new(&row.visibility)
            } else if row.visibility == "private" {
                Cell::new("private").fg(comfy_table::Color::DarkGrey)
            } else {
                Cell::new("public").fg(crate::output::tables::status_color("open", &self.prefs)
                    .unwrap_or(comfy_table::Color::Reset))
            };
            table.add_row(vec![
                Cell::new(&row.platform),
                Cell::new(&row.repo),
                Cell::new(&row.default_branch),
                visibility_cell,
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

async fn list_inner(args: ListArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);

    let platforms: Vec<Platform> = match args.platform.as_deref() {
        Some(s) => vec![s.parse().map_err(|e: String| anyhow::anyhow!(e))?],
        None => vec![Platform::Github, Platform::Gitlab],
    };

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, args.no_color, None, None, None);
    let format = global_format.unwrap_or(OutputFormat::Table);

    let mut rows = Vec::new();
    let mut any_listed = false;
    let mut any_skipped = false;
    let mut any_private = false;
    for p in platforms {
        let Some(conn) = registry.repo(p) else {
            if format == OutputFormat::Table {
                crate::output::diag::skip(
                    &prefs,
                    p.to_string(),
                    "no credential",
                    format!("rupu auth login --provider {p}"),
                );
            }
            any_skipped = true;
            continue;
        };
        let repos = conn.list_repos().await?;
        for r in repos {
            if r.private {
                any_private = true;
            }
            rows.push(RepoListRow {
                platform: p.to_string(),
                repo: format!("{}/{}", r.r.owner, r.r.repo),
                default_branch: r.default_branch,
                visibility: if r.private { "private" } else { "public" }.into(),
            });
            any_listed = true;
        }
    }
    if !any_listed && format == OutputFormat::Table {
        if !any_skipped {
            println!("No repos to list across configured platforms.");
        }
        return Ok(());
    }
    let output = RepoListOutput {
        prefs: prefs.clone(),
        report: RepoListReport {
            kind: "repo_list",
            version: 1,
            rows,
        },
    };
    report::emit_collection(Some(format), &output)?;

    if format == OutputFormat::Table && !any_private {
        if let Some(extras) = registry.github_extras() {
            if let Some(scopes) = extras.fetch_token_scopes().await {
                emit_private_repo_diag(&prefs, &scopes);
            }
        }
    }

    Ok(())
}

async fn attach_inner(args: TrackArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let repo = resolve_repo_or_autodetect(Some(&args.repo))?;
    let repo_ref = canonical_repo_ref(&repo);
    let path = checkout_path(args.path.as_deref())?;
    validate_checkout_matches_repo(&path, &repo_ref)?;
    let origin_url = detect_origin_url(&path)?;
    let default_branch = detect_origin_default_branch(&path);
    let rec = store.upsert(
        &repo_ref,
        &path,
        origin_url.as_deref(),
        default_branch.as_deref(),
    )?;
    println!("tracked {} -> {}", rec.repo_ref, rec.preferred_path);
    Ok(())
}

async fn prefer_inner(args: TrackArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let repo = resolve_repo_or_autodetect(Some(&args.repo))?;
    let repo_ref = canonical_repo_ref(&repo);
    let path = checkout_path(args.path.as_deref())?;
    validate_checkout_matches_repo(&path, &repo_ref)?;
    let origin_url = detect_origin_url(&path)?;
    let default_branch = detect_origin_default_branch(&path);
    store.upsert(
        &repo_ref,
        &path,
        origin_url.as_deref(),
        default_branch.as_deref(),
    )?;
    let rec = store.set_preferred_path(&repo_ref, &path)?;
    println!("preferred {} -> {}", rec.repo_ref, rec.preferred_path);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct TrackedRepoRow {
    repo: String,
    preferred_path: String,
    known_paths: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TrackedReposReport {
    kind: &'static str,
    version: u8,
    rows: Vec<TrackedRepoRow>,
}

struct TrackedReposOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: TrackedReposReport,
}

impl CollectionOutput for TrackedReposOutput {
    type JsonReport = TrackedReposReport;
    type CsvRow = TrackedRepoRow;

    fn command_name(&self) -> &'static str {
        "repos tracked"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["repo", "preferred_path", "known_paths", "default_branch"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "Repo",
            "Preferred Path",
            "Known Paths",
            "Default Branch",
        ]);
        for rec in &self.report.rows {
            let branch_cell = match rec.default_branch.as_deref() {
                Some(branch) => Cell::new(branch),
                None => Cell::new("-"),
            };
            let repo_cell = if !self.prefs.use_color() {
                Cell::new(&rec.repo)
            } else {
                Cell::new(&rec.repo).fg(crate::output::tables::status_color("running", &self.prefs)
                    .unwrap_or(comfy_table::Color::Reset))
            };
            table.add_row(vec![
                repo_cell,
                Cell::new(&rec.preferred_path),
                Cell::new(rec.known_paths.to_string()),
                branch_cell,
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

async fn tracked_inner(
    args: TrackedArgs,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, args.no_color, None, None, None);
    let store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let repos = store.list()?;
    if repos.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!("(no tracked repos)");
        return Ok(());
    }
    let rows = repos
        .iter()
        .map(|rec| TrackedRepoRow {
            repo: rec.repo_ref.clone(),
            preferred_path: rec.preferred_path.clone(),
            known_paths: rec.known_paths.len(),
            default_branch: rec.default_branch.clone(),
        })
        .collect::<Vec<_>>();
    let output = TrackedReposOutput {
        prefs,
        report: TrackedReposReport {
            kind: "tracked_repos",
            version: 1,
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

async fn forget_inner(args: ForgetArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let repo = resolve_repo_or_autodetect(Some(&args.repo))?;
    let repo_ref = canonical_repo_ref(&repo);
    if let Some(path) = args.path.as_deref() {
        let removed = store.forget_path(&repo_ref, Path::new(path))?;
        if removed {
            println!("forgot path {} from {}", path, repo_ref);
        } else {
            println!("path {} was not tracked for {}", path, repo_ref);
        }
    } else {
        let removed = store.forget_repo(&repo_ref)?;
        if removed {
            println!("forgot {}", repo_ref);
        } else {
            println!("{} was not tracked", repo_ref);
        }
    }
    Ok(())
}

fn checkout_path(path: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir()?,
    };
    Ok(path.canonicalize()?)
}

fn validate_checkout_matches_repo(path: &Path, expected_repo_ref: &str) -> anyhow::Result<()> {
    let detected = autodetect_repo_from_path(path)?;
    let detected_ref = canonical_repo_ref(&detected);
    if detected_ref != expected_repo_ref {
        anyhow::bail!(
            "checkout {} points at {}, not {}",
            path.display(),
            detected_ref,
            expected_repo_ref
        );
    }
    Ok(())
}

fn detect_origin_url(path: &Path) -> anyhow::Result<Option<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn detect_origin_default_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if let Some(branch) = value.strip_prefix("origin/") {
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }

    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn auto_track_checkout(global: &Path, path: &Path) -> anyhow::Result<bool> {
    let repo = match autodetect_repo_from_path(path) {
        Ok(repo) => repo,
        Err(err) => {
            debug!(path = %path.display(), error = %err, "skipping repo auto-track");
            return Ok(false);
        }
    };
    let repo_ref = canonical_repo_ref(&repo);
    let origin_url = detect_origin_url(path)?;
    let default_branch = detect_origin_default_branch(path);
    let store = RepoRegistryStore {
        root: paths::repos_dir(global),
    };
    store.upsert(
        &repo_ref,
        path,
        origin_url.as_deref(),
        default_branch.as_deref(),
    )?;
    Ok(true)
}

/// Three cases to handle when private repos are absent:
///
/// 1. **Empty `X-OAuth-Scopes` header.** The token is a GitHub App
///    user-to-server token, not a classic OAuth token. GitHub Apps
///    don't grant OAuth scopes; access is per-installation. (rupu's
///    SSO flow uses the GitHub Copilot client_id `Iv1.…` — see
///    `crates/rupu-auth/src/oauth/providers.rs`.) Re-logging in via
///    `--mode sso` won't change anything because no scope is involved.
///    The fix is either an installation-level grant on the user's
///    org, or switching to `--mode api-key` with a classic PAT
///    that has the `repo` scope.
///
/// 2. **Has scopes but `repo` is missing.** Classic OAuth token with
///    insufficient scope. Re-login (or PAT) with the right scope.
///
/// 3. **Has `repo`.** Token is fully privileged; the repos must
///    genuinely not exist or the user lacks access. Say nothing.
fn emit_private_repo_diag(prefs: &crate::cmd::ui::UiPrefs, scopes: &[String]) {
    if scopes.is_empty() {
        crate::output::diag::warn_with_hint(
            prefs,
            "no private github repos shown — your stored token is a GitHub App \
             user-to-server token (rupu impersonates the GitHub Copilot client). \
             GitHub App tokens don't carry OAuth scopes; they grant per-installation \
             access, so private repos only appear from orgs / accounts where the \
             Copilot app is installed and has access to those repos.",
            "use `rupu auth login --provider github --mode api-key` with a classic \
             PAT (https://github.com/settings/tokens) that has the `repo` scope, \
             OR install the Copilot app on the relevant org / repos at \
             https://github.com/settings/installations.",
        );
        return;
    }
    let has_repo = scopes.iter().any(|s| s == "repo");
    if has_repo {
        return;
    }
    let has_public_only = scopes.iter().any(|s| s == "public_repo");
    let detail = if has_public_only {
        "your stored github token only has the `public_repo` scope; \
         private repos require the `repo` scope."
    } else {
        "your stored github token does not have the `repo` scope, \
         which is needed to list private repos."
    };
    crate::output::diag::warn_with_hint(
        prefs,
        format!(
            "no private github repos shown — {detail} (current scopes: {})",
            scopes.join(", ")
        ),
        "rupu auth logout --provider github && rupu auth login --provider github --mode sso",
    );
}
