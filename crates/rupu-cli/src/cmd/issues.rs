//! `rupu issues list | show | run` — interactive surface for the
//! `IssueConnector` trait. Mirrors `rupu repos` for the issue side.
//!
//! All three commands auto-detect the target repo from the cwd's
//! git remote (same UX `gh issue list` provides) when `--repo` is
//! not supplied. The detection logic looks up `origin` in `.git/config`
//! and parses common SSH / HTTPS / shorthand forms into a `RepoRef`.

use crate::cmd::completers::workflow_names;
use crate::cmd::ui::UiPrefs;
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, BRAND, DIM};
use crate::output::report::{self, CollectionOutput, DetailOutput};
use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use clap_complete::ArgValueCompleter;
use comfy_table::{Cell, ColumnConstraint, Width};
use rupu_scm::{IssueFilter, IssueRef, IssueState, IssueTracker, Platform, Registry, RepoRef};
use serde::Serialize;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List issues for a repo. Auto-detects repo from cwd's
    /// `origin` remote when `--repo` is omitted.
    List(ListArgs),
    /// Show one issue's full body + metadata.
    Show(ShowArgs),
    /// Convenience: run a workflow with an issue run-target.
    /// Equivalent to `rupu workflow run <name> <issue-ref>` (the
    /// run-target is a positional on `workflow run`, not a flag).
    Run(RunArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ListArgs {
    /// Repo target in the run-target syntax (e.g.
    /// `github:Section9Labs/rupu`). When omitted, auto-detected
    /// from the cwd's git `origin` remote.
    #[arg(long)]
    pub repo: Option<String>,
    /// Filter by state. `open` (default) | `closed` | `all`.
    #[arg(long, default_value = "open")]
    pub state: String,
    /// Filter by label. Repeatable; matched as AND.
    #[arg(long = "label")]
    pub labels: Vec<String>,
    /// Comma-separated label list (alternative to repeating --label).
    /// AND semantics — all listed labels must be present.
    #[arg(long = "labels")]
    pub labels_csv: Option<String>,
    /// Cap on returned issues. Default: 50.
    #[arg(long, default_value_t = 50)]
    pub limit: u32,
    /// Disable colored output (also honored: `NO_COLOR` env var,
    /// `[ui].color = "never"` in config).
    #[arg(long)]
    pub no_color: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ShowArgs {
    /// Issue ref in run-target syntax
    /// (e.g. `github:Section9Labs/rupu/issues/42`). May omit the
    /// `<owner>/<repo>` portion if cwd has a detectable remote —
    /// `#42` or `42` alone resolves against that.
    pub r#ref: String,
    /// Disable colored output.
    #[arg(long)]
    pub no_color: bool,
    #[arg(long, conflicts_with = "no_pager")]
    pub pager: bool,
    #[arg(long, conflicts_with = "pager")]
    pub no_pager: bool,
}

#[derive(ClapArgs, Debug)]
pub struct RunArgs {
    /// Workflow filename stem under `.rupu/workflows/` (or
    /// `<global>/workflows/`). Same name `rupu workflow run`
    /// accepts.
    #[arg(add = ArgValueCompleter::new(workflow_names))]
    pub workflow: String,
    /// Issue ref. Same shorthand rules as `show`.
    pub r#ref: String,
    /// Override permission mode (`ask` | `bypass` | `readonly`).
    #[arg(long)]
    pub mode: Option<String>,
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List(a) => list(a, global_format).await,
        Action::Show(a) => show(a, global_format).await,
        Action::Run(a) => run(a).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List(_) => ("issues list", report::TABLE_JSON_CSV),
        Action::Show(_) => ("issues show", report::TABLE_JSON),
        Action::Run(_) => ("issues run", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct IssueListRow {
    number: u64,
    state: String,
    labels: Vec<String>,
    label_colors: std::collections::BTreeMap<String, String>,
    author: String,
    title: String,
}

#[derive(Serialize)]
struct IssueListCsvRow {
    number: u64,
    state: String,
    labels: String,
    author: String,
    title: String,
}

#[derive(Serialize)]
struct IssueListSummary {
    repo: String,
    state_filter: String,
    labels_filter: Vec<String>,
    count: usize,
}

#[derive(Serialize)]
struct IssueListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<IssueListRow>,
    summary: IssueListSummary,
}

struct IssueListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: IssueListReport,
    csv_rows: Vec<IssueListCsvRow>,
}

#[derive(Serialize)]
struct IssueShowItem {
    issue_ref: String,
    tracker: String,
    project: String,
    number: u64,
    title: String,
    state: String,
    author: String,
    labels: Vec<String>,
    label_colors: std::collections::BTreeMap<String, String>,
    created_at: String,
    updated_at: String,
    body: String,
}

#[derive(Serialize)]
struct IssueShowReport {
    kind: &'static str,
    version: u8,
    item: IssueShowItem,
}

struct IssueShowOutput {
    prefs: UiPrefs,
    report: IssueShowReport,
}

impl CollectionOutput for IssueListOutput {
    type JsonReport = IssueListReport;
    type CsvRow = IssueListCsvRow;

    fn command_name(&self) -> &'static str {
        "issues list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["number", "state", "labels", "author", "title"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["#", "STATE", "LABELS", "AUTHOR", "TITLE"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(row.number.to_string()),
                crate::output::tables::status_cell(&row.state, &self.prefs),
                Cell::new(crate::output::tables::label_chips_with_colors_capped(
                    &row.labels,
                    &row.label_colors,
                    &self.prefs,
                    3,
                )),
                Cell::new(&row.author),
                Cell::new(truncate(&row.title, 60)),
            ]);
        }
        if let Some(col) = table.column_mut(2) {
            col.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(48)));
        }
        println!("{table}");
        Ok(())
    }
}

impl DetailOutput for IssueShowOutput {
    type JsonReport = IssueShowReport;

    fn command_name(&self) -> &'static str {
        "issues show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let width = crossterm::terminal::size()
            .map(|(value, _)| value.max(40) as usize)
            .unwrap_or(100);
        let body = render_issue_show_snapshot(&self.report.item, &self.prefs, width);
        crate::cmd::ui::paginate(&body, &self.prefs)
    }
}

async fn list(args: ListArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let (registry, global, project_root) = build_registry().await?;
    let repo = resolve_repo_or_autodetect(args.repo.as_deref())?;
    let tracker = repo_to_issue_tracker(repo.platform);
    let conn = registry.issues(tracker).ok_or_else(|| {
        anyhow::anyhow!("no {tracker} credential — run `rupu auth login --provider {tracker}`")
    })?;

    // Merge --label (repeatable) and --labels foo,bar (csv) into a
    // single label set. AND match — all listed labels must be present.
    let mut all_labels: Vec<String> = args.labels.clone();
    if let Some(csv) = &args.labels_csv {
        for piece in csv.split(',') {
            let trimmed = piece.trim();
            if !trimmed.is_empty() && !all_labels.iter().any(|l| l == trimmed) {
                all_labels.push(trimmed.to_string());
            }
        }
    }

    let filter = IssueFilter {
        state: parse_state_filter(&args.state)?,
        labels: all_labels.clone(),
        limit: Some(args.limit),
        ..Default::default()
    };
    let project = format!("{}/{}", repo.owner, repo.repo);
    let issues = conn.list_issues(&project, filter).await?;

    if issues.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        // Empty results go to stdout so `rupu issues list ... | wc -l`
        // and similar pipelines see a clean "(none)" line. Echoing
        // `all_labels` (the merged --label + --labels set) tells the
        // user exactly what filter ran.
        println!(
            "(no issues match — state={}, labels={:?})",
            args.state, all_labels
        );
        return Ok(());
    }

    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, args.no_color, None, None, None);
    let rows: Vec<IssueListRow> = issues
        .iter()
        .map(|issue| IssueListRow {
            number: issue.r.number,
            state: match issue.state {
                IssueState::Open => "open".to_string(),
                IssueState::Closed => "closed".to_string(),
            },
            labels: issue.labels.clone(),
            label_colors: issue.label_colors.clone(),
            author: issue.author.clone(),
            title: issue.title.clone(),
        })
        .collect();
    let csv_rows: Vec<IssueListCsvRow> = rows
        .iter()
        .map(|row| IssueListCsvRow {
            number: row.number,
            state: row.state.clone(),
            labels: row.labels.join(","),
            author: row.author.clone(),
            title: row.title.clone(),
        })
        .collect();
    let output = IssueListOutput {
        prefs,
        report: IssueListReport {
            kind: "issue_list",
            version: 1,
            summary: IssueListSummary {
                repo: project,
                state_filter: args.state.clone(),
                labels_filter: all_labels,
                count: rows.len(),
            },
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

fn layered_config(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

async fn show(args: ShowArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let (registry, global, project_root) = build_registry().await?;
    let issue_ref = resolve_issue_ref(&args.r#ref)?;
    let conn = registry.issues(issue_ref.tracker).ok_or_else(|| {
        anyhow::anyhow!(
            "no {} credential — run `rupu auth login --provider {}`",
            issue_ref.tracker,
            issue_ref.tracker,
        )
    })?;
    let issue = conn.get_issue(&issue_ref).await?;
    let cfg = layered_config(&global, project_root.as_deref());
    let pager_flag = if args.pager {
        Some(true)
    } else if args.no_pager {
        Some(false)
    } else {
        None
    };
    let prefs = UiPrefs::resolve(&cfg.ui, args.no_color, None, pager_flag, None);
    let state = match issue.state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    };
    let report = IssueShowReport {
        kind: "issue_show",
        version: 1,
        item: IssueShowItem {
            issue_ref: format!(
                "{}:{}/issues/{}",
                issue.r.tracker, issue.r.project, issue.r.number
            ),
            tracker: issue.r.tracker.to_string(),
            project: issue.r.project.clone(),
            number: issue.r.number,
            title: issue.title.clone(),
            state: state.into(),
            author: issue.author.clone(),
            labels: issue.labels.clone(),
            label_colors: issue.label_colors.clone(),
            created_at: issue.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            updated_at: issue.updated_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            body: issue.body,
        },
    };
    report::emit_detail(global_format, &IssueShowOutput { prefs, report })
}

fn render_issue_show_snapshot(item: &IssueShowItem, prefs: &UiPrefs, width: usize) -> String {
    let mut rows = vec![
        render_issue_show_header_line(item, width),
        String::new(),
        render_issue_show_kv_row("state", &item.state, width),
        render_issue_show_kv_row("author", &item.author, width),
        render_issue_show_kv_row("project", &item.project, width),
        render_issue_show_kv_row("ref", &item.issue_ref, width),
    ];
    let labels = if item.labels.is_empty() {
        "—".to_string()
    } else {
        crate::output::tables::label_chips_with_colors(
            &item.labels,
            &item.label_colors,
            prefs,
        )
    };
    rows.push(render_issue_show_kv_row_raw("labels", &labels, width));
    rows.push(render_issue_show_kv_row("created", &item.created_at, width));
    rows.push(render_issue_show_kv_row("updated", &item.updated_at, width));
    rows.push(String::new());
    rows.push(render_issue_show_section_header("body", width));
    if item.body.trim().is_empty() {
        rows.push(render_issue_show_kv_row("body", "(no body)", width));
    } else {
        let rendered = crate::cmd::ui::highlight_markdown(item.body.trim_end(), prefs);
        for line in rendered.lines() {
            rows.push(truncate_ansi_value(line, width));
        }
    }
    rows.join("\n") + "\n"
}

fn render_issue_show_header_line(item: &IssueShowItem, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "issues show", BRAND);
    let _ = palette::write_colored(&mut buf, "  ", DIM);
    let _ = palette::write_bold_colored(&mut buf, &format!("#{}", item.number), BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &truncate(&item.project, 28), DIM);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &truncate(&item.title, 48), DIM);
    truncate_ansi_value(&buf, width)
}

fn render_issue_show_section_header(label: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, BRAND);
    truncate_ansi_value(&buf, width)
}

fn render_issue_show_kv_row(label: &str, value: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), BRAND);
    let _ = palette::write_colored(
        &mut buf,
        &truncate(value, width.saturating_sub(11)),
        DIM,
    );
    truncate_ansi_value(&buf, width)
}

fn render_issue_show_kv_row_raw(label: &str, value: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), BRAND);
    buf.push_str(value);
    truncate_ansi_value(&buf, width)
}

fn truncate_ansi_value(value: &str, width: usize) -> String {
    if crate::output::printer::visible_len(value) <= width {
        value.to_string()
    } else {
        crate::output::printer::wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    // Resolve the ref now so we fail with a clear error before
    // shelling into the workflow runner. The actual orchestration
    // re-parses it via `parse_run_target`.
    let issue_ref = resolve_issue_ref(&args.r#ref)?;
    let target = format!(
        "{}:{}/issues/{}",
        issue_ref.tracker, issue_ref.project, issue_ref.number
    );
    // Delegate to the existing workflow run-by-name path so we
    // share all the StepFactory + RunStore + UI plumbing.
    super::workflow::run_by_target(&args.workflow, &target, args.mode.as_deref()).await
}

// ── helpers ─────────────────────────────────────────────────────────────

async fn build_registry() -> anyhow::Result<(
    Arc<Registry>,
    std::path::PathBuf,
    Option<std::path::PathBuf>,
)> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &pwd) {
        warn!(path = %pwd.display(), error = %err, "failed to auto-track checkout");
    }
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;
    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);
    Ok((registry, global, project_root))
}

fn parse_state_filter(s: &str) -> anyhow::Result<Option<IssueState>> {
    match s.to_ascii_lowercase().as_str() {
        "open" => Ok(Some(IssueState::Open)),
        "closed" => Ok(Some(IssueState::Closed)),
        "all" | "any" => Ok(None),
        other => anyhow::bail!("--state must be `open`, `closed`, or `all`, got `{other}`"),
    }
}

fn repo_to_issue_tracker(p: Platform) -> IssueTracker {
    match p {
        Platform::Github => IssueTracker::Github,
        Platform::Gitlab => IssueTracker::Gitlab,
    }
}

/// Parse `--repo` (run-target syntax) or auto-detect from cwd's
/// git remote when `--repo` is `None`. Returns a typed `RepoRef`.
pub(crate) fn resolve_repo_or_autodetect(repo_arg: Option<&str>) -> anyhow::Result<RepoRef> {
    if let Some(s) = repo_arg {
        let parsed = crate::run_target::parse_run_target(s)?;
        match parsed {
            crate::run_target::RunTarget::Repo {
                platform,
                owner,
                repo,
                ..
            }
            | crate::run_target::RunTarget::Pr {
                platform,
                owner,
                repo,
                ..
            } => Ok(RepoRef {
                platform,
                owner,
                repo,
            }),
            crate::run_target::RunTarget::Issue {
                tracker, project, ..
            } => {
                let (owner, repo) = project.split_once('/').ok_or_else(|| {
                    anyhow::anyhow!("issue project `{project}` is not `<owner>/<repo>`")
                })?;
                let platform = match tracker {
                    IssueTracker::Github => Platform::Github,
                    IssueTracker::Gitlab => Platform::Gitlab,
                    other => anyhow::bail!(
                        "tracker {other} not yet supported by `rupu issues` (only GitHub / GitLab)"
                    ),
                };
                Ok(RepoRef {
                    platform,
                    owner: owner.into(),
                    repo: repo.into(),
                })
            }
        }
    } else {
        autodetect_repo_from_cwd()
    }
}

/// Resolve a CLI issue-ref string into the canonical
/// `<tracker>:<project>/issues/<N>` form — the same shape persisted
/// on `RunRecord.issue_ref` and used by the orchestrator. Filters
/// (`rupu workflow runs --issue <ref>`) compare against this
/// canonical form so the user can pass any of the accepted shapes.
pub(crate) fn canonical_issue_ref(s: &str) -> anyhow::Result<String> {
    let r = resolve_issue_ref(s)?;
    Ok(format!("{}:{}/issues/{}", r.tracker, r.project, r.number))
}

/// Parse a CLI issue ref. Accepts:
/// - Full run-target form: `github:Section9Labs/rupu/issues/42`.
/// - Just a number (`42` or `#42`) — resolves via cwd autodetect.
/// - `<repo>#42` shorthand: `Section9Labs/rupu#42` — assumes GitHub.
pub(crate) fn resolve_issue_ref(s: &str) -> anyhow::Result<IssueRef> {
    let trimmed = s.trim();

    // Just a number / `#N` form: needs cwd autodetect for the project.
    let bare = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if bare.chars().all(|c| c.is_ascii_digit()) {
        let n: u64 = bare
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid issue number `{bare}`: {e}"))?;
        let repo = autodetect_repo_from_cwd().map_err(|e| {
            anyhow::anyhow!(
                "issue number {n} given without `--repo` and no detectable git remote: {e}"
            )
        })?;
        return Ok(IssueRef {
            tracker: repo_to_issue_tracker(repo.platform),
            project: format!("{}/{}", repo.owner, repo.repo),
            number: n,
        });
    }

    // Full run-target syntax — accept any supported issue tracker.
    if let Ok(crate::run_target::RunTarget::Issue {
        tracker,
        project,
        number,
    }) = crate::run_target::parse_run_target(trimmed)
    {
        return Ok(IssueRef {
            tracker,
            project,
            number,
        });
    }

    // `<owner>/<repo>#N` shorthand — assume GitHub. (GitLab MRs use
    // `!N`; for issues GitLab keeps `#N` too, so this stays portable.)
    if let Some((proj, num_str)) = trimmed.rsplit_once('#') {
        if let Ok(num) = num_str.parse::<u64>() {
            if proj.contains('/') {
                return Ok(IssueRef {
                    tracker: IssueTracker::Github,
                    project: proj.to_string(),
                    number: num,
                });
            }
        }
    }

    anyhow::bail!(
        "could not parse issue ref `{s}` — expected `<tracker>:<project>/issues/<N>`, \
         `<owner>/<repo>#<N>`, or just `<N>` (with cwd autodetect)"
    )
}

/// Autodetect the repo from the cwd's git remote. Walks up looking
/// for `.git/config`, parses the `[remote "origin"]` `url = ...`
/// line, and converts common forms (HTTPS, SSH) into a `RepoRef`.
pub(crate) fn canonical_repo_ref(repo: &RepoRef) -> String {
    format!("{}:{}/{}", repo.platform, repo.owner, repo.repo)
}

fn autodetect_repo_from_cwd() -> anyhow::Result<RepoRef> {
    let pwd = std::env::current_dir()?;
    autodetect_repo_from_path(&pwd)
}

pub(crate) fn autodetect_repo_from_path(path: &Path) -> anyhow::Result<RepoRef> {
    let git_config = find_git_config(path).ok_or_else(|| {
        anyhow::anyhow!("not in a git checkout — pass --repo <platform>:<owner>/<repo>")
    })?;
    let url = read_origin_url(&git_config)?;
    parse_remote_url(&url).ok_or_else(|| {
        anyhow::anyhow!("could not parse origin url `{url}` to <platform>:<owner>/<repo>")
    })
}

fn find_git_config(start: &Path) -> Option<std::path::PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(d) = cur {
        let candidate = d.join(".git").join("config");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = d.parent();
    }
    None
}

pub(crate) fn read_origin_url(config_path: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", config_path.display()))?;
    let mut in_origin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Match `[remote "origin"]` exactly so a [remote "upstream"]
            // section doesn't trigger.
            in_origin = trimmed == "[remote \"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("url = ") {
            return Ok(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("url=") {
            return Ok(rest.trim().to_string());
        }
    }
    anyhow::bail!("no [remote \"origin\"] url in {}", config_path.display())
}

/// Map common origin URL forms to a `RepoRef`. Handles:
/// - `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `ssh://git@github.com/owner/repo`
/// - GitLab equivalents
/// - `github-<alias>:owner/repo` (SSH alias forms used in user `.ssh/config`)
pub(crate) fn parse_remote_url(url: &str) -> Option<RepoRef> {
    let stripped = url.trim().trim_end_matches('/').trim_end_matches(".git");
    // ssh-alias / SSH form: `<host>:owner/repo` (no scheme)
    if !stripped.contains("://") {
        // `git@github.com:owner/repo` or `github-foo:owner/repo`
        if let Some((host_part, path)) = stripped.split_once(':') {
            // Strip a leading `git@` if present.
            let host = host_part.rsplit('@').next().unwrap_or(host_part);
            let platform = host_to_platform(host)?;
            let (owner, repo) = path.split_once('/')?;
            // Some hosts allow nested groups (gitlab). Take the LAST
            // segment as the repo, the rest as owner.
            let (owner, repo) = owner_repo_from_path(&format!("{owner}/{repo}"))?;
            return Some(RepoRef {
                platform,
                owner,
                repo,
            });
        }
        return None;
    }
    // URL form: scheme://[user@]host/path
    let after_scheme = stripped.split_once("://")?.1;
    let (host_and_user, path) = after_scheme.split_once('/')?;
    let host = host_and_user.rsplit('@').next().unwrap_or(host_and_user);
    let platform = host_to_platform(host)?;
    let (owner, repo) = owner_repo_from_path(path)?;
    Some(RepoRef {
        platform,
        owner,
        repo,
    })
}

fn host_to_platform(host: &str) -> Option<Platform> {
    // Match exact + ssh-alias prefix (e.g. `github-personal` from
    // `.ssh/config` Host blocks). Self-hosted GitLab installs need
    // `--repo` since their hostname carries no signal.
    let lower = host.to_ascii_lowercase();
    if lower == "github.com" || lower.starts_with("github-") || lower.starts_with("github.") {
        return Some(Platform::Github);
    }
    if lower == "gitlab.com" || lower.starts_with("gitlab-") || lower.starts_with("gitlab.") {
        return Some(Platform::Gitlab);
    }
    None
}

fn owner_repo_from_path(path: &str) -> Option<(String, String)> {
    let cleaned = path.trim_matches('/').trim_end_matches(".git");
    let mut parts: Vec<&str> = cleaned.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let repo = parts.pop()?.to_string();
    let owner = parts.join("/");
    Some((owner, repo))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_url_https_github() {
        let r = parse_remote_url("https://github.com/Section9Labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "Section9Labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_ssh_github() {
        let r = parse_remote_url("git@github.com:Section9Labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "Section9Labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_ssh_alias() {
        // `.ssh/config` Host alias form: github-daneel:section9labs/rupu.git
        let r = parse_remote_url("github-daneel:section9labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parse_remote_url_gitlab_nested_groups() {
        // GitLab supports nested groups; owner is everything before the last segment.
        let r = parse_remote_url("https://gitlab.com/foo/bar/baz.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "foo/bar");
        assert_eq!(r.repo, "baz");
    }

    #[test]
    fn parse_remote_url_unknown_host_returns_none() {
        assert!(parse_remote_url("https://example.com/owner/repo.git").is_none());
    }

    #[test]
    fn resolve_issue_ref_full_form() {
        let r = resolve_issue_ref("github:Section9Labs/rupu/issues/42").unwrap();
        assert_eq!(r.tracker, IssueTracker::Github);
        assert_eq!(r.project, "Section9Labs/rupu");
        assert_eq!(r.number, 42);
    }

    #[test]
    fn resolve_issue_ref_tracker_native_form() {
        let linear = resolve_issue_ref("linear:team-123/issues/42").unwrap();
        assert_eq!(linear.tracker, IssueTracker::Linear);
        assert_eq!(linear.project, "team-123");
        assert_eq!(linear.number, 42);

        let jira = resolve_issue_ref("jira:acme.atlassian.net/ENG/issues/7").unwrap();
        assert_eq!(jira.tracker, IssueTracker::Jira);
        assert_eq!(jira.project, "acme.atlassian.net/ENG");
        assert_eq!(jira.number, 7);
    }

    #[test]
    fn resolve_issue_ref_shorthand_with_repo() {
        let r = resolve_issue_ref("Section9Labs/rupu#42").unwrap();
        assert_eq!(r.tracker, IssueTracker::Github);
        assert_eq!(r.project, "Section9Labs/rupu");
        assert_eq!(r.number, 42);
    }

    #[test]
    fn resolve_issue_ref_garbage_errors() {
        assert!(resolve_issue_ref("not-a-ref-at-all").is_err());
    }

    #[test]
    fn parse_state_filter_accepts_known_values() {
        assert_eq!(parse_state_filter("open").unwrap(), Some(IssueState::Open));
        assert_eq!(
            parse_state_filter("closed").unwrap(),
            Some(IssueState::Closed)
        );
        assert_eq!(parse_state_filter("all").unwrap(), None);
        assert!(parse_state_filter("garbage").is_err());
    }

    #[test]
    fn truncate_short_is_unchanged() {
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn truncate_long_appends_ellipsis() {
        let out = truncate("0123456789abcdef", 8);
        assert_eq!(out.chars().count(), 8);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn issue_show_snapshot_renders_metadata_and_body() {
        let prefs = crate::cmd::ui::UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            true,
            None,
            Some(false),
            None,
        );
        let item = IssueShowItem {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            tracker: "github".into(),
            project: "Section9Labs/rupu".into(),
            number: 42,
            title: "Fix retained issue snapshot".into(),
            state: "open".into(),
            author: "matt".into(),
            labels: vec!["bug".into(), "ui".into()],
            label_colors: std::collections::BTreeMap::from([
                ("bug".into(), "ff0000".into()),
                ("ui".into(), "00ff00".into()),
            ]),
            created_at: "2026-05-15 12:00:00 UTC".into(),
            updated_at: "2026-05-15 12:30:00 UTC".into(),
            body: "## Summary\n\n- line one\n- line two".into(),
        };

        let rendered = render_issue_show_snapshot(&item, &prefs, 100);
        assert!(rendered.contains("issues show"));
        assert!(rendered.contains("#42"));
        assert!(rendered.contains("state"));
        assert!(rendered.contains("author"));
        assert!(rendered.contains("[bug]"));
        assert!(rendered.contains("## Summary"));
        assert!(rendered.contains("- line one"));
    }

    #[test]
    fn issue_show_snapshot_handles_empty_body() {
        let prefs = crate::cmd::ui::UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            true,
            None,
            Some(false),
            None,
        );
        let item = IssueShowItem {
            issue_ref: "github:Section9Labs/rupu/issues/7".into(),
            tracker: "github".into(),
            project: "Section9Labs/rupu".into(),
            number: 7,
            title: "Empty body".into(),
            state: "closed".into(),
            author: "matt".into(),
            labels: Vec::new(),
            label_colors: std::collections::BTreeMap::new(),
            created_at: "2026-05-15 12:00:00 UTC".into(),
            updated_at: "2026-05-15 12:30:00 UTC".into(),
            body: "".into(),
        };

        let rendered = render_issue_show_snapshot(&item, &prefs, 80);
        assert!(rendered.contains("body"));
        assert!(rendered.contains("(no body)"));
    }
}
