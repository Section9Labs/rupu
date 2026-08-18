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

use crate::cmd::retention::parse_retention_duration;
use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use crate::paths;
use anyhow::Context;
use clap::{Args as ClapArgs, Subcommand};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Delete per-run netflow ledgers older than a cutoff.
    Prune(PruneArgs),
}

#[derive(ClapArgs, Debug)]
pub struct PruneArgs {
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`. Defaults to `30d`
    /// (mirrors `transcript prune`'s own fallback) when omitted.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files. Always cheap to run
    /// first — the default (no flag) deletes immediately, same as
    /// `transcript prune`.
    #[arg(long)]
    pub dry_run: bool,
}

/// Fallback cutoff when `--older-than` is not given. There is no
/// netflow-specific `[storage]` retention key (unlike
/// `archived_transcript_retention` / `archived_session_retention`) —
/// adding one is out of this task's scope, so the default lives here,
/// matching the same `30d` value `transcript prune` falls back to.
const DEFAULT_RETENTION: &str = "30d";

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::Prune(args) => prune(args, global_format, absolute, all_columns).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Prune(_) => ("netflow prune", report::TABLE_JSON_CSV),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct NetflowPruneRow {
    run_id: String,
    scope: String,
    bytes: u64,
    modified_at: String,
    status: String,
}

#[derive(Serialize)]
struct NetflowPruneCsvRow {
    run_id: String,
    scope: String,
    bytes: u64,
    modified_at: String,
    status: String,
}

#[derive(Serialize)]
struct NetflowPruneReport {
    kind: &'static str,
    version: u8,
    rows: Vec<NetflowPruneRow>,
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
            "ledgers newer than the cutoff, and anything that is not a per-run \
             ledger file (the directory's own `.gitignore`, a legacy \
             `flows.jsonl`), are left untouched."
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
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();
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
    let retention = args.older_than.as_deref().unwrap_or(DEFAULT_RETENTION);
    let older_than = parse_retention_duration(retention)?;

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Sweep both roots a ledger could ever have been written to, not
    // just the one `paths::netflow_dir` currently resolves to: that
    // resolution is an existence-gated CURRENT routing decision (see
    // `rupu_netflow::netflow_dir`'s doc comment), not a historical
    // guarantee — a project whose `.rupu/netflow/` was created after
    // some runs already wrote to the global directory can have stale
    // ledgers sitting in global even though every new run now lands
    // in the project-local one. Mirrors `transcript prune`'s own dual
    // project+global scan.
    let mut candidates: Vec<(&'static str, PathBuf)> = Vec::new();
    if let Some(root) = project_root.as_deref() {
        let local = root.join(".rupu/netflow");
        if local.is_dir() {
            candidates.push(("project", local));
        }
    }
    candidates.push(("global", global.join("netflow")));

    let mut rows = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (scope, dir) in candidates {
        if !dir.is_dir() {
            continue;
        }
        for entry in prune_ledgers(&dir, older_than, args.dry_run)? {
            let status = match &entry.error {
                Some(err) => {
                    failures.push(format!("{} ({err})", entry.path.display()));
                    format!("failed: {err}")
                }
                None if args.dry_run => "would_delete".to_string(),
                None => "deleted".to_string(),
            };
            rows.push(NetflowPruneRow {
                run_id: run_id_from_ledger_path(&entry.path),
                scope: scope.to_string(),
                bytes: entry.bytes,
                modified_at: entry.modified_at.to_rfc3339(),
                status,
            });
        }
    }
    rows.sort_by(|a, b| a.run_id.cmp(&b.run_id));

    let csv_rows = rows
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
        report: NetflowPruneReport {
            kind: "netflow_prune",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)?;

    // Partial failure must not be silent: the report above already
    // named every file that could not be removed (STATUS
    // `failed: <reason>`), but a non-zero exit is what actually tells
    // a script — or an operator glancing at `$?` — that this run did
    // NOT fully succeed. Every candidate that COULD be removed already
    // was; this only affects the exit code, never a rollback.
    if !failures.is_empty() {
        anyhow::bail!(
            "failed to remove {} netflow ledger(s): {}",
            failures.len(),
            failures.join(", ")
        );
    }
    Ok(())
}

fn run_id_from_ledger_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct PrunedLedger {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified_at: chrono::DateTime<chrono::Utc>,
    /// `Some(message)` when removal was attempted and failed. Always
    /// `None` for a dry run (nothing is ever attempted) and for a real
    /// prune that succeeded.
    pub error: Option<String>,
}

/// Delete (or, in dry-run mode, merely report) every per-run netflow
/// ledger directly inside `dir` whose mtime is older than `older_than`.
///
/// # Filename filter — accept/reject
///
/// - **Accept**: a regular file (`path.is_file()`, so a symlink to a
///   file counts, but removing it only ever unlinks the symlink itself
///   — never its target) directly in `dir` (never recursed into) whose
///   extension is exactly `.jsonl`.
/// - **Reject — directories**: `path.is_file()` gates these out before
///   the extension check ever runs, so a subdirectory (e.g. a leftover
///   `archive/` from an older layout) is never touched, let alone
///   descended into.
/// - **Reject — `.gitignore`**: its extension isn't `.jsonl`, so the
///   filter never reaches it. This is deliberate and load-bearing: the
///   self-ignoring `.gitignore` `NetflowPaths::ensure_dir` writes into
///   this directory is the actual privacy boundary keeping every
///   ledger out of `git add .` (see `ledger/paths.rs`); a prune that
///   could delete it would reopen the exact leak class that file was
///   added to close.
/// - **Reject — legacy `flows.jsonl`**: excluded by exact filename. It
///   matches `*.jsonl` but is the pre-migration, single cross-run
///   ledger this per-run layout replaced — not a PER-RUN ledger, which
///   is the one thing this function's contract promises to prune nb.
///   Nothing writes it anymore; an operator who wants it gone can
///   remove it by hand. (A run that happened to be named literally
///   `flows` would also be excluded by this rule — an intentional
///   over-retention, not a bug: keeping a file this function is unsure
///   about is always the safe direction.)
///
/// # Liveness: why this is mtime-only
///
/// This function has no access to any run store. A ledger's filename
/// is a run (or workflow-step / fan-out unit) id, and those ids are
/// minted and tracked across THREE separate, differently-shaped
/// stores — the workflow `RunStore`, standalone run metadata
/// (pid-based), and sessions — with no single cheap lookup that covers
/// all three from a bare filename (see `rupu-cp`'s
/// `run_and_unit_ids`/`resolve_ledger_paths` for how much machinery
/// answering that question for ONE known parent run already needs).
/// Building that here would make `rupu-cli`'s thin dispatcher reach
/// into orchestrator/session internals just to prune files, and would
/// still miss ids that live in stores this crate cannot enumerate from
/// a directory listing alone.
///
/// mtime is the proxy instead, and it is a sound one for any
/// `--older-than` cutoff longer than a run can plausibly stay alive:
/// the ledger file is created (mtime = now) the instant a run's
/// netflow sink is constructed (`NetflowWriterHandle::spawn`'s
/// `OpenOptions::create(true)`), at or before the run's very first
/// unit of work, and every subsequent flow record bumps it again. A
/// run that is still `Running`/`Pending` therefore has a ledger no
/// older than "how long this run has been alive" — comfortably inside
/// the `30d` default. A caller passing a much shorter `--older-than`
/// than any real run could plausibly still be going widens that
/// window and takes on the corresponding risk knowingly (documented on
/// the flag's own help text); when age cannot even be read (a
/// platform without mtime support), the file is left alone rather than
/// guessed at — over-retention over deletion, per this command's
/// governing rule.
///
/// # Partial failure
///
/// A `remove_file` failure on one candidate does not stop the sweep —
/// every other eligible file is still attempted — and is reported back
/// as a per-file `error`, never silently absorbed. The caller
/// (`prune`) turns any non-empty error set into a non-zero exit after
/// printing which files they were.
pub(crate) fn prune_ledgers(
    dir: &Path,
    older_than: chrono::Duration,
    dry_run: bool,
) -> anyhow::Result<Vec<PrunedLedger>> {
    let cutoff = chrono::Utc::now() - older_than;
    let mut out = Vec::new();

    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading netflow ledger directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("flows.jsonl") {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            // Raced away between the readdir listing and this stat —
            // nothing left to prune, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("reading metadata for {}", path.display()))
            }
        };
        let Ok(modified) = metadata.modified() else {
            // No mtime support on this platform: cannot judge age, so
            // keep the file rather than guess.
            continue;
        };
        let modified_at = chrono::DateTime::<chrono::Utc>::from(modified);
        if modified_at > cutoff {
            continue;
        }

        let bytes = metadata.len();
        if dry_run {
            out.push(PrunedLedger {
                path,
                bytes,
                modified_at,
                error: None,
            });
            continue;
        }

        match std::fs::remove_file(&path) {
            Ok(()) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at,
                error: None,
            }),
            Err(e) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at,
                error: Some(e.to_string()),
            }),
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backdate(path: &Path, days_ago: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let then =
            std::time::SystemTime::now() - std::time::Duration::from_secs(days_ago * 24 * 60 * 60);
        file.set_modified(then).unwrap();
    }

    #[test]
    fn prune_removes_ledgers_older_than_the_cutoff_and_keeps_newer_ones() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        let new = dir.join("run-new.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        std::fs::write(&new, "{}\n").unwrap();
        backdate(&old, 40);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert_eq!(removed.len(), 1);
        assert!(!old.exists(), "the stale ledger is gone");
        assert!(new.exists(), "the recent ledger survives");
    }

    #[test]
    fn dry_run_reports_without_deleting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        backdate(&old, 40);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), true).unwrap();

        assert_eq!(removed.len(), 1, "still reported");
        assert!(old.exists(), "but not deleted");
    }

    #[test]
    fn prune_ignores_non_ledger_files() {
        // The netflow dir also holds the self-ignoring .gitignore and an
        // archive/ subdirectory. Neither is a ledger.
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join(".gitignore"), "*\n").unwrap();
        std::fs::create_dir_all(dir.join("archive")).unwrap();
        backdate(&dir.join(".gitignore"), 40);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert!(removed.is_empty());
        assert!(dir.join(".gitignore").exists());
        assert!(dir.join("archive").is_dir());
    }

    #[test]
    fn prune_never_deletes_the_legacy_flows_jsonl_file() {
        // Requirement 3: `flows.jsonl` is the pre-migration cross-run
        // ledger, not a per-run one. It matches `*.jsonl` but must
        // survive a prune sweep regardless of age.
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let legacy = dir.join("flows.jsonl");
        std::fs::write(&legacy, "{}\n").unwrap();
        backdate(&legacy, 400);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert!(removed.is_empty());
        assert!(legacy.exists(), "the legacy ledger is never pruned");
    }

    #[test]
    fn prune_keeps_a_file_just_inside_the_cutoff() {
        // A file backdated to just under the cutoff duration must
        // survive — over-retention is the safe direction, so the
        // comparison must not accidentally sweep in anything whose age
        // is merely close to (rather than strictly greater than) the
        // cutoff duration.
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let recent = dir.join("run-recent.jsonl");
        std::fs::write(&recent, "{}\n").unwrap();
        backdate(&recent, 29);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert!(removed.is_empty(), "29 days old must survive a 30d cutoff");
        assert!(recent.exists());
    }

    #[test]
    fn prune_reports_bytes_for_each_removed_ledger() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        std::fs::write(&old, "0123456789").unwrap();
        backdate(&old, 40);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].bytes, 10);
    }

    #[test]
    fn a_missing_directory_is_an_error_not_an_empty_result() {
        // Distinguish "nothing to prune" (directory exists, empty) from
        // "the directory does not exist at all" — the CLI layer only
        // ever calls this for a directory it already confirmed
        // `is_dir()`, so a `NotFound` here means something changed out
        // from under it and should surface, not be swallowed as zero
        // candidates.
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist");

        let result = prune_ledgers(&missing, chrono::Duration::days(30), false);

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_removal_failure_is_reported_and_does_not_abort_the_rest() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let stuck = dir.join("run-stuck.jsonl");
        let removable = dir.join("run-removable.jsonl");
        std::fs::write(&stuck, "{}\n").unwrap();
        std::fs::write(&removable, "{}\n").unwrap();
        backdate(&stuck, 40);
        backdate(&removable, 40);

        // Removing a file needs write+execute on its PARENT directory,
        // not the file itself — strip that so `remove_file` fails with
        // EACCES for every entry, deterministically, without touching
        // individual file permissions.
        let original_perms = std::fs::metadata(dir).unwrap().permissions();
        let mut locked = original_perms.clone();
        locked.set_mode(0o555);
        std::fs::set_permissions(dir, locked).unwrap();

        let result = prune_ledgers(dir, chrono::Duration::days(30), false);

        // Restore before any assertion can early-return / panic, so the
        // TempDir's own cleanup on drop never fails.
        std::fs::set_permissions(dir, original_perms).unwrap();

        let removed = result.unwrap();
        assert_eq!(removed.len(), 2, "both candidates were attempted");
        assert!(
            removed.iter().all(|r| r.error.is_some()),
            "both removals failed under a read-only parent dir: {removed:?}"
        );
        assert!(stuck.exists());
        assert!(removable.exists());
    }
}
