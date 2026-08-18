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
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`. Must be positive —
    /// `0s` or a negative value is rejected, not treated as "everything".
    /// Defaults to `30d` (mirrors `transcript prune`'s own fallback) when
    /// omitted. A ledger is matched by file mtime, NOT by asking whether
    /// its owning run has actually finished, so a run that has been idle
    /// (no outbound calls) longer than this cutoff can still be pruned
    /// mid-run — this command cannot tell "idle" from "finished". Run
    /// `--dry-run` first if you're unsure. Anything modified in roughly
    /// the last hour is never eligible regardless of what you pass here.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files. Always cheap to run
    /// first — the default (no flag) deletes immediately, same as
    /// `transcript prune`. Recommended before any cutoff shorter than a
    /// day, since this command cannot distinguish an idle run from a
    /// finished one (see `--older-than`'s own help).
    #[arg(long)]
    pub dry_run: bool,
}

/// Fallback cutoff when `--older-than` is not given. There is no
/// netflow-specific `[storage]` retention key (unlike
/// `archived_transcript_retention` / `archived_session_retention`) —
/// adding one is out of this task's scope, so the default lives here,
/// matching the same `30d` value `transcript prune` falls back to.
const DEFAULT_RETENTION: &str = "30d";

/// Reject a zero or negative `--older-than` outright rather than let
/// `Utc::now() - duration` silently do the wrong thing (Critical fix,
/// netflow-per-run Plan 3 Task 2 review round 1): `0s` puts the cutoff
/// at `now`, so a ledger being appended to at this exact instant
/// satisfies `mtime <= cutoff` and is deleted — silently, since the
/// writer's own open file descriptor survives the unlink on POSIX. A
/// negative value pushes the cutoff into the future and deletes
/// everything unconditionally.
///
/// Deliberately local to `netflow prune`, NOT folded into the shared
/// `crate::cmd::retention::parse_retention_duration` that `transcript
/// prune`/`session prune`/`cleanup` also use: those three commands only
/// ever prune already-ARCHIVED data (a run's own liveness check already
/// gates archival — see `transcript.rs`'s `prune_archived_transcripts`
/// skipping any transcript with a live `session_id`), so an operator
/// deliberately passing `0s` there to mean "everything currently
/// archived" is not the same hazard: there is no live writer on the
/// other end of an archived file. `netflow prune`, by contrast, can
/// reach a ledger belonging to a run that is `Running`/`Pending` RIGHT
/// NOW — see `prune_ledgers`'s own "Liveness" doc section. Those three
/// commands' own test suites already rely on `0s` as a deliberate
/// "select everything regardless of age" convenience (`cli_cleanup.rs`),
/// so widening this check into the shared parser would both misdescribe
/// a risk that doesn't apply there and break existing, intentional
/// test contracts for a problem this review was not about.
fn reject_non_positive_retention(value: &str, duration: chrono::Duration) -> anyhow::Result<()> {
    if duration <= chrono::Duration::zero() {
        anyhow::bail!(
            "invalid duration `{value}`: retention cutoff must be positive \
             (a zero or negative duration would delete everything, including \
             files being written to right now)"
        );
    }
    Ok(())
}

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
    reject_non_positive_retention(retention, older_than)?;

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
    // project+global scan. Both suffixes are DERIVED from
    // `rupu-netflow`'s own path helpers, not hardcoded here a second
    // time — see `rupu_netflow::ledger::paths`'s doc comment on why a
    // second copy of this suffix is exactly the drift it warns against.
    let mut candidates: Vec<(&'static str, PathBuf)> = Vec::new();
    if let Some(root) = project_root.as_deref() {
        let local = rupu_netflow::project_local_netflow_dir(root);
        if local.is_dir() {
            candidates.push(("project", local));
        }
    }
    candidates.push(("global", rupu_netflow::global_netflow_dir(&global)));

    let mut rows = Vec::new();
    // Two DIFFERENT failure shapes, both non-silent: `failures` is a
    // per-file removal failure (we identified the file, selected it,
    // and `remove_file` itself failed — see `PrunedLedger::error`).
    // `scan_failures` is a directory-level failure (an entire root
    // could not even be listed) — distinct because it has no per-file
    // row of its own to attach to, but must not be dropped either.
    let mut failures: Vec<String> = Vec::new();
    let mut scan_failures: Vec<String> = Vec::new();
    for (scope, dir) in candidates {
        if !dir.is_dir() {
            continue;
        }
        // Deliberately NOT `?` — a failure sweeping ONE root (e.g. its
        // `read_dir` failing after a permissions change mid-run) must
        // never discard rows already collected from a PRIOR root in
        // this same loop, some of which may already have been deleted
        // from disk. Record the failure and keep going; every
        // already-known outcome still reaches the report and the exit
        // code still goes non-zero (see below).
        let entries = match prune_ledgers(&dir, older_than, args.dry_run) {
            Ok(entries) => entries,
            Err(e) => {
                scan_failures.push(format!("{} ({e})", dir.display()));
                continue;
            }
        };
        for entry in entries {
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
                modified_at: entry
                    .modified_at
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "unknown".to_string()),
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
    // `failed: <reason>`) AND every root that could not even be
    // scanned, but a non-zero exit is what actually tells a script —
    // or an operator glancing at `$?` — that this run did NOT fully
    // succeed. Every candidate that COULD be removed already was; this
    // only affects the exit code, never a rollback. The report is
    // ALWAYS printed above before this returns, in whatever `--format`
    // was requested — never only on the success path.
    if !failures.is_empty() || !scan_failures.is_empty() {
        let mut detail = Vec::new();
        if !failures.is_empty() {
            detail.push(format!(
                "failed to remove {} ledger(s): {}",
                failures.len(),
                failures.join(", ")
            ));
        }
        if !scan_failures.is_empty() {
            detail.push(format!(
                "failed to scan {} netflow director{}: {}",
                scan_failures.len(),
                if scan_failures.len() == 1 { "y" } else { "ies" },
                scan_failures.join(", ")
            ));
        }
        anyhow::bail!(detail.join("; "));
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
    /// Bytes reclaimed by removing `path`. For a symlink this is
    /// always `0`, never the target's size: `remove_file` on a symlink
    /// unlinks only the link, so reporting the target's byte count
    /// would claim space that was never actually freed.
    pub bytes: u64,
    /// `None` when the file's mtime could not be determined at all
    /// (a metadata read failure recorded via `error` below, or a
    /// platform with no mtime support) — distinct from a real,
    /// successfully-read timestamp.
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `Some(message)` when this entry could not be fully evaluated or
    /// removed — a `remove_file` failure on a selected candidate, a
    /// `std::fs::metadata` failure on an otherwise-matching filename,
    /// or a directory-entry read failure with no path of its own (see
    /// this function's "Partial failure" doc section). Always `None`
    /// for a dry run (nothing is ever attempted) and for a real prune
    /// that succeeded.
    pub error: Option<String>,
}

/// Delete (or, in dry-run mode, merely report) every per-run netflow
/// ledger directly inside `dir` whose mtime is older than `older_than`.
///
/// # Filename filter — accept/reject
///
/// Delegates the filename accept/reject decision to
/// [`rupu_netflow::is_per_run_ledger_path`] — see its doc comment for
/// the full accept/reject reasoning (`*.jsonl` accepted; `.gitignore`
/// and the legacy `flows.jsonl` rejected). That is the ONE place this
/// rule is decided; `rupu-cp`'s read side calls the same function so
/// the two can never drift apart on what counts as a ledger. This
/// function layers exactly one more check on top, AFTER that cheap
/// no-I/O name check: `metadata.is_file()` (from the SAME `metadata()`
/// call already needed for mtime/size, not a second `Path::is_file()`
/// stat — see the "Partial failure" section below for why a second
/// call would be the wrong move) — a destructive prune must never
/// touch a directory (e.g. a leftover `archive/`), which the read side
/// does not need to guard against the same way. A symlink to a file
/// still counts as accepted; removing it only ever unlinks the symlink
/// itself, never walks through to delete its target, and its own size
/// is never counted as reclaimed (see `PrunedLedger::bytes`'s doc).
///
/// # Liveness: why this is mtime-only, and the recency floor
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
/// the flag's own `--help` text — read it there, not just here).
///
/// `MIN_LEDGER_AGE` is the UNCONDITIONAL backstop for that risk: no
/// matter what `older_than` (or `parse_retention_duration`'s own
/// positivity check) allows through, a file modified more recently
/// than this floor is never eligible. This is what makes `--older-than
/// 0s` (rejected outright, see `parse_retention_duration`) and a small
/// positive value like `1s`/`1m` behave the same way — the effective
/// cutoff can never be more recent than `now - MIN_LEDGER_AGE` — so the
/// single most dangerous case (a ledger being appended to at this
/// exact instant getting deleted out from under its writer, silently,
/// because the writer's own open file descriptor survives the unlink
/// on POSIX) is structurally impossible, not just discouraged by
/// documentation. It does NOT fully solve the general "idle gap"
/// problem a `--older-than 1h`/`1d` cutoff still has against a live but
/// quiet run — no floor short enough to be useful for real pruning
/// could — that residual risk is the one the `--older-than`/`--dry-run`
/// help text asks an operator to reason about themselves; the floor's
/// job is only to make the worst, silent, instantaneous case
/// impossible regardless of input.
///
/// When age cannot even be read (a platform without mtime support),
/// the file is left alone rather than guessed at — over-retention over
/// deletion, per this command's governing rule.
///
/// # Partial failure
///
/// Nothing in this loop aborts the sweep once it has begun — every
/// error surfaces as a `PrunedLedger.error` row instead, so a failure
/// on one candidate never discards the record of what was already
/// removed earlier in the same directory:
///
/// - A directory-entry read failure (`entries` yielding an `Err` with
///   no path of its own) is recorded against `dir` itself.
/// - A `std::fs::metadata` failure on an otherwise-matching filename
///   (anything other than the raced-away `NotFound` case, which is
///   silently skipped — nothing is left to prune) is recorded against
///   that file, with `bytes: 0` and `modified_at: None` since neither
///   could be determined.
/// - A `remove_file` failure on a selected candidate is recorded
///   against that file, with the `bytes`/`modified_at` already read
///   before the removal attempt.
///
/// Only `std::fs::read_dir(dir)` itself failing (the directory cannot
/// be opened at all) still returns `Err` from this function — nothing
/// has been scanned yet at that point, so there is no partial state to
/// lose. The caller (`prune`) is responsible for not letting THAT
/// failure discard rows already collected from a different directory
/// in a multi-directory sweep — see its own comment on why it uses a
/// `match`, not `?`, around this function's call.
const MIN_LEDGER_AGE: chrono::Duration = chrono::Duration::hours(1);

pub(crate) fn prune_ledgers(
    dir: &Path,
    older_than: chrono::Duration,
    dry_run: bool,
) -> anyhow::Result<Vec<PrunedLedger>> {
    let now = chrono::Utc::now();
    // See `MIN_LEDGER_AGE`'s doc section above: the effective cutoff
    // can never be more recent than `now - MIN_LEDGER_AGE`, regardless
    // of how short an `older_than` the caller passed in.
    let cutoff = (now - older_than).min(now - MIN_LEDGER_AGE);
    let mut out = Vec::new();

    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading netflow ledger directory {}", dir.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                // No path available for this specific entry — record
                // the failure against the directory itself rather than
                // aborting the rest of the scan.
                out.push(PrunedLedger {
                    path: dir.to_path_buf(),
                    bytes: 0,
                    modified_at: None,
                    error: Some(format!("could not read a directory entry: {e}")),
                });
                continue;
            }
        };
        let path = entry.path();

        // Cheap, no-I/O filename check FIRST: a name this predicate
        // rejects (`.gitignore`, `flows.jsonl`, anything without a
        // `.jsonl` extension) was never a prune candidate, so a
        // permission problem reading THAT file's metadata is
        // irrelevant to this command's contract and must not be
        // reported as a prune failure below.
        if !rupu_netflow::is_per_run_ledger_path(&path) {
            continue;
        }

        // One `metadata()` call, used for both "is this really a
        // file, not a directory" and "is this stale enough". A
        // separate `Path::is_file()` pre-check would call `metadata()`
        // a SECOND time and map any error (including a permission
        // failure) to a bare `false` — silently indistinguishable from
        // "genuinely not a file", which is exactly the class of error
        // this function's "Partial failure" contract promises to
        // surface, not swallow.
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            // Raced away between the readdir listing and this stat —
            // nothing left to prune, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.push(PrunedLedger {
                    path,
                    bytes: 0,
                    modified_at: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            // No mtime support on this platform: cannot judge age, so
            // keep the file rather than guess.
            continue;
        };
        let modified_at = chrono::DateTime::<chrono::Utc>::from(modified);
        if modified_at > cutoff {
            continue;
        }

        // A symlink's OWN size is negligible and not what "bytes
        // reclaimed" should mean; `metadata()` above followed the link
        // to get `len()`, which is the TARGET's size — removing the
        // link never frees that. `symlink_metadata` does not follow
        // the link, so its file type reveals whether `path` itself is
        // a symlink without a second round-trip through the target.
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let bytes = if is_symlink { 0 } else { metadata.len() };

        if dry_run {
            out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
                error: None,
            });
            continue;
        }

        match std::fs::remove_file(&path) {
            Ok(()) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
                error: None,
            }),
            Err(e) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
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
    fn reject_non_positive_retention_accepts_a_positive_duration() {
        assert!(reject_non_positive_retention("30d", chrono::Duration::days(30)).is_ok());
        assert!(reject_non_positive_retention("1s", chrono::Duration::seconds(1)).is_ok());
    }

    #[test]
    fn reject_non_positive_retention_rejects_zero() {
        let err = reject_non_positive_retention("0s", chrono::Duration::zero()).unwrap_err();
        assert!(
            err.to_string().contains("must be positive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_non_positive_retention_rejects_negative() {
        let err = reject_non_positive_retention("-5d", chrono::Duration::days(-5)).unwrap_err();
        assert!(
            err.to_string().contains("must be positive"),
            "unexpected error: {err}"
        );
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

    /// Critical fix (netflow-per-run Plan 3 Task 2 review round 1):
    /// before this floor existed, `--older-than 0s` (now rejected by
    /// `parse_retention_duration`, but exercised here directly against
    /// `prune_ledgers`, which only sees the already-parsed
    /// `chrono::Duration` and has no opinion of its own on what
    /// produced it) put the cutoff at `now`, so a ledger being
    /// appended to at this exact instant satisfied `mtime <= cutoff`
    /// and was deleted — silently, since the writer's own open file
    /// descriptor survives the unlink on POSIX. `MIN_LEDGER_AGE` makes
    /// this structurally impossible: a freshly-written file (mtime =
    /// now) must survive ANY cutoff, however short.
    #[test]
    fn a_very_short_cutoff_never_deletes_something_touched_moments_ago() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let fresh = dir.join("run-fresh.jsonl");
        std::fs::write(&fresh, "{}\n").unwrap();

        let removed = prune_ledgers(dir, chrono::Duration::seconds(1), false).unwrap();

        assert!(
            removed.is_empty(),
            "a file written moments ago must survive even a 1-second cutoff"
        );
        assert!(fresh.exists());
    }

    /// Same floor, from the other direction: a file old enough to
    /// clear BOTH the requested cutoff and the recency floor is still
    /// pruned — the floor must not turn into a blanket "nothing is
    /// ever eligible" behavior.
    #[test]
    fn a_cutoff_past_the_recency_floor_still_prunes_a_genuinely_old_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old = dir.join("run-old.jsonl");
        std::fs::write(&old, "{}\n").unwrap();
        backdate(&old, 40);

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert_eq!(removed.len(), 1);
        assert!(!old.exists());
    }

    /// Minor 3: dry-run and a real run must select the exact same
    /// files. Structurally guaranteed today (one cutoff computation,
    /// one filter chain, `dry_run` only gates the `remove_file` call at
    /// the very end) — pinned here against a future refactor that
    /// accidentally lets the two paths diverge.
    #[test]
    fn dry_run_and_a_real_run_select_the_same_set() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let old_a = dir.join("run-a.jsonl");
        let old_b = dir.join("run-b.jsonl");
        let new_c = dir.join("run-c.jsonl");
        for p in [&old_a, &old_b, &new_c] {
            std::fs::write(p, "{}\n").unwrap();
        }
        backdate(&old_a, 40);
        backdate(&old_b, 35);

        let mut previewed: Vec<_> = prune_ledgers(dir, chrono::Duration::days(30), true)
            .unwrap()
            .into_iter()
            .map(|p| p.path)
            .collect();
        previewed.sort();

        // Dry run touched nothing, so the same two files are still
        // there to select for real.
        let mut removed: Vec<_> = prune_ledgers(dir, chrono::Duration::days(30), false)
            .unwrap()
            .into_iter()
            .map(|p| p.path)
            .collect();
        removed.sort();

        assert_eq!(previewed, vec![old_a.clone(), old_b.clone()]);
        assert_eq!(previewed, removed);
    }

    /// Minor 2: a symlink named `*.jsonl` must never report the
    /// target's byte size as reclaimed — `remove_file` only ever
    /// unlinks the symlink itself, so the target's bytes were never
    /// actually freed.
    #[cfg(unix)]
    #[test]
    fn a_symlink_reports_zero_bytes_reclaimed_not_the_targets_size() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let target = dir.join("target-data.bin");
        std::fs::write(&target, vec![0u8; 4096]).unwrap();
        backdate(&target, 40);

        let link = dir.join("run-link.jsonl");
        symlink(&target, &link).unwrap();

        let removed = prune_ledgers(dir, chrono::Duration::days(30), false).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(
            removed[0].bytes, 0,
            "removing a symlink reclaims none of its target's bytes"
        );
        assert!(!link.exists(), "the symlink itself is gone");
        assert!(
            target.exists(),
            "remove_file on a symlink must never touch its target"
        );
    }

    /// Important 1 fix: a `metadata()` failure on an otherwise-matching
    /// filename must be reported as a per-file error, not silently
    /// skipped (the old `Path::is_file()` pre-check would have mapped
    /// this same error to a bare `false`, indistinguishable from
    /// "genuinely not a file") and must not abort the rest of the
    /// sweep. Strips execute (search) permission from the parent
    /// directory — `read_dir` can still list entry NAMES with only
    /// read permission, but resolving a full path to stat it needs
    /// search permission on every containing directory, so
    /// `std::fs::metadata` on an entry inside fails with EACCES while
    /// the directory listing itself still succeeds.
    #[cfg(unix)]
    #[test]
    fn a_metadata_read_failure_is_reported_and_does_not_abort_the_rest() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let blocked = dir.join("run-blocked.jsonl");
        std::fs::write(&blocked, "{}\n").unwrap();
        backdate(&blocked, 40);

        let original_perms = std::fs::metadata(dir).unwrap().permissions();
        let mut locked = original_perms.clone();
        locked.set_mode(0o600); // read+write, no execute/search
        std::fs::set_permissions(dir, locked).unwrap();

        let result = prune_ledgers(dir, chrono::Duration::days(30), false);

        // Restore before any assertion can early-return / panic, so
        // the TempDir's own cleanup on drop never fails.
        std::fs::set_permissions(dir, original_perms).unwrap();

        let removed = result.unwrap();
        assert_eq!(
            removed.len(),
            1,
            "the entry was found and reported, even though its metadata \
             could not be read: {removed:?}"
        );
        assert!(removed[0].error.is_some());
        assert!(removed[0].modified_at.is_none());
        assert!(blocked.exists());
    }
}
