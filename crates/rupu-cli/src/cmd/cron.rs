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
    /// Read-only inspection of event-triggered workflows.
    ///
    /// Prints each workflow's name, target event id, sources from
    /// `[triggers].poll_sources`, and the most recent persisted
    /// cursor across those sources.
    Events {
        /// Disable colored output.
        #[arg(long)]
        no_color: bool,
    },
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::List { no_color } => list(no_color, global_format, absolute, all_columns).await,
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
        println!(
            "{}",
            render_cron_list_table(&self.report.rows, &self.prefs, Utc::now())
        );
        Ok(())
    }
}

/// Build the `cron list` human table: workflow names never wrap, a
/// populated `NEXT (UTC)` renders the absolute wall-clock instant while
/// `IN` renders the countdown to it, and a bare `N workflows` summary (no
/// `STATUS` header here, so no breakdown). Extracted from `render_table`
/// so it can be asserted directly against its returned string. `now` is
/// a parameter so tests are deterministic; `render_table` passes
/// `Utc::now()`.
///
/// `next_utc` / `in_seconds` are computed together (`list()`, below) and
/// are both `None` in exactly one case: the schedule failed to parse, or
/// has no future occurrence. That is NOT "this entity has no such
/// dimension" — every cron workflow has a schedule — it is "we could not
/// compute it," which is precisely the failure state an operator runs
/// `cron list` to discover. `CellValue::Missing` is suppression-eligible:
/// mapping both columns to it would mean a listing where *every*
/// schedule is broken drops both columns, hiding the exact failure the
/// command exists to surface. So `None` renders as the stable
/// `CellValue::Text("unschedulable")` on both columns instead — never
/// empty, never `"—"`, so `is_empty()` never suppresses it even when
/// every row shares it.
fn render_cron_list_table(
    rows: &[CronListRow],
    prefs: &crate::cmd::ui::UiPrefs,
    now: DateTime<Utc>,
) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        prefs.render_opts(),
        vec!["NAME", "SCHEDULE", "NEXT (UTC)", "IN"],
    )
    .with_summary("workflow");

    for row in rows {
        // `next_utc` is written (`list()`) as
        // `time.format("%Y-%m-%d %H:%M:%S")` — the same naive format
        // `render_transcript_list_table` parses (transcript.rs). Try
        // that exact format first (the source `DateTime<Utc>` was UTC
        // before stringifying, so interpreting the naive result as UTC
        // is correct, not a guess), fall back to RFC3339 for any other
        // writer of this field, and finally to the verbatim text so a
        // malformed value never drops the row.
        //
        // Deliberately NOT `CellValue::Timestamp`, and deliberately
        // ALWAYS absolute, not toggled by `--absolute`: this column is
        // explicitly labelled "(UTC)" — it answers "when exactly does
        // this fire," a wall-clock question. `IN` (below) is the
        // complementary countdown answering "how long until." An
        // earlier version of this function routed `NEXT (UTC)` through
        // `fmt::relative_time` (via `CellValue::Timestamp`, or by hand
        // via `tables::format_seconds`), which made it byte-identical to
        // `IN` under the default view and, worse, made `fmt::relative_time`
        // clamp every row to "just now" — that renderer assumes a past
        // instant (correct for started_at/updated_at, wrong for a value
        // that is *always* future). Both problems went away by making
        // `NEXT (UTC)` unconditionally absolute: `--absolute` now has
        // nothing left to toggle on this table (both columns already
        // show their one true representation), which is fine and
        // honest, not a bug — the flag still matters for every other
        // entity-table command.
        let next_cell = match &row.next_utc {
            Some(text) => chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .map(|naive| naive.and_utc())
                .or_else(|_| DateTime::parse_from_rfc3339(text).map(|ts| ts.with_timezone(&Utc)))
                .map(|ts| CellValue::Text(ts.to_rfc3339()))
                .unwrap_or_else(|_| CellValue::Text(text.clone())),
            None => CellValue::Text("unschedulable".to_string()),
        };
        // `IN` is the countdown companion to `NEXT (UTC)`'s wall-clock
        // time — always the signed relative delta, never absolute.
        let in_cell = match row.in_seconds {
            Some(seconds) => CellValue::Text(crate::output::tables::format_seconds(seconds)),
            None => CellValue::Text("unschedulable".to_string()),
        };
        table = table.row(vec![
            CellValue::Name(row.name.clone()),
            CellValue::Text(row.schedule.clone()),
            next_cell,
            in_cell,
        ]);
    }
    table.render(now)
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

async fn list(
    no_color: bool,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> anyhow::Result<()> {
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
    let prefs = ui_prefs(no_color, absolute, all_columns)?;
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

fn ui_prefs(
    no_color: bool,
    absolute: bool,
    all_columns: bool,
) -> anyhow::Result<crate::cmd::ui::UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    // UI prefs only — lock does not apply (I-7)
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();
    Ok(
        crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None)
            .with_table_flags(absolute, all_columns),
    )
}

/// The `rupu cron tick` core: fires due cron-scheduled workflows, then
/// polls event-triggered workflows. `pub(crate)` so `rupu cp serve`'s
/// cron-tick background loop (`crate::cmd::cp`) can call the exact same
/// entrypoint on a timer instead of reimplementing the tick logic.
pub(crate) async fn tick(
    dry_run: bool,
    skip_events: bool,
    only_events: bool,
) -> anyhow::Result<()> {
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
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;

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
    let registry = Arc::new(
        rupu_scm::Registry::discover(&resolver, &cfg, Arc::new(rupu_netflow::NullSink)).await,
    );

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
        let event_source = apply_account_override(event_source, source);
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
        // `cwd: None` — a cron poll has no filesystem context to key a
        // path rule on any more than a webhook payload does (spec §6.3:
        // "a webhook payload or cron poll knows the owner but has no
        // cwd"); only the explicit/owner/sole-account tiers can fire.
        let (account, connector) = match registry.events_for_source(&event_source, None) {
            Ok(pair) => pair,
            Err(rupu_scm::AccountError::NoAccounts { .. }) => {
                info!(
                    source = %source_ref,
                    "no event connector configured for trigger source"
                );
                continue;
            }
            Err(e) => {
                warn!(source = %source_ref, error = %e, "account resolution failed for trigger source; skipping");
                continue;
            }
        };
        debug!(source = %source_ref, account = %account.as_str(), "resolved account for trigger source");

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
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg), project_cfg.as_deref())
        .unwrap_or_default();

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

/// Splice a `PollSourceEntry`'s explicit `account = "..."` override
/// (Task 6, spec §6.5) into a freshly-parsed `EventSourceRef`. The
/// compact `source` string (`"linear:team-123"`) has nowhere to encode
/// an account, so this is the only way one reaches
/// `Registry::events_for_source`'s `explicit` tier — a `Repo`-sourced
/// trigger has no such field and passes through unchanged (it infers
/// its account from `repo.owner` via an owner rule instead).
///
/// Extracted as a pure, dependency-free step — mirrors
/// `rupu-scm/src/registry.rs`'s `resolve_configured_default` reasoning
/// — so it's unit-testable without a `Registry`/`rupu cron tick`
/// integration harness. Shared with `cmd/autoflow.rs`'s
/// `enqueue_polled_wakes`, which polls the same `poll_sources` config
/// through the same account-resolution surface for autoflow wake-ups.
pub(crate) fn apply_account_override(
    mut event_source: EventSourceRef,
    entry: &PollSourceEntry,
) -> EventSourceRef {
    if let EventSourceRef::TrackerProject { account, .. } = &mut event_source {
        *account = entry.account().map(rupu_scm::AccountId::new);
    }
    event_source
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
        EventSourceRef::TrackerProject {
            tracker, project, ..
        } => (
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
        // An autoflow the operator disabled must not fire on the cron tick.
        //
        // The CP web UI's disable toggle (`api::autoflows::set_autoflow_enabled`)
        // writes `autoflow.enabled: false` into this YAML, and the autoflow
        // ENGINE honors it (`cmd/autoflow.rs`'s `.filter(|a| a.enabled)`). The
        // cron tick is a SECOND, independent dispatch subsystem, and it used to
        // select purely on `trigger.on: cron` — so a cron-triggered workflow
        // showing as "disabled" in the UI kept firing on schedule. Observed
        // live on `nightly-health`.
        //
        // Deliberately keyed on an EXPLICIT autoflow block: a workflow that
        // never opted into autoflow has no such flag to consult and must keep
        // firing, or this would silently break every ordinary cron job.
        if wf.autoflow.as_ref().is_some_and(|a| !a.enabled) {
            info!(
                workflow = %stem,
                "skipping: autoflow is disabled (autoflow.enabled: false)"
            );
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

    /// Smoke test for T6 (dogfood-autoflows): `cp serve`'s cron-tick
    /// background loop calls this exact `tick()` fn on a timer instead of
    /// reimplementing the tick logic — this just proves the factored-out
    /// core is callable end-to-end (dry-run, events skipped so no network
    /// connector is built) and returns `Ok`.
    #[tokio::test]
    async fn tick_core_is_callable_dry_run() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();
        let tmp = TempDir::new().unwrap();
        let global = tmp.path().join("home");
        let old_home = std::env::var_os("RUPU_HOME");
        std::env::set_var("RUPU_HOME", &global);

        let result = tick(/* dry_run */ true, /* skip_events */ true, false).await;

        match old_home {
            Some(value) => std::env::set_var("RUPU_HOME", value),
            None => std::env::remove_var("RUPU_HOME"),
        }
        assert!(
            result.is_ok(),
            "cron tick core should be callable: {result:?}"
        );
    }

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
    fn apply_account_override_splices_tracker_project_account() {
        let entry = PollSourceEntry::Detailed(PollSourceSpec {
            source: "linear:team-123".into(),
            poll_interval: None,
            account: Some("linear-work".into()),
        });
        let parsed: EventSourceRef = entry.source().parse().unwrap();
        let spliced = apply_account_override(parsed, &entry);
        match spliced {
            EventSourceRef::TrackerProject { account, .. } => {
                assert_eq!(account, Some(rupu_scm::AccountId::new("linear-work")));
            }
            EventSourceRef::Repo { .. } => panic!("expected TrackerProject"),
        }
    }

    #[test]
    fn apply_account_override_is_noop_for_repo_sources() {
        // `Repo` has no `account` field to splice into — it infers its
        // account from `repo.owner` via an owner rule instead.
        let entry = PollSourceEntry::Source("github:acme/api".into());
        let parsed: EventSourceRef = entry.source().parse().unwrap();
        let spliced = apply_account_override(parsed, &entry);
        assert!(matches!(spliced, EventSourceRef::Repo { .. }));
    }

    #[test]
    fn apply_account_override_clears_to_none_when_entry_has_no_account() {
        let entry = PollSourceEntry::Detailed(PollSourceSpec {
            source: "linear:team-123".into(),
            poll_interval: None,
            account: None,
        });
        let parsed: EventSourceRef = entry.source().parse().unwrap();
        let spliced = apply_account_override(parsed, &entry);
        match spliced {
            EventSourceRef::TrackerProject { account, .. } => assert_eq!(account, None),
            EventSourceRef::Repo { .. } => panic!("expected TrackerProject"),
        }
    }

    #[test]
    fn poll_source_due_without_interval_is_always_true() {
        let tmp = TempDir::new().unwrap();
        let entry = PollSourceEntry::Detailed(PollSourceSpec {
            source: "github:Section9Labs/rupu".into(),
            poll_interval: None,
            account: None,
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
            account: None,
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
                account: None,
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

    // ── Disabled autoflows must not fire on the cron tick ────────────
    //
    // The CP web UI's autoflow disable toggle writes `autoflow.enabled:
    // false` into the workflow YAML, and the autoflow ENGINE honors it
    // (`cmd/autoflow.rs`'s `.filter(|a| a.enabled)`). But the cron tick is a
    // second, independent dispatch subsystem that selected purely on
    // `trigger.on: cron` — so a cron-triggered workflow the operator had
    // disabled in the UI kept firing on schedule, with the UI showing it as
    // off. Observed live: `nightly-health` carried `autoflow.enabled: false`
    // and still fired, writing `cron-state/nightly-health.last_fired`.

    fn tmp_wf_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        for (name, body) in files {
            std::fs::write(d.path().join(name), body).expect("write workflow");
        }
        d
    }

    const CRON_DISABLED: &str = r#"
name: nightly-health
autoflow:
  enabled: false
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    const CRON_ENABLED: &str = r#"
name: nightly-enabled
autoflow:
  enabled: true
  entity: issue
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    const CRON_PLAIN: &str = r#"
name: plain-cron
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    #[test]
    fn cron_tick_skips_a_workflow_disabled_in_the_cp_ui() {
        let d = tmp_wf_dir(&[("nightly-health.yaml", CRON_DISABLED)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            !got.contains_key("nightly-health"),
            "a workflow with `autoflow.enabled: false` must not be collected \
             for the cron tick — the CP disable toggle writes exactly that flag"
        );
    }

    #[test]
    fn cron_tick_still_fires_a_plain_cron_workflow_with_no_autoflow_block() {
        // Guard against over-correcting: a workflow that never opted into
        // autoflow at all has no `enabled` flag to consult and must keep
        // firing. Disabling would silently break every ordinary cron job.
        let d = tmp_wf_dir(&[("plain-cron.yaml", CRON_PLAIN)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            got.contains_key("plain-cron"),
            "a cron workflow with no `autoflow:` block must still fire"
        );
    }

    #[test]
    fn cron_tick_still_fires_an_enabled_autoflow() {
        let d = tmp_wf_dir(&[("nightly-enabled.yaml", CRON_ENABLED)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            got.contains_key("nightly-enabled"),
            "`autoflow.enabled: true` must still fire on schedule"
        );
    }

    fn cron_list_test_now() -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 7, 30, 17, 0, 0).unwrap()
    }

    fn cron_list_test_prefs() -> crate::cmd::ui::UiPrefs {
        let cfg = rupu_config::UiConfig::default();
        crate::cmd::ui::UiPrefs::resolve(&cfg, true, None, None, None)
    }

    fn cron_row(
        name: &str,
        schedule: &str,
        next_utc: Option<&str>,
        in_seconds: Option<i64>,
    ) -> CronListRow {
        CronListRow {
            name: name.to_string(),
            schedule: schedule.to_string(),
            next_utc: next_utc.map(str::to_string),
            in_seconds,
        }
    }

    #[test]
    fn cron_list_table_name_never_wraps() {
        // NAME maps to `CellValue::Name`: `workflow show`/`run` accept it
        // back, so like `Id` it must never wrap across lines.
        let rows = vec![cron_row(
            "nightly-maintainability-security",
            "0 7 * * *",
            Some("2026-07-30 13:00:00"),
            Some(3600),
        )];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(
            out.contains("nightly-maintainability-security"),
            "got: {out}"
        );
    }

    #[test]
    fn cron_list_table_next_utc_renders_absolute_for_the_real_stored_format() {
        // `CronListRow.next_utc` is written (see `list()`) as
        // `time.format("%Y-%m-%d %H:%M:%S")` — not RFC3339. The real
        // stored format must recover the instant and render it as an
        // absolute wall-clock instant, not fall through to the
        // literal-text branch. `NEXT (UTC)` is always absolute — it's
        // the column that answers "when exactly," complementary to
        // `IN`'s countdown — so this is not conditioned on `--absolute`.
        let rows = vec![cron_row(
            "nightly-health",
            "0 6 * * *",
            Some("2026-07-30 13:00:00"),
            Some(3600),
        )];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(out.contains("2026-07-30T13:00:00"), "got: {out}");
        assert!(
            !out.contains("2026-07-30 13:00:00"),
            "literal naive text leaked instead of a reformatted absolute instant: {out}"
        );
    }

    #[test]
    fn cron_list_table_next_utc_in_the_future_never_renders_just_now() {
        // The real production shape: `next_utc` is always in the
        // *future* relative to `now` (it's the schedule's next fire
        // time). Routing it through `CellValue::Timestamp`/
        // `fmt::relative_time` (as an earlier version of this function
        // did) clamps every future instant to "just now" — a real bug
        // caught by running the actual binary against this repo's own
        // nightly workflows (5h/6h out). Rendering `NEXT (UTC)` as an
        // unconditional absolute instant sidesteps that renderer
        // entirely, so this guards against it coming back.
        let rows = vec![cron_row(
            "nightly-health",
            "0 6 * * *",
            Some("2026-07-30 21:00:00"),
            Some(4 * 3600),
        )];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(out.contains("2026-07-30T21:00:00"), "got: {out}");
        assert!(
            !out.contains("just now"),
            "future next_utc must not render as \"just now\": {out}"
        );
    }

    #[test]
    fn cron_list_table_next_utc_and_in_carry_different_information() {
        // NEXT (UTC) and IN exist to answer different questions —
        // "when exactly" vs. "how long until" — and must not collapse
        // to the same text. A prior version of this function rendered
        // both as the same relative countdown ("in 4h" / "in 4h"),
        // which silently lost the wall-clock answer NEXT (UTC) is
        // labelled to give.
        let rows = vec![cron_row(
            "nightly-health",
            "0 6 * * *",
            Some("2026-07-30 21:00:00"),
            Some(4 * 3600),
        )];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        // IN carries the countdown...
        assert!(out.contains("in 4h"), "got: {out}");
        // ...exactly once — not duplicated into NEXT (UTC) too.
        assert_eq!(
            out.matches("in 4h").count(),
            1,
            "NEXT (UTC) must not also render the countdown: {out}"
        );
        // NEXT (UTC) carries the absolute instant instead.
        assert!(out.contains("2026-07-30T21:00:00"), "got: {out}");
    }

    #[test]
    fn cron_list_table_absolute_flag_has_no_effect() {
        // `NEXT (UTC)` is unconditionally absolute and `IN` is
        // unconditionally the countdown — neither column has anything
        // left to toggle on this table, unlike `transcript list`/
        // `session list`'s `CellValue::Timestamp` columns. That's fine
        // and honest, not a missed wiring: assert `--absolute` is a
        // true no-op here rather than inventing a toggle to justify the
        // flag existing on this command.
        let rows = vec![
            cron_row(
                "nightly-health",
                "0 6 * * *",
                Some("2026-07-30 13:00:00"),
                Some(3600),
            ),
            cron_row("broken-cron", "not a schedule", None, None),
        ];
        let now = cron_list_test_now();
        let default_out = render_cron_list_table(&rows, &cron_list_test_prefs(), now);
        let absolute_prefs = cron_list_test_prefs().with_table_flags(true, false);
        let absolute_out = render_cron_list_table(&rows, &absolute_prefs, now);
        assert_eq!(
            default_out, absolute_out,
            "--absolute should be a no-op on this table"
        );
    }

    #[test]
    fn cron_list_table_survives_an_unparseable_next_utc() {
        let rows = vec![cron_row(
            "nightly-health",
            "0 6 * * *",
            Some("not-a-timestamp"),
            Some(3600),
        )];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(out.contains("not-a-timestamp"), "row was dropped: {out}");
    }

    #[test]
    fn cron_list_table_unschedulable_row_still_shows_both_columns() {
        // A single broken-schedule row mixed in with a healthy one: NEXT
        // (UTC) and IN must both still render a stable, non-empty label
        // for the broken row rather than an empty cell.
        let rows = vec![
            cron_row(
                "nightly-health",
                "0 6 * * *",
                Some("2026-07-30 13:00:00"),
                Some(3600),
            ),
            cron_row("broken-cron", "not a schedule", None, None),
        ];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(out.contains("broken-cron"), "got: {out}");
        assert!(out.contains("unschedulable"), "got: {out}");
        // Both NEXT (UTC) and IN carry the label for the broken row — not
        // just one of the two columns.
        assert_eq!(
            out.matches("unschedulable").count(),
            2,
            "expected both NEXT (UTC) and IN to show the label: {out}"
        );
    }

    #[test]
    fn cron_list_table_every_row_unschedulable_still_shows_both_columns() {
        // The guard for this task's whole point: `next_utc` and
        // `in_seconds` are both `None` together when a schedule can't be
        // computed — not "this entity has no such dimension" (every cron
        // workflow has a schedule). If both mapped to `CellValue::Missing`,
        // a listing where *every* row is broken would suppress both
        // columns entirely, hiding the exact failure `cron list` exists to
        // surface. With every row sharing the same non-empty
        // `CellValue::Text("unschedulable")`, `retained_columns` must NOT
        // drop NEXT (UTC) or IN.
        let rows = vec![
            cron_row("broken-one", "not a schedule", None, None),
            cron_row("broken-two", "also not a schedule", None, None),
        ];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(
            out.contains("NEXT (UTC)"),
            "NEXT (UTC) header dropped: {out}"
        );
        assert!(out.contains("IN"), "IN header dropped: {out}");
        assert_eq!(
            out.matches("unschedulable").count(),
            4,
            "expected all 4 cells (2 rows × 2 columns) to show the label: {out}"
        );
    }

    #[test]
    fn cron_list_table_summary_is_a_bare_count() {
        // No `STATUS` header on this table, so `.with_summary("workflow")`
        // must not attempt a breakdown.
        let rows = vec![
            cron_row(
                "nightly-health",
                "0 6 * * *",
                Some("2026-07-30 13:00:00"),
                Some(3600),
            ),
            cron_row("broken-cron", "not a schedule", None, None),
        ];
        let out = render_cron_list_table(&rows, &cron_list_test_prefs(), cron_list_test_now());
        assert!(out.starts_with("2 workflows\n\n"), "got: {out}");
    }
}
