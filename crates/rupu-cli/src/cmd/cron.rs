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
use rupu_orchestrator::{TriggerKind, Workflow};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use tracing::{info, warn};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List every cron-triggered workflow + its schedule + next-fire
    /// time. Read-only; doesn't update state.
    List,
    /// Walk all workflows, fire any whose schedule matches between
    /// the persisted `last_fired` and now. Designed to run from
    /// system cron at 1-minute granularity.
    Tick {
        /// Don't actually run workflows or update state; just print
        /// what would fire. Useful for verifying a `crontab` line.
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Tick { dry_run } => tick(dry_run).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu cron: {e}");
            ExitCode::from(1)
        }
    }
}

async fn list() -> anyhow::Result<()> {
    let workflows = collect_cron_workflows()?;
    println!("{:<28} {:<24} NEXT (UTC)", "NAME", "SCHEDULE");
    let now = Utc::now();
    for w in &workflows {
        let next = parse_schedule(&w.schedule)
            .ok()
            .and_then(|s| next_fire_after(&s, now));
        let next_str = next
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "<unschedulable>".to_string());
        println!("{:<28} {:<24} {}", w.name, w.schedule, next_str);
    }
    Ok(())
}

async fn tick(dry_run: bool) -> anyhow::Result<()> {
    let workflows = collect_cron_workflows()?;
    if workflows.is_empty() {
        info!("no cron-triggered workflows found");
        return Ok(());
    }

    let global = paths::global_dir()?;
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
        if let Err(e) = super::workflow::run_by_name(&w.name, inputs, None).await {
            warn!(workflow = %w.name, error = %e, "workflow run failed");
        }
    }
    Ok(())
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
