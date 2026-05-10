//! `rupu usage` — usage reporting across transcripts + workflow run metadata.
//!
//! Default view: last 30 days, composite breakdown by `(provider, model, agent)`
//! plus a summary table. Structured output supports `table`, `json`, and `csv`.

use crate::cmd::usage_report::{UsageDataset, UsageFact, UsageFilter};
use crate::paths;
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand, ValueEnum};
use comfy_table::Cell;
use rupu_transcript::TimeWindow;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageGroupBy {
    Composite,
    Provider,
    Model,
    Agent,
    Workflow,
    Repo,
    Day,
}

#[derive(Args, Debug, Clone, Default)]
pub struct UsageScopeArgs {
    /// Only count runs whose `started_at` is at or after this timestamp.
    /// RFC-3339 / ISO-8601 (`2026-05-01T00:00:00Z`) or relative (`7d`, `24h`, `30m`).
    #[arg(long)]
    pub since: Option<String>,
    /// Only count runs whose `started_at` is at or before this timestamp.
    #[arg(long)]
    pub until: Option<String>,
    /// Filter to one repo ref, for example `github:Section9Labs/rupu`.
    #[arg(long)]
    pub repo: Option<String>,
    /// Filter to one issue ref, for example `github:Section9Labs/rupu/issues/42`.
    #[arg(long)]
    pub issue: Option<String>,
    /// Filter to one workflow name.
    #[arg(long)]
    pub workflow: Option<String>,
    /// Filter to one agent name.
    #[arg(long)]
    pub agent: Option<String>,
    /// Filter to one provider id.
    #[arg(long)]
    pub provider: Option<String>,
    /// Filter to one model id.
    #[arg(long)]
    pub model: Option<String>,
    /// Filter to one worker id.
    #[arg(long)]
    pub worker: Option<String>,
    /// Filter to one backend id.
    #[arg(long)]
    pub backend: Option<String>,
    /// Filter to one trigger source (`run_cli`, `workflow_cli`, `autoflow`, ...).
    #[arg(long)]
    pub trigger: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum UsageCommand {
    /// Show per-run usage rows.
    Runs(UsageRunsArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct UsageRunsArgs {
    #[command(flatten)]
    pub scope: UsageScopeArgs,
    /// Only show failed runs.
    #[arg(long)]
    pub failed: bool,
    /// Filter to one run status.
    #[arg(long)]
    pub status: Option<String>,
    /// Show only the N most expensive runs.
    #[arg(long)]
    pub top_cost: Option<usize>,
}

#[derive(Args, Debug)]
pub struct UsageArgs {
    #[command(flatten)]
    pub scope: UsageScopeArgs,
    /// Group the main report by one dimension.
    #[arg(long, value_enum, default_value_t = UsageGroupBy::Composite)]
    pub group_by: UsageGroupBy,
    #[command(subcommand)]
    pub command: Option<UsageCommand>,
}

pub async fn handle(
    args: UsageArgs,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> ExitCode {
    let result = run(args, global_format).await;
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn run(
    args: UsageArgs,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None);
    let format =
        crate::output::formats::resolve(global_format, crate::output::formats::OutputFormat::Table);
    crate::output::formats::ensure_supported(
        "rupu usage",
        format,
        &[
            crate::output::formats::OutputFormat::Table,
            crate::output::formats::OutputFormat::Json,
            crate::output::formats::OutputFormat::Csv,
        ],
    )?;

    match args.command {
        Some(UsageCommand::Runs(run_args)) => render_run_view(
            &global,
            project_root.as_deref(),
            &cfg,
            &prefs,
            run_args,
            format,
        ),
        None => render_breakdown_view(
            &global,
            project_root.as_deref(),
            &cfg,
            &prefs,
            args.scope,
            args.group_by,
            format,
        ),
    }
}

fn render_breakdown_view(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
    cfg: &rupu_config::Config,
    prefs: &crate::cmd::ui::UiPrefs,
    scope: UsageScopeArgs,
    group_by: UsageGroupBy,
    format: crate::output::formats::OutputFormat,
) -> anyhow::Result<()> {
    let window = resolve_window(&scope)?;
    let dataset = UsageDataset::load(global, project_root, window.window())?;
    let filtered = dataset.filtered(&scope_filter(&scope, None, false));
    let summary = build_summary(&filtered, &cfg.pricing);
    let rows = build_breakdown_rows(&filtered.facts, group_by, &cfg.pricing);
    let report = UsageBreakdownReport {
        kind: "usage_breakdown",
        version: 1,
        window: window.to_output(),
        filters: UsageFiltersOutput::from_scope(&scope, None, false),
        group_by,
        summary,
        rows,
    };

    match format {
        crate::output::formats::OutputFormat::Table => {
            print_breakdown_table(&report, prefs);
            Ok(())
        }
        crate::output::formats::OutputFormat::Json => crate::output::formats::print_json(&report),
        crate::output::formats::OutputFormat::Csv => {
            crate::output::formats::print_csv_rows(&report.rows)
        }
    }
}

fn render_run_view(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
    cfg: &rupu_config::Config,
    prefs: &crate::cmd::ui::UiPrefs,
    args: UsageRunsArgs,
    format: crate::output::formats::OutputFormat,
) -> anyhow::Result<()> {
    let window = resolve_window(&args.scope)?;
    let dataset = UsageDataset::load(global, project_root, window.window())?;
    let filtered = dataset.filtered(&scope_filter(
        &args.scope,
        args.status.as_deref(),
        args.failed,
    ));
    let summary = build_summary(&filtered, &cfg.pricing);
    let rows = build_run_rows(&filtered, &cfg.pricing, args.top_cost);
    let report = UsageRunsReport {
        kind: "usage_runs",
        version: 1,
        window: window.to_output(),
        filters: UsageFiltersOutput::from_scope(&args.scope, args.status.as_deref(), args.failed),
        summary,
        rows,
    };

    match format {
        crate::output::formats::OutputFormat::Table => {
            print_run_table(&report, prefs);
            Ok(())
        }
        crate::output::formats::OutputFormat::Json => crate::output::formats::print_json(&report),
        crate::output::formats::OutputFormat::Csv => {
            crate::output::formats::print_csv_rows(&report.rows)
        }
    }
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

#[derive(Debug, Clone, Copy, Serialize)]
struct ResolvedWindow {
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    default_last_30d: bool,
}

impl ResolvedWindow {
    fn window(self) -> TimeWindow {
        TimeWindow {
            since: self.since,
            until: self.until,
        }
    }

    fn to_output(self) -> UsageWindowOutput {
        UsageWindowOutput {
            since: self.since,
            until: self.until,
            default_last_30d: self.default_last_30d,
            label: format_window_label(self),
        }
    }
}

fn resolve_window(scope: &UsageScopeArgs) -> anyhow::Result<ResolvedWindow> {
    let since = scope
        .since
        .as_deref()
        .map(parse_time_arg)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--since: {e}"))?;
    let until = scope
        .until
        .as_deref()
        .map(parse_time_arg)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--until: {e}"))?;
    if since.is_none() && until.is_none() {
        return Ok(ResolvedWindow {
            since: Some(Utc::now() - chrono::Duration::days(30)),
            until: None,
            default_last_30d: true,
        });
    }
    Ok(ResolvedWindow {
        since,
        until,
        default_last_30d: false,
    })
}

fn scope_filter(scope: &UsageScopeArgs, status: Option<&str>, failed: bool) -> UsageFilter {
    UsageFilter {
        repo_ref: scope.repo.clone(),
        issue_ref: scope.issue.clone(),
        workflow_name: scope.workflow.clone(),
        agent: scope.agent.clone(),
        provider: scope.provider.clone(),
        model: scope.model.clone(),
        worker_id: scope.worker.clone(),
        backend_id: scope.backend.clone(),
        trigger_source: scope.trigger.clone(),
        status: status.map(str::to_string),
        failed_only: failed,
    }
}

/// Accept either a full RFC-3339 timestamp (`2026-05-01T00:00:00Z`)
/// or a relative shorthand (`7d`, `24h`, `30m`, `90s`). Relative
/// forms are interpreted as "now minus that duration" — useful for
/// `--since 7d`. Bare numbers (no unit) are rejected.
fn parse_time_arg(s: &str) -> Result<DateTime<Utc>, String> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    let (num_part, unit) = s.split_at(
        s.char_indices()
            .find(|(_, c)| c.is_alphabetic())
            .map(|(i, _)| i)
            .ok_or_else(|| {
                format!("`{s}` is not RFC-3339 and has no unit (try `7d` / `24h` / `30m`)")
            })?,
    );
    if num_part.is_empty() {
        return Err(format!(
            "`{s}` is missing a number before the unit (try `7d` / `24h` / `30m`)"
        ));
    }
    let n: i64 = num_part
        .parse()
        .map_err(|e| format!("invalid number `{num_part}`: {e}"))?;
    let dur = match unit {
        "s" => chrono::Duration::seconds(n),
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        "w" => chrono::Duration::weeks(n),
        other => return Err(format!("unknown unit `{other}` (expected s/m/h/d/w)")),
    };
    Ok(Utc::now() - dur)
}

#[derive(Debug, Clone, Default, Serialize)]
struct CostTally {
    sum_usd: f64,
    priced_items: u64,
    unpriced_items: u64,
}

impl CostTally {
    fn add(&mut self, cost_usd: Option<f64>) {
        match cost_usd {
            Some(value) => {
                self.sum_usd += value;
                self.priced_items += 1;
            }
            None => self.unpriced_items += 1,
        }
    }

    fn cost_usd(&self) -> Option<f64> {
        (self.priced_items > 0).then_some(self.sum_usd)
    }

    fn partial(&self) -> bool {
        self.priced_items > 0 && self.unpriced_items > 0
    }
}

#[derive(Debug, Clone, Serialize)]
struct UsageTopEntry {
    key: String,
    total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    cost_partial: bool,
}

#[derive(Debug, Clone, Serialize)]
struct UsageSummary {
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cached_tokens: u64,
    total_tokens: u64,
    total_runs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cost_usd: Option<f64>,
    cost_partial: bool,
    top_providers: Vec<UsageTopEntry>,
    top_models: Vec<UsageTopEntry>,
    top_agents: Vec<UsageTopEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageFiltersOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    failed_only: bool,
}

impl UsageFiltersOutput {
    fn from_scope(scope: &UsageScopeArgs, status: Option<&str>, failed_only: bool) -> Self {
        Self {
            repo: scope.repo.clone(),
            issue: scope.issue.clone(),
            workflow: scope.workflow.clone(),
            agent: scope.agent.clone(),
            provider: scope.provider.clone(),
            model: scope.model.clone(),
            worker: scope.worker.clone(),
            backend: scope.backend.clone(),
            trigger: scope.trigger.clone(),
            status: status.map(str::to_string),
            failed_only,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct UsageWindowOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<DateTime<Utc>>,
    default_last_30d: bool,
    label: String,
}

#[derive(Debug, Clone, Serialize)]
struct UsageBreakdownReport {
    kind: &'static str,
    version: u8,
    window: UsageWindowOutput,
    filters: UsageFiltersOutput,
    group_by: UsageGroupBy,
    summary: UsageSummary,
    rows: Vec<UsageBreakdownRow>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageRunsReport {
    kind: &'static str,
    version: u8,
    window: UsageWindowOutput,
    filters: UsageFiltersOutput,
    summary: UsageSummary,
    rows: Vec<UsageRunRow>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageBreakdownRow {
    group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    day: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    runs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    cost_partial: bool,
}

#[derive(Debug, Clone, Serialize)]
struct UsageRunRow {
    started_at: DateTime<Utc>,
    run_id: String,
    source: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<String>,
    providers: String,
    models: String,
    agents: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    cost_partial: bool,
}

#[derive(Default)]
struct GroupAccumulator {
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    run_ids: BTreeSet<String>,
    cost: CostTally,
}

fn build_summary(dataset: &UsageDataset, pricing: &rupu_config::PricingConfig) -> UsageSummary {
    let totals = dataset.totals();
    let total_cost = dataset
        .facts
        .iter()
        .fold(CostTally::default(), |mut tally, fact| {
            tally.add(cost_for_fact(fact, pricing));
            tally
        });
    UsageSummary {
        total_input_tokens: totals.input_tokens,
        total_output_tokens: totals.output_tokens,
        total_cached_tokens: totals.cached_tokens,
        total_tokens: totals.input_tokens + totals.output_tokens,
        total_runs: totals.runs,
        total_cost_usd: total_cost.cost_usd(),
        cost_partial: total_cost.partial(),
        top_providers: top_entries(dataset, pricing, UsageGroupBy::Provider),
        top_models: top_entries(dataset, pricing, UsageGroupBy::Model),
        top_agents: top_entries(dataset, pricing, UsageGroupBy::Agent),
    }
}

fn top_entries(
    dataset: &UsageDataset,
    pricing: &rupu_config::PricingConfig,
    group_by: UsageGroupBy,
) -> Vec<UsageTopEntry> {
    build_breakdown_rows(&dataset.facts, group_by, pricing)
        .into_iter()
        .take(3)
        .map(|row| UsageTopEntry {
            key: row.group,
            total_tokens: row.input_tokens + row.output_tokens,
            cost_usd: row.cost_usd,
            cost_partial: row.cost_partial,
        })
        .collect()
}

fn build_breakdown_rows(
    facts: &[UsageFact],
    group_by: UsageGroupBy,
    pricing: &rupu_config::PricingConfig,
) -> Vec<UsageBreakdownRow> {
    let mut grouped: BTreeMap<String, (UsageBreakdownRow, GroupAccumulator)> = BTreeMap::new();
    for fact in facts {
        let mut row = row_template(group_by, fact);
        let entry = grouped
            .entry(row.group.clone())
            .or_insert_with(|| (row.clone(), GroupAccumulator::default()));
        row = entry.0.clone();
        let acc = &mut entry.1;
        acc.input_tokens += fact.input_tokens;
        acc.output_tokens += fact.output_tokens;
        acc.cached_tokens += fact.cached_tokens;
        acc.run_ids.insert(fact.run_id.clone());
        acc.cost.add(cost_for_fact(fact, pricing));
        entry.0 = row;
    }

    let mut rows = grouped
        .into_values()
        .map(|(mut row, acc)| {
            row.input_tokens = acc.input_tokens;
            row.output_tokens = acc.output_tokens;
            row.cached_tokens = acc.cached_tokens;
            row.runs = acc.run_ids.len() as u64;
            row.cost_usd = acc.cost.cost_usd();
            row.cost_partial = acc.cost.partial();
            row
        })
        .collect::<Vec<_>>();

    rows.sort_by(|a, b| compare_breakdown_rows(a, b, group_by));
    rows
}

fn row_template(group_by: UsageGroupBy, fact: &UsageFact) -> UsageBreakdownRow {
    let day = fact.started_at.format("%Y-%m-%d").to_string();
    match group_by {
        UsageGroupBy::Composite => UsageBreakdownRow {
            group: format!("{} / {} / {}", fact.provider, fact.model, fact.agent),
            provider: Some(fact.provider.clone()),
            model: Some(fact.model.clone()),
            agent: Some(fact.agent.clone()),
            workflow: None,
            repo: None,
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Provider => UsageBreakdownRow {
            group: fact.provider.clone(),
            provider: Some(fact.provider.clone()),
            model: None,
            agent: None,
            workflow: None,
            repo: None,
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Model => UsageBreakdownRow {
            group: fact.model.clone(),
            provider: None,
            model: Some(fact.model.clone()),
            agent: None,
            workflow: None,
            repo: None,
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Agent => UsageBreakdownRow {
            group: fact.agent.clone(),
            provider: None,
            model: None,
            agent: Some(fact.agent.clone()),
            workflow: None,
            repo: None,
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Workflow => UsageBreakdownRow {
            group: fact
                .workflow_name
                .clone()
                .unwrap_or_else(|| "standalone".into()),
            provider: None,
            model: None,
            agent: None,
            workflow: fact.workflow_name.clone().or(Some("standalone".into())),
            repo: None,
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Repo => UsageBreakdownRow {
            group: fact.repo_ref.clone().unwrap_or_else(|| "standalone".into()),
            provider: None,
            model: None,
            agent: None,
            workflow: None,
            repo: fact.repo_ref.clone().or(Some("standalone".into())),
            day: None,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
        UsageGroupBy::Day => UsageBreakdownRow {
            group: day.clone(),
            provider: None,
            model: None,
            agent: None,
            workflow: None,
            repo: None,
            day: Some(day),
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            runs: 0,
            cost_usd: None,
            cost_partial: false,
        },
    }
}

fn compare_breakdown_rows(
    a: &UsageBreakdownRow,
    b: &UsageBreakdownRow,
    group_by: UsageGroupBy,
) -> std::cmp::Ordering {
    if group_by == UsageGroupBy::Day {
        return b.day.cmp(&a.day);
    }
    compare_cost_then_tokens(a.cost_usd, b.cost_usd, a.total_tokens(), b.total_tokens())
        .then_with(|| a.group.cmp(&b.group))
}

fn build_run_rows(
    dataset: &UsageDataset,
    pricing: &rupu_config::PricingConfig,
    top_cost: Option<usize>,
) -> Vec<UsageRunRow> {
    let mut cost_by_run: BTreeMap<&str, CostTally> = BTreeMap::new();
    for fact in &dataset.facts {
        cost_by_run
            .entry(fact.run_id.as_str())
            .or_default()
            .add(cost_for_fact(fact, pricing));
    }

    let mut rows = dataset
        .runs
        .iter()
        .map(|run| {
            let cost = cost_by_run
                .get(run.run_id.as_str())
                .cloned()
                .unwrap_or_default();
            UsageRunRow {
                started_at: run.started_at,
                run_id: run.run_id.clone(),
                source: match run.source {
                    crate::cmd::usage_report::UsageSource::StandaloneRun => "standalone_run",
                    crate::cmd::usage_report::UsageSource::WorkflowRun => "workflow_run",
                }
                .into(),
                status: run.status.clone(),
                workflow: run.workflow_name.clone(),
                repo: run.repo_ref.clone(),
                issue: run.issue_ref.clone(),
                worker: run.worker_id.clone(),
                backend: run.backend_id.clone(),
                trigger: run.trigger_source.clone(),
                providers: join_values(&run.providers),
                models: join_values(&run.models),
                agents: join_values(&run.agents),
                input_tokens: run.input_tokens,
                output_tokens: run.output_tokens,
                cached_tokens: run.cached_tokens,
                cost_usd: cost.cost_usd(),
                cost_partial: cost.partial(),
            }
        })
        .collect::<Vec<_>>();

    if let Some(limit) = top_cost {
        rows.sort_by(|a, b| {
            compare_cost_then_tokens(a.cost_usd, b.cost_usd, a.total_tokens(), b.total_tokens())
                .then_with(|| b.started_at.cmp(&a.started_at))
        });
        rows.truncate(limit);
    } else {
        rows.sort_by_key(|row| std::cmp::Reverse(row.started_at));
    }

    rows
}

fn join_values(values: &[String]) -> String {
    if values.is_empty() {
        "—".into()
    } else {
        values.join(",")
    }
}

fn cost_for_fact(fact: &UsageFact, pricing: &rupu_config::PricingConfig) -> Option<f64> {
    crate::pricing::lookup(pricing, &fact.provider, &fact.model, &fact.agent)
        .map(|price| price.cost_usd(fact.input_tokens, fact.output_tokens, fact.cached_tokens))
}

fn compare_cost_then_tokens(
    left_cost: Option<f64>,
    right_cost: Option<f64>,
    left_tokens: u64,
    right_tokens: u64,
) -> std::cmp::Ordering {
    right_cost
        .partial_cmp(&left_cost)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| right_tokens.cmp(&left_tokens))
}

fn print_breakdown_table(report: &UsageBreakdownReport, prefs: &crate::cmd::ui::UiPrefs) {
    if report.rows.is_empty() {
        println!("(no runs match — try `--since 30d` to widen the window)");
        return;
    }
    print_summary_table(&report.window, &report.summary, prefs);

    let mut table = crate::output::tables::new_table();
    match report.group_by {
        UsageGroupBy::Composite => table.set_header(vec![
            "PROVIDER",
            "MODEL",
            "AGENT",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Provider => table.set_header(vec![
            "PROVIDER",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Model => table.set_header(vec![
            "MODEL",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Agent => table.set_header(vec![
            "AGENT",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Workflow => table.set_header(vec![
            "WORKFLOW",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Repo => table.set_header(vec![
            "REPO",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
        UsageGroupBy::Day => table.set_header(vec![
            "DAY",
            "INPUT",
            "OUTPUT",
            "CACHED",
            "RUNS",
            "COST (USD)",
        ]),
    };

    for row in &report.rows {
        let cost_cell = cost_cell(row.cost_usd, row.cost_partial, prefs);
        match report.group_by {
            UsageGroupBy::Composite => table.add_row(vec![
                Cell::new(row.provider.as_deref().unwrap_or("—")),
                Cell::new(row.model.as_deref().unwrap_or("—")),
                Cell::new(row.agent.as_deref().unwrap_or("—")),
                Cell::new(format_count(row.input_tokens)),
                Cell::new(format_count(row.output_tokens)),
                Cell::new(format_count(row.cached_tokens)),
                Cell::new(row.runs.to_string()),
                cost_cell,
            ]),
            _ => table.add_row(vec![
                Cell::new(&row.group),
                Cell::new(format_count(row.input_tokens)),
                Cell::new(format_count(row.output_tokens)),
                Cell::new(format_count(row.cached_tokens)),
                Cell::new(row.runs.to_string()),
                cost_cell,
            ]),
        };
    }

    table.add_row(match report.group_by {
        UsageGroupBy::Composite => vec![
            Cell::new("TOTAL"),
            Cell::new(""),
            Cell::new(""),
            Cell::new(format_count(report.summary.total_input_tokens)),
            Cell::new(format_count(report.summary.total_output_tokens)),
            Cell::new(format_count(report.summary.total_cached_tokens)),
            Cell::new(report.summary.total_runs.to_string()),
            cost_cell(
                report.summary.total_cost_usd,
                report.summary.cost_partial,
                prefs,
            ),
        ],
        _ => vec![
            Cell::new("TOTAL"),
            Cell::new(format_count(report.summary.total_input_tokens)),
            Cell::new(format_count(report.summary.total_output_tokens)),
            Cell::new(format_count(report.summary.total_cached_tokens)),
            Cell::new(report.summary.total_runs.to_string()),
            cost_cell(
                report.summary.total_cost_usd,
                report.summary.cost_partial,
                prefs,
            ),
        ],
    });

    println!("{table}");
    print_cost_note(
        report.summary.total_cost_usd.is_none(),
        report.summary.cost_partial,
    );
}

fn print_run_table(report: &UsageRunsReport, prefs: &crate::cmd::ui::UiPrefs) {
    if report.rows.is_empty() {
        println!("(no runs match — try `--since 30d` to widen the window)");
        return;
    }
    print_summary_table(&report.window, &report.summary, prefs);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "STARTED_AT",
        "RUN_ID",
        "STATUS",
        "WORKFLOW",
        "REPO",
        "ISSUE",
        "WORKER",
        "BACKEND",
        "TRIGGER",
        "PROVIDERS",
        "MODELS",
        "AGENTS",
        "INPUT",
        "OUTPUT",
        "CACHED",
        "COST (USD)",
    ]);
    for row in &report.rows {
        table.add_row(vec![
            Cell::new(format_timestamp(row.started_at)),
            Cell::new(&row.run_id),
            crate::output::tables::status_cell(&row.status, prefs),
            Cell::new(row.workflow.as_deref().unwrap_or("—")),
            Cell::new(row.repo.as_deref().unwrap_or("—")),
            Cell::new(row.issue.as_deref().unwrap_or("—")),
            Cell::new(row.worker.as_deref().unwrap_or("—")),
            Cell::new(row.backend.as_deref().unwrap_or("—")),
            Cell::new(row.trigger.as_deref().unwrap_or("—")),
            Cell::new(&row.providers),
            Cell::new(&row.models),
            Cell::new(&row.agents),
            Cell::new(format_count(row.input_tokens)),
            Cell::new(format_count(row.output_tokens)),
            Cell::new(format_count(row.cached_tokens)),
            cost_cell(row.cost_usd, row.cost_partial, prefs),
        ]);
    }
    println!("{table}");
    let no_pricing = report.summary.total_cost_usd.is_none();
    print_cost_note(no_pricing, report.summary.cost_partial);
}

fn print_summary_table(
    window: &UsageWindowOutput,
    summary: &UsageSummary,
    prefs: &crate::cmd::ui::UiPrefs,
) {
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["METRIC", "VALUE"]);
    table.add_row(vec![Cell::new("Window"), Cell::new(&window.label)]);
    table.add_row(vec![
        Cell::new("Runs"),
        Cell::new(summary.total_runs.to_string()),
    ]);
    table.add_row(vec![
        Cell::new("Input Tokens"),
        Cell::new(format_count(summary.total_input_tokens)),
    ]);
    table.add_row(vec![
        Cell::new("Output Tokens"),
        Cell::new(format_count(summary.total_output_tokens)),
    ]);
    table.add_row(vec![
        Cell::new("Cached Tokens"),
        Cell::new(format_count(summary.total_cached_tokens)),
    ]);
    table.add_row(vec![
        Cell::new("Total Tokens"),
        Cell::new(format_count(summary.total_tokens)),
    ]);
    table.add_row(vec![
        Cell::new("Total Cost"),
        cost_cell(summary.total_cost_usd, summary.cost_partial, prefs),
    ]);
    table.add_row(vec![
        Cell::new("Top Providers"),
        Cell::new(format_top_entries(&summary.top_providers)),
    ]);
    table.add_row(vec![
        Cell::new("Top Models"),
        Cell::new(format_top_entries(&summary.top_models)),
    ]);
    table.add_row(vec![
        Cell::new("Top Agents"),
        Cell::new(format_top_entries(&summary.top_agents)),
    ]);
    println!("{table}");
}

fn print_cost_note(no_pricing: bool, partial_cost: bool) {
    if no_pricing {
        println!(
            "(no pricing data — add `[pricing.<provider>.\"<model>\"]` or `[pricing.agents.<agent>]` to your config.toml to enable cost)",
        );
    } else if partial_cost {
        println!("(some usage rows have no pricing data; cost totals marked with `*` are partial)");
    }
}

fn format_top_entries(entries: &[UsageTopEntry]) -> String {
    if entries.is_empty() {
        return "—".into();
    }
    entries
        .iter()
        .map(|entry| {
            let cost = format_cost(entry.cost_usd, entry.cost_partial);
            format!(
                "{} ({cost}, {} tok)",
                entry.key,
                format_count(entry.total_tokens)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_window_label(window: ResolvedWindow) -> String {
    if window.default_last_30d {
        return "last 30d".into();
    }
    match (window.since, window.until) {
        (Some(since), Some(until)) => {
            format!("{} .. {}", format_timestamp(since), format_timestamp(until))
        }
        (Some(since), None) => format!("since {}", format_timestamp(since)),
        (None, Some(until)) => format!("until {}", format_timestamp(until)),
        (None, None) => "all time".into(),
    }
}

fn format_timestamp(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%MZ").to_string()
}

fn cost_cell(cost_usd: Option<f64>, partial: bool, prefs: &crate::cmd::ui::UiPrefs) -> Cell {
    match cost_usd {
        Some(value) => Cell::new(format!("${value:.4}{}", if partial { "*" } else { "" })),
        None => {
            if prefs.use_color() {
                Cell::new("\x1b[2m—\x1b[0m")
            } else {
                Cell::new("—")
            }
        }
    }
}

fn format_cost(cost_usd: Option<f64>, partial: bool) -> String {
    match cost_usd {
        Some(value) => format!("${value:.4}{}", if partial { "*" } else { "" }),
        None => "—".into(),
    }
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

impl UsageBreakdownRow {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

impl UsageRunRow {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_arg_accepts_rfc3339() {
        let ts = parse_time_arg("2026-05-01T00:00:00Z").unwrap();
        assert_eq!(ts.timestamp(), 1777593600);
    }

    #[test]
    fn parse_time_arg_accepts_relative_shorthand() {
        let ts = parse_time_arg("1h").unwrap();
        let expected = Utc::now() - chrono::Duration::hours(1);
        let drift = (ts - expected).num_seconds().abs();
        assert!(drift < 5, "drift too large: {drift}s");
    }

    #[test]
    fn parse_time_arg_rejects_bare_number() {
        assert!(parse_time_arg("7").is_err());
    }

    #[test]
    fn parse_time_arg_rejects_unknown_unit() {
        assert!(parse_time_arg("7y").is_err());
    }

    #[test]
    fn resolve_window_defaults_to_last_30d() {
        let window = resolve_window(&UsageScopeArgs::default()).unwrap();
        assert!(window.default_last_30d);
        assert!(window.since.is_some());
        assert!(window.until.is_none());
    }
}
