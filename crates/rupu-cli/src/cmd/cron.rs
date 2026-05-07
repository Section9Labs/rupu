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

use crate::paths;
use chrono::{DateTime, Utc};
use clap::Subcommand;
use rupu_orchestrator::cron_schedule::{next_fire_after, parse_schedule, should_fire};
use rupu_orchestrator::event_matches;
use rupu_orchestrator::{TriggerKind, Workflow};
use rupu_scm::{Platform, RepoRef};
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

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List { no_color } => list(no_color).await,
        Action::Tick {
            dry_run,
            skip_events,
            only_events,
        } => tick(dry_run, skip_events, only_events).await,
        Action::Events { no_color } => events(no_color).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e)
    }
}

async fn list(no_color: bool) -> anyhow::Result<()> {
    let workflows = collect_cron_workflows()?;
    if workflows.is_empty() {
        println!(
            "(no cron-triggered workflows found)\n\nAdd `trigger.on: cron` to a workflow under \
             `.rupu/workflows/` and configure a schedule (e.g. `cron: \"0 4 * * *\"`)."
        );
        return Ok(());
    }
    let now = Utc::now();
    let prefs = ui_prefs(no_color)?;

    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "SCHEDULE", "NEXT (UTC)", "IN"]);
    for w in &workflows {
        let next = parse_schedule(&w.schedule)
            .ok()
            .and_then(|s| next_fire_after(&s, now));
        let (next_str, until_cell) = match next {
            Some(t) => {
                let delta = (t - now).num_seconds();
                (
                    t.format("%Y-%m-%d %H:%M:%S").to_string(),
                    crate::output::tables::relative_time_cell(delta, &prefs),
                )
            }
            None => ("<unschedulable>".to_string(), comfy_table::Cell::new("")),
        };
        table.add_row(vec![
            comfy_table::Cell::new(&w.name),
            comfy_table::Cell::new(&w.schedule),
            comfy_table::Cell::new(next_str),
            until_cell,
        ]);
    }
    println!("{table}");
    Ok(())
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
        &cfg.ui, no_color, None, None,
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
        let Some((platform, repo)) = parse_poll_source(source) else {
            warn!(source = %source, "invalid poll_sources entry; expected `<platform>:<owner>/<repo>`");
            continue;
        };
        let Some(connector) = registry.events(platform) else {
            info!(
                source = %source,
                "no event connector for platform — run `rupu auth login --provider {}`",
                platform.as_str()
            );
            continue;
        };

        let cursor_file = cursor_path(&cursors_root, platform, &repo);
        let cursor = read_cursor(&cursor_file).ok();

        let result = match connector.poll_events(&repo, cursor.as_deref(), max).await {
            Ok(r) => r,
            Err(e) => {
                warn!(source = %source, error = %e, "poll_events failed; will retry next tick");
                continue;
            }
        };

        // Cursor advance happens BEFORE dispatch. A workflow that crashes
        // after cursor-advance won't re-process the same events on the
        // next tick — see spec §8 invariant 2.
        if !dry_run {
            if let Err(e) = write_cursor(&cursor_file, &result.next_cursor) {
                warn!(
                    source = %source,
                    error = %e,
                    "failed to persist event cursor; events may be re-fired on next tick"
                );
            }
        }

        for event in &result.events {
            for wf in &event_workflows {
                if !event_matches(&wf.event, &event.id) {
                    continue;
                }
                let event_payload = build_event_payload(event, platform);
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

                let run_id = format!("evt-{}-{}-{}", wf.name, platform.as_str(), event.delivery);

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
async fn events(no_color: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();

    let workflows = collect_event_workflows()?;
    let cursors_root = global.join("cron-state").join("event-cursors");

    if workflows.is_empty() {
        println!(
            "(no event-triggered workflows found)\n\nDrop a workflow YAML under `.rupu/workflows/` \
             with `trigger.on: event` (e.g. `event: github.issue.opened`) and configure \
             `[triggers].poll_sources` in `config.toml`. See `docs/triggers.md` for details."
        );
        return Ok(());
    }
    if cfg.triggers.poll_sources.is_empty() {
        println!(
            "(workflows configured, but `[triggers].poll_sources` is empty in config.toml — \
             `rupu cron tick` will not poll any sources until you add at least one entry like \
             `github:owner/repo`.)\n"
        );
    }

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "EVENT", "SOURCES", "CURSOR"]);
    for wf in &workflows {
        let sources = cfg.triggers.poll_sources.join(",");
        // Best-effort: print the cursor of the *first* configured source.
        let cursor_repr = cfg
            .triggers
            .poll_sources
            .iter()
            .filter_map(|s| parse_poll_source(s))
            .find_map(|(platform, repo)| {
                let path = cursor_path(&cursors_root, platform, &repo);
                read_cursor(&path).ok()
            })
            .unwrap_or_else(|| "(none)".into());
        let event_cell = comfy_table::Cell::new(&wf.event)
            .fg(crate::output::tables::status_color("running", &prefs)
                .unwrap_or(comfy_table::Color::Reset));
        // The "running" color (blue) is reused for event-id cells so
        // the column is visually anchored without inventing a new
        // semantic bucket. Falls back to default when colors are off.
        table.add_row(vec![
            comfy_table::Cell::new(&wf.name),
            event_cell,
            comfy_table::Cell::new(if sources.is_empty() {
                "(none configured)".into()
            } else {
                sources
            }),
            comfy_table::Cell::new(truncate(&cursor_repr, 60)),
        ]);
    }
    println!("{table}");
    Ok(())
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

fn parse_poll_source(s: &str) -> Option<(Platform, RepoRef)> {
    let (platform_str, rest) = s.split_once(':')?;
    let (owner, repo) = rest.split_once('/')?;
    let platform = match platform_str {
        "github" => Platform::Github,
        "gitlab" => Platform::Gitlab,
        _ => return None,
    };
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((
        platform,
        RepoRef {
            platform,
            owner: owner.into(),
            repo: repo.into(),
        },
    ))
}

/// `<global>/cron-state/event-cursors/<vendor>/<owner>--<repo>.cursor`.
/// `--` separator avoids ambiguity if either name contains `/`.
fn cursor_path(root: &Path, platform: Platform, repo: &RepoRef) -> PathBuf {
    root.join(platform.as_str())
        .join(format!("{}--{}.cursor", repo.owner, repo.repo))
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

/// Build the JSON value bound as `{{event.*}}` in step prompts +
/// `when:` filters. Spec §7.
fn build_event_payload(ev: &rupu_scm::PolledEvent, platform: Platform) -> serde_json::Value {
    let owner = &ev.repo.owner;
    let name = &ev.repo.repo;
    serde_json::json!({
        "id": ev.id,
        "vendor": platform.as_str(),
        "delivery": ev.delivery,
        "repo": {
            "full_name": format!("{owner}/{name}"),
            "owner": owner,
            "name": name,
        },
        "payload": ev.payload,
    })
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
}
