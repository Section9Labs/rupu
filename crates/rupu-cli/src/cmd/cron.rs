//! `rupu cron list | tick`.
//!
//! Designed to be invoked from system cron at 1-minute granularity:
//!
//!   `* * * * *  /usr/local/bin/rupu cron tick`
//!
//! Each tick walks the global + project workflows directories, picks
//! workflows whose `trigger.on: cron` matches the schedule between
//! the persisted `last_fired` timestamp and now, dispatches them via
//! the same code path as `rupu workflow run`, and records the new
//! `last_fired` per workflow under `<global>/cron-state/<name>.last_fired`.
//!
//! Tick is idempotent at 1-minute granularity: running it twice in
//! the same minute won't fire a `0 4 * * *` workflow twice on the
//! same day. We use `last_fired < schedule_match <= now` semantics.
//!
//! `rupu cron list` is a read-only sanity-check command that prints
//! every cron-triggered workflow + its next firing time.
//!
//! Long-term — see TODO.md → "Workflow triggers" — a native daemon
//! (`rupu cron run`) is the durable answer; this PR is the shipping-
//! today version that delegates scheduling to system cron.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use chrono::{DateTime, Utc};
use clap::Subcommand;
use rupu_config::PollSourceEntry;
use rupu_orchestrator::cron_schedule::{next_fire_after, parse_schedule, should_fire};
use rupu_orchestrator::{annotate_event_payload, matching_event_id, TriggerKind, Workflow};
use rupu_scm::EventSourceRef;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List every cron-triggered workflow + its schedule + next-fire
    /// time. Read-only; doesn't update state.
    List {
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = "never"` in config).
        #[arg(long)]
        no_color: bool,
    },
    /// Walk all workflows, fire any whose schedule matches between
    /// the persisted `last_fired` and now. Designed to run from
    /// system cron at 1-minute granularity.
    Tick {
        /// Don't actually run workflows or update state; just print
        /// what would fire. Useful for verifying a `crontab` line.
        #[arg(long)]
        dry_run: bool,
        /// Run only the cron-scheduled tier; skip event polling.
        /// Useful for crontab lines that want predictable cost.
        #[arg(long, conflicts_with = "only_events")]
        skip_events: bool,
        /// Run only the event-polling tier; skip cron-scheduled fires.
        /// Useful for splitting tick frequencies (cron at 1 min,
        /// events at 5 min).
        #[arg(long, conflicts_with = "skip_events")]
        only_events: bool,
    },
    /// Read-only inspection of event-triggered workflows: prints
    /// each workflow's name, target event id, sources from
    /// `[triggers].poll_sources`, and the most recent persisted
    /// cursor across those sources.
    Events {
        /// Disable colored output.
        #[arg(long)]
        no_color: bool,
    },
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List { no_color } => list(no_color, global_format).await,
        Action::Tick {
            dry_run,
            skip_events,
            only_events,
        } => tick(dry_run, skip_events, only_events).await,
        Action::Events { no_color } => events(no_color, global_format).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("cron list", report::TABLE_JSON_CSV),
        Action::Events { .. } => ("cron events", report::TABLE_JSON_CSV),
        Action::Tick { .. } => ("cron tick", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct CronListRow {
    name: String,
    schedule: String,
    next_utc: Option<String>,
    in_seconds: Option<i64>,
}

#[derive(Serialize)]
struct CronListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CronListRow>,
}

#[derive(Serialize)]
struct CronEventsRow {
    name: String,
    event: String,
    sources: Vec<String>,
    cursor: Option<String>,
}

#[derive(Serialize)]
struct CronEventsSummary {
    poll_sources: Vec<String>,
}

#[derive(Serialize)]
struct CronEventsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CronEventsRow>,
    summary: CronEventsSummary,
}

struct CronListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: CronListReport,
}

impl CollectionOutput for CronListOutput {
    type JsonReport = CronListReport;
    type CsvRow = CronListRow;

    fn command_name(&self) -> &'static str {
        "cron list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name", "schedule", "next_utc", "in_seconds"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "SCHEDULE", "NEXT (UTC)", "IN"]);
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.name),
                comfy_table::Cell::new(&row.schedule),
                comfy_table::Cell::new(row.next_utc.as_deref().unwrap_or("<unschedulable>")),
                match row.in_seconds {
                    Some(delta) => crate::output::tables::relative_time_cell(delta, &self.prefs),
                    None => comfy_table::Cell::new(""),
                },
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

struct CronEventsOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: CronEventsReport,
}

impl CollectionOutput for CronEventsOutput {
    type JsonReport = CronEventsReport;
    type CsvRow = CronEventsRow;

    fn command_name(&self) -> &'static str {
        "cron events"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name", "event", "sources", "cursor"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "EVENT", "SOURCES", "CURSOR"]);
        for row in &self.report.rows {
            let event_cell = comfy_table::Cell::new(&row.event)
                .fg(crate::output::tables::status_color("running", &self.prefs)
                    .unwrap_or(comfy_table::Color::Reset));
            table.add_row(vec![
                comfy_table::Cell::new(&row.name),
                event_cell,
                comfy_table::Cell::new(if row.sources.is_empty() {
                    "(none configured)".to_string()
                } else {
                    row.sources.join(",")
                }),
                comfy_table::Cell::new(truncate(row.cursor.as_deref().unwrap_or("(none)"), 60)),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

async fn list(no_color: bool, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let workflows = collect_cron_workflows()?;
    if workflows.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!(
            "(no cron-triggered workflows found)\n\nAdd `trigger.on: cron` to a workflow under \
             `.rupu/workflows/` and configure a schedule (e.g. `cron: \"0 4 * * *\"`)."
        );
        return Ok(());
    }
    let now = Utc::now();
    let prefs = ui_prefs(no_color)?;
    let rows = workflows
        .iter()
        .map(|workflow| {
            let next = parse_schedule(&workflow.schedule)
                .ok()
                .and_then(|schedule| next_fire_after(&schedule, now));
            CronListRow {
                name: workflow.name.clone(),
                schedule: workflow.schedule.clone(),
                next_utc: next.map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string()),
                in_seconds: next.map(|time| (time - now).num_seconds()),
            }
        })
        .collect();
    let output = CronListOutput {
        prefs,
        report: CronListReport {
            kind: "cron_list",
            version: 1,
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

fn ui_prefs(no_color: bool) -> anyhow::Result<crate::cmd::ui::UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();
    Ok(crate::cmd::ui::UiPrefs::resolve(
        &cfg.ui, no_color, None, None, None,
    ))
}

async fn tick(dry_run: bool, skip_events: bool, only_events: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;

    if !only_events {
        tick_cron(&global, dry_run).await?;
    }
    if !skip_events {
        tick_polled_events(&global, dry_run).await?;
    }
    Ok(())
}

async fn tick_cron(global: &Path, dry_run: bool) -> anyhow::Result<()> {
    let workflows = collect_cron_workflows()?;
    if workflows.is_empty() {
        info!("no cron-triggered workflows found");
        return Ok(());
    }

    let state_dir = global.join("cron-state");
    if !dry_run {
        paths::ensure_dir(&state_dir)?;
    }

    let now = Utc::now();
    for w in &workflows {
        let schedule = match parse_schedule(&w.schedule) {
            Ok(s) => s,
            Err(e) => {
                warn!(workflow = %w.name, error = %e, "skipping: invalid cron expression");
                continue;
            }
        };

        let state_file = state_dir.join(format!("{}.last_fired", w.name));
        let last_fired = read_last_fired(&state_file).ok();

        if !should_fire(&schedule, last_fired, now) {
            continue;
        }

        if dry_run {
            println!(
                "would fire: {} (last_fired={:?}, now={})",
                w.name, last_fired, now
            );
            continue;
        }

        info!(workflow = %w.name, "firing");
        // Persist `last_fired` BEFORE the run so a workflow that
        // overruns into the next tick doesn't double-fire. If the
        // run itself fails, state is still recorded — same semantics
        // as system cron / Kubernetes CronJob.
        if let Err(e) = write_last_fired(&state_file, now) {
            warn!(
                workflow = %w.name,
                error = %e,
                "failed to persist last_fired; firing anyway"
            );
        }
        let inputs: Vec<(String, String)> = Vec::new();
        // Cron-triggered runs have no event payload, so `{{event.*}}`
        // bindings render as empty strings.
        match super::workflow::run_by_name(&w.name, inputs, None, None).await {
            Ok(outcome) => {
                if let Some(step) = outcome.awaiting_step_id {
                    info!(
                        workflow = %w.name,
                        run_id = %outcome.run_id,
                        step = %step,
                        "workflow paused at approval gate; \
                         resume with `rupu workflow approve <run-id>`"
                    );
                }
            }
            Err(e) => {
                warn!(workflow = %w.name, error = %e, "workflow run failed");
            }
        }
    }
    Ok(())
}

/// The polled-events tier of `rupu cron tick`. For each repo configured
/// in `[triggers].poll_sources`, ask the connector for events since
/// the last cursor; for each event, walk event-triggered workflows
/// looking for matches. Cursor is persisted BEFORE dispatch to ensure
/// we don't re-process events on a crash mid-run.
///
/// Spec: design §4.1, §6.2, §10. Plan 1 task 6 + task 8.
async fn tick_polled_events(global: &Path, dry_run: bool) -> anyhow::Result<()> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

    let triggers_cfg = &cfg.triggers;
    if triggers_cfg.poll_sources.is_empty() {
        debug!("no [triggers].poll_sources configured; skipping event poll");
        return Ok(());
    }
    let max = triggers_cfg.effective_max_events_per_tick();

    let event_workflows = collect_event_workflows()?;
    if event_workflows.is_empty() {
        debug!("no event-triggered workflows found; skipping event poll");
        return Ok(());
    }

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);

    let cursors_root = global.join("cron-state").join("event-cursors");
    if !dry_run {
        paths::ensure_dir(&cursors_root)?;
    }

    for source in &triggers_cfg.poll_sources {
        let source_ref = source.source();
        let Ok(event_source) = source_ref.parse::<EventSourceRef>() else {
            warn!(source = %source_ref, "invalid poll_sources entry");
            continue;
        };
        let last_polled_file = last_polled_at_path(&cursors_root, &event_source);
        match poll_source_due(source, &last_polled_file, Utc::now()) {
            Ok(true) => {}
            Ok(false) => {
                debug!(source = %source_ref, "poll source not due yet; skipping");
                continue;
            }
            Err(e) => {
                warn!(source = %source_ref, error = %e, "invalid poll interval; polling anyway");
            }
        }
        let Some(connector) = registry.events_for_source(&event_source) else {
            info!(
                source = %source_ref,
                "no event connector configured for trigger source"
            );
            continue;
        };

        let cursor_file = cursor_path(&cursors_root, &event_source);
        let cursor = read_cursor(&cursor_file).ok();

        let result = match connector
            .poll_events(&event_source, cursor.as_deref(), max)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(source = %source_ref, error = %e, "poll_events failed; will retry next tick");
                continue;
            }
        };

        // Cursor advance happens BEFORE dispatch. A workflow that crashes
        // after cursor-advance won't re-process the same events on the
        // next tick — see spec §8 invariant 2.
        if !dry_run {
            if let Err(e) = write_cursor(&cursor_file, &result.next_cursor) {
                warn!(
                    source = %source_ref,
                    error = %e,
                    "failed to persist event cursor; events may be re-fired on next tick"
                );
            }
            if let Err(e) = write_last_polled_at(&last_polled_file, Utc::now()) {
                warn!(
                    source = %source_ref,
                    error = %e,
                    "failed to persist last-polled timestamp; source may poll early next tick"
                );
            }
        }

        for event in &result.events {
            for wf in &event_workflows {
                let Some(matched_event_id) =
                    matching_event_id(&wf.event, &event.id, &event.payload)
                else {
                    continue;
                };
                let event_payload = build_event_payload(event, &matched_event_id);
                if let Some(filter) = &wf.filter {
                    match evaluate_filter(filter, &event_payload) {
                        Ok(true) => {}
                        Ok(false) => {
                            debug!(
                                workflow = %wf.name,
                                delivery = %event.delivery,
                                "filter excluded event"
                            );
                            continue;
                        }
                        Err(e) => {
                            warn!(
                                workflow = %wf.name,
                                error = %e,
                                "filter evaluation failed; treating as exclude"
                            );
                            continue;
                        }
                    }
                }

                let run_id = format!(
                    "evt-{}-{}-{}",
                    wf.name,
                    source_slug(&event.source),
                    event.delivery
                );

                if dry_run {
                    println!(
                        "would fire: {} (event={}, delivery={}, run_id={})",
                        wf.name, event.id, event.delivery, run_id
                    );
                    continue;
                }

                info!(
                    workflow = %wf.name,
                    event = %event.id,
                    run_id = %run_id,
                    "firing"
                );
                let inputs: Vec<(String, String)> = Vec::new();
                match super::workflow::run_by_name_with_run_id(
                    &wf.name,
                    inputs,
                    None,
                    Some(event_payload),
                    run_id.clone(),
                )
                .await
                {
                    Ok(outcome) => {
                        if let Some(step) = outcome.awaiting_step_id {
                            info!(
                                workflow = %wf.name,
                                run_id = %outcome.run_id,
                                step = %step,
                                "workflow paused at approval gate"
                            );
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("already exists") {
                            // Idempotent re-fire — the event was already
                            // dispatched on a prior tick. Spec §8 invariant 1.
                            debug!(
                                workflow = %wf.name,
                                run_id = %run_id,
                                "event already dispatched; skipping"
                            );
                        } else {
                            warn!(
                                workflow = %wf.name,
                                run_id = %run_id,
                                error = %e,
                                "workflow run failed"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `rupu cron events` — read-only inspection of event-triggered
/// workflows + which sources they cover + most recent cursor.
async fn events(no_color: bool, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();

    let workflows = collect_event_workflows()?;
    let cursors_root = global.join("cron-state").join("event-cursors");

    if workflows.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!(
            "(no event-triggered workflows found)\n\nDrop a workflow YAML under `.rupu/workflows/` \
             with `trigger.on: event` (e.g. `event: github.issue.opened`) and configure \
             `[triggers].poll_sources` in `config.toml`. See `docs/triggers.md` for details."
        );
        return Ok(());
    }
    if cfg.triggers.poll_sources.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!(
            "(workflows configured, but `[triggers].poll_sources` is empty in config.toml — \
             `rupu cron tick` will not poll any sources until you add at least one entry like \
             `github:owner/repo`.)\n"
        );
    }

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None);
    let poll_sources: Vec<String> = cfg
        .triggers
        .poll_sources
        .iter()
        .map(format_poll_source_entry)
        .collect();
    let rows = workflows
        .iter()
        .map(|wf| {
            let cursor = cfg
                .triggers
                .poll_sources
                .iter()
                .filter_map(|s| s.source().parse::<EventSourceRef>().ok())
                .find_map(|source| {
                    let path = cursor_path(&cursors_root, &source);
                    read_cursor(&path).ok()
                });
            CronEventsRow {
                name: wf.name.clone(),
                event: wf.event.clone(),
                sources: poll_sources.clone(),
                cursor,
            }
        })
        .collect();
    let output = CronEventsOutput {
        prefs,
        report: CronEventsReport {
            kind: "cron_events",
            version: 1,
            summary: CronEventsSummary { poll_sources },
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

struct EventWorkflow {
    name: String,
    event: String,
    filter: Option<String>,
}

fn collect_event_workflows() -> anyhow::Result<Vec<EventWorkflow>> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let mut by_name: BTreeMap<String, EventWorkflow> = BTreeMap::new();
    push_event(&global.join("workflows"), &mut by_name);
    if let Some(p) = &project_root {
        push_event(&p.join(".rupu/workflows"), &mut by_name);
    }
    Ok(by_name.into_values().collect())
}

fn push_event(dir: &Path, into: &mut BTreeMap<String, EventWorkflow>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping unreadable workflow");
                continue;
            }
        };
        let wf = match Workflow::parse(&body) {
            Ok(w) => w,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping malformed workflow");
                continue;
            }
        };
        if wf.trigger.on != TriggerKind::Event {
            continue;
        }
        let Some(event) = wf.trigger.event.clone() else {
            warn!(path = %p.display(), "trigger.on=event without event: field; skipping");
            continue;
        };
        into.insert(
            stem.to_string(),
            EventWorkflow {
                name: stem.to_string(),
                event,
                filter: wf.trigger.filter.clone(),
            },
        );
    }
}

fn format_poll_source_entry(source: &PollSourceEntry) -> String {
    match source.poll_interval() {
        Some(interval) => format!("{}@{interval}", source.source()),
        None => source.source().to_string(),
    }
}

fn source_slug(source: &EventSourceRef) -> String {
    let text = match source {
        EventSourceRef::Repo { repo } => format!("repo-{}-{}", repo.owner, repo.repo),
        EventSourceRef::TrackerProject { project, .. } => format!("project-{project}"),
    };
    text.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '-',
        })
        .collect()
}

/// `<global>/cron-state/event-cursors/<vendor>/<source>.cursor`.
fn cursor_path(root: &Path, source: &EventSourceRef) -> PathBuf {
    root.join(source.vendor())
        .join(format!("{}.cursor", source_slug(source)))
}

fn read_cursor(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

fn write_cursor(path: &Path, body: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("cursor.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn last_polled_at_path(root: &Path, source: &EventSourceRef) -> PathBuf {
    root.join(source.vendor())
        .join(format!("{}.last_polled", source_slug(source)))
}

fn read_last_polled_at(path: &Path) -> anyhow::Result<DateTime<Utc>> {
    let body = std::fs::read_to_string(path)?;
    Ok(DateTime::parse_from_rfc3339(body.trim())?.with_timezone(&Utc))
}

fn write_last_polled_at(path: &Path, at: DateTime<Utc>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("last_polled.tmp");
    std::fs::write(&tmp, at.to_rfc3339())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn poll_source_due(
    source: &PollSourceEntry,
    last_polled_path: &Path,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let Some(interval) = source.poll_interval() else {
        return Ok(true);
    };
    let last_polled = match read_last_polled_at(last_polled_path) {
        Ok(at) => at,
        Err(_) => return Ok(true),
    };
    Ok(last_polled + parse_relative_duration(interval)? <= now)
}

fn parse_relative_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let trimmed = value.trim();
    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| anyhow::anyhow!("invalid duration `{value}`"))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid duration `{value}`: {e}"))?;
    let duration = match unit {
        's' => chrono::Duration::seconds(amount),
        'm' => chrono::Duration::minutes(amount),
        'h' => chrono::Duration::hours(amount),
        'd' => chrono::Duration::days(amount),
        _ => anyhow::bail!("invalid duration `{value}`"),
    };
    Ok(duration)
}

/// Build the JSON value bound as `{{event.*}}` in step prompts +
/// `when:` filters. Spec §7.
fn build_event_payload(ev: &rupu_scm::PolledEvent, matched_event_id: &str) -> serde_json::Value {
    let (vendor, repo, source) = match &ev.source {
        EventSourceRef::Repo { repo } => (
            repo.platform.as_str(),
            serde_json::json!({
                "full_name": format!("{}/{}", repo.owner, repo.repo),
                "owner": repo.owner,
                "name": repo.repo,
            }),
            serde_json::json!({
                "kind": "repo",
                "vendor": repo.platform.as_str(),
                "ref": format!("{}:{}/{}", repo.platform.as_str(), repo.owner, repo.repo),
            }),
        ),
        EventSourceRef::TrackerProject { tracker, project } => (
            tracker.as_str(),
            serde_json::json!({}),
            serde_json::json!({
                "kind": "tracker_project",
                "vendor": tracker.as_str(),
                "project": project,
                "ref": format!("{}:{project}", tracker.as_str()),
            }),
        ),
    };
    let mut base = match ev.payload.clone() {
        serde_json::Value::Object(map) => serde_json::Value::Object(map),
        other => serde_json::json!({ "payload": other }),
    };
    let object = base.as_object_mut().expect("object after normalization");
    object.insert("id".into(), serde_json::Value::String(ev.id.clone()));
    object.insert(
        "vendor".into(),
        serde_json::Value::String(vendor.to_string()),
    );
    object.insert(
        "delivery".into(),
        serde_json::Value::String(ev.delivery.clone()),
    );
    object.insert("repo".into(), repo);
    object.insert("source".into(), source);
    object
        .entry("payload")
        .or_insert_with(|| ev.payload.clone());
    annotate_event_payload(&base, &ev.id, matched_event_id)
}

/// Evaluate a `trigger.filter:` expression as a minijinja boolean.
/// The expression has access to `event.*` (and only `event.*`).
/// Returns `Ok(false)` for a clean-render-but-falsy result and `Err`
/// for parse / runtime failures.
fn evaluate_filter(expr: &str, event_payload: &serde_json::Value) -> anyhow::Result<bool> {
    use minijinja::Environment;
    let mut env = Environment::new();
    let template_name = "<trigger.filter>";
    env.add_template(template_name, expr)
        .map_err(|e| anyhow::anyhow!("filter parse: {e}"))?;
    let tmpl = env.get_template(template_name)?;
    let rendered = tmpl
        .render(minijinja::context! { event => event_payload })
        .map_err(|e| anyhow::anyhow!("filter render: {e}"))?;
    match rendered.trim() {
        "true" | "True" | "1" => Ok(true),
        "false" | "False" | "0" | "" => Ok(false),
        other => Err(anyhow::anyhow!(
            "filter must render to a boolean ('true'/'false'); got `{other}`"
        )),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

struct CronWorkflow {
    name: String,
    schedule: String,
}

/// Walk global + project workflows directories, parse each YAML
/// file, and collect every workflow with `trigger.on: cron`. Project
/// entries shadow global by name (same precedence as
/// `rupu workflow list`).
fn collect_cron_workflows() -> anyhow::Result<Vec<CronWorkflow>> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let mut by_name: BTreeMap<String, CronWorkflow> = BTreeMap::new();
    push_cron(&global.join("workflows"), &mut by_name);
    if let Some(p) = &project_root {
        push_cron(&p.join(".rupu/workflows"), &mut by_name);
    }
    Ok(by_name.into_values().collect())
}

fn push_cron(dir: &Path, into: &mut BTreeMap<String, CronWorkflow>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping unreadable workflow");
                continue;
            }
        };
        let wf = match Workflow::parse(&body) {
            Ok(w) => w,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping malformed workflow");
                continue;
            }
        };
        if wf.trigger.on != TriggerKind::Cron {
            continue;
        }
        let Some(schedule) = wf.trigger.cron.clone() else {
            // The schema validator should have caught this, but be
            // defensive — a malformed cron-trigger workflow is just
            // skipped, not fatal to the whole tick.
            warn!(path = %p.display(), "trigger.on=cron without cron: field; skipping");
            continue;
        };
        into.insert(
            stem.to_string(),
            CronWorkflow {
                name: stem.to_string(),
                schedule,
            },
        );
    }
}

fn read_last_fired(path: &Path) -> anyhow::Result<DateTime<Utc>> {
    let s = std::fs::read_to_string(path)?;
    let parsed = DateTime::parse_from_rfc3339(s.trim())?.with_timezone(&Utc);
    Ok(parsed)
}

fn write_last_fired(path: &Path, ts: DateTime<Utc>) -> anyhow::Result<()> {
    let body = ts.to_rfc3339();
    let tmp = path.with_extension("last_fired.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_config::PollSourceSpec;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn last_fired_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("foo.last_fired");
        let ts = Utc::now();
        write_last_fired(&path, ts).unwrap();
        let read = read_last_fired(&path).unwrap();
        // RFC3339 round-trip preserves to-second precision; sub-second
        // can drift. Compare timestamps by truncating to seconds.
        assert_eq!(read.timestamp(), ts.timestamp());
    }

    #[test]
    fn poll_source_due_without_interval_is_always_true() {
        let tmp = TempDir::new().unwrap();
        let entry = PollSourceEntry::Detailed(PollSourceSpec {
            source: "github:Section9Labs/rupu".into(),
            poll_interval: None,
        });
        assert!(poll_source_due(&entry, &tmp.path().join("missing"), Utc::now()).unwrap());
    }

    #[test]
    fn poll_source_due_respects_last_polled_timestamp() {
        let tmp = TempDir::new().unwrap();
        let repo = rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "Section9Labs".into(),
            repo: "rupu".into(),
        };
        let path = last_polled_at_path(tmp.path(), &repo.into());
        write_last_polled_at(&path, Utc::now() - chrono::Duration::minutes(3)).unwrap();
        let entry = PollSourceEntry::Detailed(PollSourceSpec {
            source: "github:Section9Labs/rupu".into(),
            poll_interval: Some("5m".into()),
        });
        assert!(!poll_source_due(&entry, &path, Utc::now()).unwrap());
        write_last_polled_at(&path, Utc::now() - chrono::Duration::minutes(6)).unwrap();
        assert!(poll_source_due(&entry, &path, Utc::now()).unwrap());
    }

    #[test]
    fn build_event_payload_records_matched_alias() {
        let event = rupu_scm::PolledEvent {
            id: "github.issue.labeled".into(),
            delivery: "evt-123".into(),
            source: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            }
            .into(),
            subject: None,
            payload: json!({
                "payload": {
                    "action": "labeled",
                    "issue": { "number": 42 }
                }
            }),
        };
        let payload = build_event_payload(&event, "issue.queue_entered");
        assert_eq!(payload["id"], "issue.queue_entered");
        assert_eq!(payload["canonical_id"], "github.issue.labeled");
        assert_eq!(payload["repo"]["full_name"], "Section9Labs/rupu");
        assert_eq!(payload["payload"]["issue"]["number"], 42);
    }

    #[test]
    fn build_event_payload_for_tracker_source_uses_source_block() {
        let event = rupu_scm::PolledEvent {
            id: "linear.issue.updated".into(),
            delivery: "evt-456".into(),
            source: rupu_scm::EventSourceRef::TrackerProject {
                tracker: rupu_scm::IssueTracker::Linear,
                project: "workspace-123".into(),
            },
            subject: None,
            payload: json!({
                "state": {
                    "before": { "id": "todo" },
                    "after": { "id": "in_progress" }
                }
            }),
        };
        let payload = build_event_payload(&event, "issue.state_changed");
        assert_eq!(payload["id"], "issue.state_changed");
        assert_eq!(payload["canonical_id"], "linear.issue.updated");
        assert_eq!(payload["vendor"], "linear");
        assert_eq!(payload["repo"], json!({}));
        assert_eq!(payload["source"]["project"], "workspace-123");
        assert_eq!(payload["state"]["before"]["id"], "todo");
        assert_eq!(payload["state"]["after"]["id"], "in_progress");
    }
}
