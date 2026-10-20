//! `rupu netflow prune` — the retention story for per-run netflow
//! ledgers.
//!
//! A preceding plan moved netflow capture from one unbounded
//! `flows.jsonl` to one ledger file per run (`<netflow_dir>/<run_id>
//! .jsonl` — see `rupu_netflow::NetflowPaths::for_run`), which fixed
//! cross-run misattribution but traded one unbounded file for an
//! unbounded NUMBER of files. Nothing deletes them on its own; this
//! module is what gives ledgers the same bounded-retention story
//! transcripts already have via `rupu transcript prune`.
//!
//! This is a DESTRUCTIVE command — it deletes an operator's audit
//! trail — so it mirrors `transcript prune`'s shape deliberately
//! (`--older-than`, `--dry-run`) rather than inventing a new feel, and
//! [`prune_ledgers`] documents the safety reasoning behind every
//! filename it accepts or rejects and how it treats a run that might
//! still be live.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use rupu_cp::netflow_prune::{prune_netflow, NetflowPrune, NetflowPruneReport, MIN_LEDGER_AGE};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Print one run's netflow flow records.
    Show {
        /// Full run id (`run_<ULID>`) whose netflow ledger to print.
        run_id: String,
    },
    /// Delete per-run netflow ledgers older than a cutoff.
    Prune(PruneArgs),
}

#[derive(ClapArgs, Debug)]
pub struct PruneArgs {
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`. Must be positive —
    /// `0s` or a negative value is rejected, not treated as "everything".
    /// Defaults to `30d` (mirrors `transcript prune`'s own fallback) when
    /// omitted. A ledger is matched by file mtime, but a ledger whose run
    /// is still live is never deleted however old it is: a workflow run
    /// that is not finished (and whose recorded runner pid, if any, is
    /// alive) protects its own, its steps', its fan-out units' and its
    /// sub-agents' ledgers, and a standalone `rupu run` whose process is
    /// alive protects its own and its sub-agents'. Such ledgers are
    /// listed as `skipped_live`. Anything modified in roughly the last
    /// hour is never eligible regardless of what you pass here.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files. Always cheap to run
    /// first — the default (no flag) deletes immediately, same as
    /// `transcript prune`.
    #[arg(long)]
    pub dry_run: bool,
}

/// Read one run's netflow flow records: the run-scoped ledger(s) merged
/// with the run's own transcript, unbounded in time.
///
/// Delegates to [`rupu_cp::api::netflow::run_scoped_flows_and_dropped`] —
/// the SAME merge the CP performs for a local run — so a run read over ssh
/// and the same run read locally cannot disagree. Returns the flows plus
/// the ledger's own dropped-record count, which the caller reports
/// alongside them (a dropped record is data the sink could not write; the
/// count is the only trace it existed).
///
/// A run with no ledger yields an empty list, not an error: a run that made
/// no outbound calls is a legitimate empty result, indistinguishable on
/// disk from one whose ledger was pruned.
fn collect_run_ledger(
    global: &Path,
    run_id: &str,
) -> anyhow::Result<(Vec<rupu_netflow::FlowRecord>, u64)> {
    // Resolve the run the way the CP does — global store first, then each
    // registered project's own `.rupu/runs`. Which store holds the record
    // matters twice over: it supplies the workspace path a project-local
    // ledger would live under, AND it is what `run_and_unit_ids` reads
    // `step_results.jsonl` from, so resolving to the wrong store silently
    // drops every dispatched step's and fan-out item's ledger.
    let (store, workspace) = resolve_run_store(global, run_id);
    Ok(rupu_cp::api::netflow::run_scoped_flows_and_dropped(
        &rupu_cp::netflow_index::DirectReader,
        &store,
        run_id,
        &workspace,
        global,
        &rupu_netflow::ledger::TimeRange::unbounded(),
    ))
}

/// The store holding `run_id`'s record, and the workspace its ledgers would
/// live under.
///
/// Falls back to the global store rooted at `global` when no store has the
/// record: the global ledger directory is still readable, so an unresolvable
/// record degrades what we can attribute rather than failing the read.
fn resolve_run_store(global: &Path, run_id: &str) -> (rupu_orchestrator::RunStore, PathBuf) {
    let global_store = rupu_orchestrator::RunStore::new(global.join("runs"));
    if let Ok(record) = global_store.load(run_id) {
        return (global_store, record.workspace_path);
    }

    let workspaces = (rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    })
    .list()
    .unwrap_or_default();
    for ws in workspaces {
        let path = PathBuf::from(&ws.path);
        let store = rupu_orchestrator::RunStore::new(path.join(".rupu").join("runs"));
        if store.load(run_id).is_ok() {
            return (store, path);
        }
    }

    (global_store, global.to_path_buf())
}

#[derive(Serialize)]
struct NetflowShowReport {
    kind: &'static str,
    version: u8,
    /// Ledger records the sink could not write. Reported so a coordinator
    /// reading this over ssh can surface the same "data was lost" signal a
    /// local read gets — never silently folded into the flow count.
    dropped_total: u64,
    rows: Vec<rupu_netflow::FlowRecord>,
}

struct NetflowShowOutput {
    report: NetflowShowReport,
}

impl CollectionOutput for NetflowShowOutput {
    type JsonReport = NetflowShowReport;
    type CsvRow = ();

    fn command_name(&self) -> &'static str {
        "netflow show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &[]
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["TS", "METHOD", "HOST", "PATH", "STATUS"]);
        for f in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(f.ts.to_rfc3339()),
                comfy_table::Cell::new(&f.method),
                comfy_table::Cell::new(&f.host),
                comfy_table::Cell::new(&f.path),
                comfy_table::Cell::new(
                    f.status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ]);
        }
        println!("{table}");
        if self.report.dropped_total > 0 {
            println!(
                "({} record(s) dropped by the sink and not recoverable)",
                self.report.dropped_total
            );
        }
        Ok(())
    }
}

fn show(run_id: &str, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (rows, dropped_total) = collect_run_ledger(&global, run_id)?;
    let output = NetflowShowOutput {
        report: NetflowShowReport {
            kind: "netflow_show",
            version: 1,
            dropped_total,
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::Show { run_id } => show(&run_id, global_format),
        Action::Prune(args) => prune(args, global_format, absolute, all_columns).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Show { .. } => ("netflow show", report::TABLE_JSON),
        Action::Prune(_) => ("netflow prune", report::TABLE_JSON_CSV),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct NetflowPruneCsvRow {
    run_id: String,
    scope: String,
    bytes: u64,
    modified_at: String,
    status: String,
}

struct NetflowPruneOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: NetflowPruneReport,
    csv_rows: Vec<NetflowPruneCsvRow>,
}

impl CollectionOutput for NetflowPruneOutput {
    type JsonReport = NetflowPruneReport;
    type CsvRow = NetflowPruneCsvRow;

    fn command_name(&self) -> &'static str {
        "netflow prune"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["run_id", "scope", "bytes", "modified_at", "status"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        use crate::output::entity_table::{CellValue, EntityTable};

        let mut table = EntityTable::new(
            &self.prefs,
            self.prefs.render_opts(),
            vec!["RUN ID", "SCOPE", "BYTES", "MODIFIED", "STATUS"],
        )
        .with_summary("ledger");

        for row in &self.report.rows {
            let modified = chrono::DateTime::parse_from_rfc3339(&row.modified_at)
                .map(|ts| CellValue::Timestamp(ts.with_timezone(&chrono::Utc)))
                .unwrap_or_else(|_| CellValue::Text(row.modified_at.clone()));
            table = table.row(vec![
                CellValue::Id(row.run_id.clone()),
                CellValue::Status(row.scope.clone()),
                CellValue::Text(row.bytes.to_string()),
                modified,
                CellValue::Status(row.status.clone()),
            ]);
        }

        println!("{}", table.render(chrono::Utc::now()));

        let (reclaimed, would_reclaim): (u64, u64) =
            self.report
                .rows
                .iter()
                .fold((0u64, 0u64), |(reclaimed, would), row| {
                    match row.status.as_str() {
                        "deleted" => (reclaimed + row.bytes, would),
                        "would_delete" => (reclaimed, would + row.bytes),
                        _ => (reclaimed, would),
                    }
                });
        if reclaimed > 0 {
            println!("reclaimed {reclaimed} byte(s)");
        }
        if would_reclaim > 0 {
            println!("would reclaim {would_reclaim} byte(s)");
        }
        println!(
            "ledgers newer than the cutoff, anything modified in roughly the \
             last {} hour(s) regardless of the cutoff, and anything that is \
             not a per-run ledger file (the directory's own `.gitignore`, a \
             legacy `flows.jsonl`), are left untouched.",
            MIN_LEDGER_AGE.num_hours(),
        );
        Ok(())
    }
}

/// `netflow prune` has no `--no-color` flag of its own — resolve UI
/// prefs the same way `transcript prune`'s `prune_ui_prefs` does for
/// its own flagless commands: config + env only, `no_color` hardcoded
/// `false` so `NO_COLOR` / `[ui].color = "never"` still work.
fn prune_ui_prefs() -> anyhow::Result<crate::cmd::ui::UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = paths::load_config_for_display(&global, project_root.as_deref(), &pwd, false);
    Ok(crate::cmd::ui::UiPrefs::resolve(
        &cfg.ui, false, None, None, None,
    ))
}

async fn prune(
    args: PruneArgs,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let NetflowPrune { report, problems } = prune_netflow(
        &global,
        project_root.as_deref(),
        args.older_than.as_deref(),
        args.dry_run,
    )?;
    let csv_rows = report
        .rows
        .iter()
        .map(|row| NetflowPruneCsvRow {
            run_id: row.run_id.clone(),
            scope: row.scope.clone(),
            bytes: row.bytes,
            modified_at: row.modified_at.clone(),
            status: row.status.clone(),
        })
        .collect();

    let prefs = prune_ui_prefs()?.with_table_flags(absolute, all_columns);
    let output = NetflowPruneOutput {
        prefs,
        report,
        csv_rows,
    };
    report::emit_collection(global_format, &output)?;

    // Partial failure must not be silent: the report above already
    // named every file that could not be removed (STATUS
    // `failed: <reason>`) AND every root that could not even be
    // scanned, but a non-zero exit is what actually tells a script —
    // or an operator glancing at `$?` — that this run did NOT fully
    // succeed. Every candidate that COULD be removed already was; this
    // only affects the exit code, never a rollback. The report is
    // ALWAYS printed above before this returns, in whatever `--format`
    // was requested — never only on the success path.
    if !problems.is_empty() {
        anyhow::bail!(problems.join("; "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write one flow record into the run's global ledger and return the
    /// record, so a test can assert on the exact value that round-tripped.
    fn seed_ledger(global: &Path, run_id: &str) -> rupu_netflow::FlowRecord {
        let dir = global.join("netflow");
        std::fs::create_dir_all(&dir).unwrap();
        let flow: rupu_netflow::FlowRecord = serde_json::from_value(serde_json::json!({
            "id": "01M1JE8KVH4KYHM9D6MXM4ZNDY",
            "ts": "2026-09-01T00:00:00Z",
            "ctx": { "origin": { "kind": "provider", "name": "anthropic" } },
            "fidelity": "full",
            "method": "POST",
            "scheme": "https",
            "host": "api.anthropic.com",
            "port": 443,
            "path": "/v1/messages",
            "outcome": "ok",
            "body_complete": true,
            "duration_ms": 430,
        }))
        .expect("flow record fixture must match the current FlowRecord shape");
        // The ledger stores `LedgerLine`s (a `{"type":"flow",...}` wrapper),
        // not bare `FlowRecord`s — write what the real writer writes.
        std::fs::write(
            dir.join(format!("{run_id}.jsonl")),
            format!(
                "{}\n",
                serde_json::to_string(&rupu_netflow::LedgerLine::Flow(Box::new(flow.clone())))
                    .unwrap()
            ),
        )
        .unwrap();
        flow
    }

    #[test]
    fn netflow_show_reads_the_runs_ledger_records() {
        let tmp = tempfile::tempdir().unwrap();
        let seeded = seed_ledger(tmp.path(), "run_a");

        let (rows, dropped) = collect_run_ledger(tmp.path(), "run_a").unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, seeded.host);
        assert_eq!(rows[0].path, "/v1/messages");
        assert_eq!(dropped, 0);
    }

    /// A run that made no outbound calls is indistinguishable on disk from
    /// one whose ledger was pruned. Both are an empty read, never an error —
    /// a coordinator reading this over ssh must not turn "no traffic" into
    /// a failed page.
    /// A run recorded in a registered PROJECT's store, not the global one.
    /// Resolving to the wrong store would read no `step_results.jsonl` and
    /// so silently drop every dispatched step's and fan-out item's ledger —
    /// the run's own would survive, which is what makes it silent.
    #[test]
    fn a_project_local_run_resolves_to_its_own_store_not_the_global_one() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path();
        let project = global.join("proj");
        std::fs::create_dir_all(project.join(".rupu/runs")).unwrap();

        let ws_store = rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        };
        rupu_workspace::upsert(&ws_store, &project).expect("register the project as a workspace");

        let store = rupu_orchestrator::RunStore::new(project.join(".rupu/runs"));
        store
            .create(
                serde_json::from_value(serde_json::json!({
                    "id": "run_proj",
                    "workflow_name": "wf",
                    "status": "completed",
                    "inputs": {},
                    "workspace_id": "ws1",
                    "workspace_path": project,
                    "transcript_dir": project.join(".rupu/transcripts"),
                    "started_at": "2026-09-01T00:00:00Z",
                }))
                .unwrap(),
                "",
            )
            .unwrap();

        let (resolved, workspace) = resolve_run_store(global, "run_proj");

        assert!(
            resolved.load("run_proj").is_ok(),
            "the project's own store must be the one returned"
        );
        // The store canonicalizes on registration (/var -> /private/var on
        // macOS), so compare canonical forms.
        assert_eq!(
            workspace.canonicalize().unwrap(),
            project.canonicalize().unwrap(),
            "and its workspace path with it"
        );
    }

    #[test]
    fn netflow_show_is_empty_for_a_run_with_no_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let (rows, dropped) = collect_run_ledger(tmp.path(), "run_missing").unwrap();
        assert!(rows.is_empty());
        assert_eq!(dropped, 0);
    }
}
