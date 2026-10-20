//! `rupu netflow prune` and the control plane's
//! `POST /api/maintenance/prune {kind: netflow}`: delete per-run netflow
//! ledgers older than a retention cutoff, never one a live run still owns.
//! It lives here, beside [`crate::api::netflow::netflow_ids`], because what
//! counts as a live run's ledger is that function's answer.

use anyhow::Context;
use rupu_runtime::retention::parse_retention_duration;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Fallback cutoff when `--older-than` is not given. There is no
/// netflow-specific `[storage]` retention key (unlike
/// `archived_transcript_retention` / `archived_session_retention`) —
/// adding one is out of this task's scope, so the default lives here,
/// matching the same `30d` value `transcript prune` falls back to.
pub const DEFAULT_RETENTION: &str = "30d";

/// A retention cutoff `netflow prune` can't take: not a duration, or not
/// positive.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidRetention(String);

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
/// `rupu_runtime::retention::parse_retention_duration` that `transcript
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
fn reject_non_positive_retention(
    value: &str,
    duration: chrono::Duration,
) -> Result<(), InvalidRetention> {
    if duration <= chrono::Duration::zero() {
        return Err(InvalidRetention(format!(
            "invalid duration `{value}`: retention cutoff must be positive \
             (a zero or negative duration would delete everything, including \
             files being written to right now)"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct NetflowPruneRow {
    pub run_id: String,
    pub scope: String,
    pub bytes: u64,
    pub modified_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetflowPruneReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<NetflowPruneRow>,
}

/// What [`prune_netflow`] did: the report, and one line per kind of partial
/// failure (files that could not be removed, roots that could not be
/// scanned). Every candidate that could be removed already was.
#[derive(Debug, Clone)]
pub struct NetflowPrune {
    pub report: NetflowPruneReport,
    pub problems: Vec<String>,
}

/// Sweep the global and project netflow roots for ledgers older than
/// `older_than` (default [`DEFAULT_RETENTION`]); `dry_run` lists them and
/// deletes nothing.
pub fn prune_netflow(
    global: &Path,
    project_root: Option<&Path>,
    older_than: Option<&str>,
    dry_run: bool,
) -> Result<NetflowPrune, InvalidRetention> {
    let retention = older_than.unwrap_or(DEFAULT_RETENTION);
    let older_than =
        parse_retention_duration(retention).map_err(|e| InvalidRetention(e.to_string()))?;
    reject_non_positive_retention(retention, older_than)?;

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
    //
    // DEDUP by canonicalized path before the sweep — `project_root_for`
    // walks up looking for ANY `.rupu/` directory, and the global root
    // IS `~/.rupu`, so running from `$HOME` (or anywhere under it that
    // isn't inside a rupu project) resolves `project_root = ~`, which
    // makes `project_local_netflow_dir(~)` and `global_netflow_dir`
    // literally the same directory. Undeduped, that directory would be
    // swept twice: the real prune is harmless (the second pass finds
    // the files already gone), but `--dry-run` would list every
    // eligible ledger twice and report double the reclaimable bytes —
    // the one number an operator is consulting the preview for. Same
    // fix shape `transcript prune` (`canonicalize(&p).unwrap_or_else
    // (|_| p.clone())`, `cmd/transcript.rs`) and `rupu-cp`'s read side
    // (`canonicalize_or_self`) already use for the identical collision.
    // The FIRST occurrence wins, so "project" is kept over "global"
    // when the two collide — a project-scoped ledger reads better than
    // a global one in the report.
    let mut candidates: Vec<(&'static str, PathBuf)> = Vec::new();
    let mut seen_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let dedupe_key = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some(root) = project_root {
        let local = rupu_netflow::project_local_netflow_dir(root);
        if local.is_dir() && seen_dirs.insert(dedupe_key(&local)) {
            candidates.push(("project", local));
        }
    }
    let global_netflow = rupu_netflow::global_netflow_dir(global);
    if seen_dirs.insert(dedupe_key(&global_netflow)) {
        candidates.push(("global", global_netflow));
    }

    // Every ledger id a still-live run owns: never pruned, however old.
    let live = live_ledger_ids(global, project_root);

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
        let entries = match prune_ledgers(&dir, older_than, dry_run, &live) {
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
                None if entry.skipped_live => "skipped_live".to_string(),
                None if dry_run => "would_delete".to_string(),
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

    let mut problems = Vec::new();
    if !failures.is_empty() {
        problems.push(format!(
            "failed to remove {} ledger(s): {}",
            failures.len(),
            failures.join(", ")
        ));
    }
    if !scan_failures.is_empty() {
        problems.push(format!(
            "failed to scan {} netflow director{}: {}",
            scan_failures.len(),
            if scan_failures.len() == 1 { "y" } else { "ies" },
            scan_failures.join(", ")
        ));
    }
    Ok(NetflowPrune {
        report: NetflowPruneReport {
            kind: "netflow_prune",
            version: 1,
            rows,
        },
        problems,
    })
}

pub fn run_id_from_ledger_path(path: &Path) -> String {
    // A directory-entry read failure (see `prune_ledgers`'s "Partial
    // failure" doc section) is recorded against the DIRECTORY itself,
    // not a `.jsonl` file — no path was ever identified for it.
    // `file_stem()` on a bare directory path returns its last
    // component (e.g. `netflow`), which would print as a run id and
    // read like a specific run failed when none was ever named. Any
    // real ledger entry — success or a per-file failure — always has a
    // `.jsonl` path, so gating on the extension cleanly separates the
    // two cases.
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return "-".to_string();
    }
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, Clone)]
pub struct PrunedLedger {
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
    /// Old enough to prune, but its run is still live (see
    /// [`live_ledger_ids`]): kept, never deleted, `bytes` not reclaimed.
    pub skipped_live: bool,
}

/// Every netflow ledger id (the ledger's file stem) owned by a run that is
/// still live, so `prune` never deletes a live run's audit trail however
/// long it has been idle:
///
/// - a workflow run in the global or project-local `RunStore` that is not
///   terminal — `Paused`/`AwaitingApproval` included, since a resume keeps
///   appending to the same ledgers — unless it is `Running`/`Pending` with
///   a recorded runner pid that is dead (an orphan the reaper will
///   finalize). Its id expands to its steps', fan-out units', sub-agents'
///   and launched children's ids, plus every ledger a placed unit landed
///   here ([`crate::api::netflow::netflow_ids`]);
/// - a standalone `rupu run` whose `<run_id>.meta.json` (project or global
///   transcripts dir) records a pid that is alive — the run writes its
///   `run.json` only when it ends, so the metadata pid is its one liveness
///   signal — expanded the same way. Metadata without a pid (written
///   before the field existed) carries no signal and protects nothing.
///
/// Unreadable stores and metadata files are skipped: a run this cannot see
/// is judged by age alone, exactly as before this check existed.
pub fn live_ledger_ids(
    global: &Path,
    project_root: Option<&Path>,
) -> std::collections::HashSet<String> {
    use rupu_orchestrator::{RunStatus, RunStore};
    let mut live = std::collections::HashSet::new();
    let global_store = RunStore::new(global.join("runs"));

    let mut stores = vec![RunStore::new(global.join("runs"))];
    if let Some(root) = project_root {
        let local = root.join(".rupu").join("runs");
        let same =
            std::fs::canonicalize(&local).ok() == std::fs::canonicalize(global.join("runs")).ok();
        if local.is_dir() && !same {
            stores.push(RunStore::new(local));
        }
    }
    for store in &stores {
        for rec in store.list().unwrap_or_default() {
            let orphaned = matches!(rec.status, RunStatus::Running | RunStatus::Pending)
                && rec
                    .runner_pid
                    .is_some_and(|pid| !rupu_orchestrator::runs::pid_is_running(pid));
            if rec.status.is_terminal() || orphaned {
                continue;
            }
            live.extend(crate::api::netflow::netflow_ids(store, global, &rec.id).ids);
        }
    }

    let mut transcript_dirs = vec![global.join("transcripts")];
    if let Some(root) = project_root {
        transcript_dirs.push(root.join(".rupu").join("transcripts"));
    }
    for dir in transcript_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_meta = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".meta.json"));
            if !is_meta {
                continue;
            }
            let Ok(meta) = rupu_runtime::standalone_run_metadata::read_metadata(&path) else {
                continue;
            };
            if meta
                .pid
                .is_some_and(rupu_orchestrator::runs::pid_is_running)
            {
                live.extend(crate::api::netflow::run_and_unit_ids(
                    global,
                    &global_store,
                    &meta.run_id,
                ));
            }
        }
    }
    live
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
/// # Liveness: live runs are skipped, then mtime, then the recency floor
///
/// A ledger's filename is a run (or workflow-step / fan-out unit /
/// sub-agent) id. Ids owned by a run that is still live — the set the
/// caller builds once with [`live_ledger_ids`] and passes as `live` — are
/// never deleted, however old their mtime: an idle-but-live run (a long
/// tool call, a run parked at a gate) keeps its audit trail. They come
/// back as `skipped_live` rows so the operator sees what was kept and why.
///
/// For every other ledger, mtime is the age: the file is created the
/// instant a run's netflow sink is constructed and every flow record
/// bumps it again. A run [`live_ledger_ids`] cannot see (an unreadable
/// store, or a standalone run whose metadata predates the recorded pid)
/// is judged by age alone.
///
/// `MIN_LEDGER_AGE` is the UNCONDITIONAL backstop under both: no
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
/// documentation — even for a run the live check cannot see.
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
pub const MIN_LEDGER_AGE: chrono::Duration = chrono::Duration::hours(1);

pub fn prune_ledgers(
    dir: &Path,
    older_than: chrono::Duration,
    dry_run: bool,
    live: &std::collections::HashSet<String>,
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
                    skipped_live: false,
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
                    skipped_live: false,
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

        // Old enough, but its run is still live: keep it, and say so.
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if live.contains(stem) {
            out.push(PrunedLedger {
                path,
                bytes: 0,
                modified_at: Some(modified_at),
                error: None,
                skipped_live: true,
            });
            continue;
        }

        if dry_run {
            out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
                error: None,
                skipped_live: false,
            });
            continue;
        }

        match std::fs::remove_file(&path) {
            Ok(()) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
                error: None,
                skipped_live: false,
            }),
            Err(e) => out.push(PrunedLedger {
                path,
                bytes,
                modified_at: Some(modified_at),
                error: Some(e.to_string()),
                skipped_live: false,
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

    /// Can this process actually be denied by directory permissions?
    ///
    /// The two failure-path tests below force `EACCES` by stripping write
    /// or search permission from a ledger's parent directory. That only
    /// works unprivileged: root holds `CAP_DAC_OVERRIDE`, so `remove_file`
    /// and `metadata` succeed anyway and the tests would assert on errors
    /// that never happened. CI runs as root, which is exactly where they
    /// first failed.
    ///
    /// Probe by attempting the operation rather than reading the uid —
    /// `geteuid` needs `libc` and `unsafe`, which is forbidden
    /// workspace-wide, and the probe answers the question that actually
    /// matters (is the denial enforced *here*) rather than a proxy for it.
    #[cfg(unix)]
    fn permission_denial_is_enforced() -> bool {
        use std::os::unix::fs::PermissionsExt;

        let probe = tempfile::TempDir::new().unwrap();
        let dir = probe.path();
        let victim = dir.join("probe.jsonl");
        std::fs::write(&victim, "{}\n").unwrap();

        let original = std::fs::metadata(dir).unwrap().permissions();
        let mut locked = original.clone();
        locked.set_mode(0o555);
        std::fs::set_permissions(dir, locked).unwrap();
        let denied = std::fs::remove_file(&victim).is_err();
        // Restore before the TempDir drops, or its own cleanup fails.
        std::fs::set_permissions(dir, original).unwrap();
        denied
    }

    #[test]
    fn run_id_from_ledger_path_uses_a_sentinel_for_a_non_jsonl_path() {
        // Minor 3 (netflow-per-run Plan 3 Task 2 review round 2): a
        // directory-entry read failure is recorded against the
        // directory itself, not a `.jsonl` file. `file_stem()` on that
        // bare directory path would print its own last component
        // (`netflow`) as though it were a failed run's id.
        assert_eq!(
            run_id_from_ledger_path(Path::new("/home/x/.rupu/netflow")),
            "-"
        );
        assert_eq!(
            run_id_from_ledger_path(Path::new("/home/x/.rupu/netflow/run_01ABC.jsonl")),
            "run_01ABC"
        );
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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), true, &Default::default()).unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let result = prune_ledgers(
            &missing,
            chrono::Duration::days(30),
            false,
            &Default::default(),
        );

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_removal_failure_is_reported_and_does_not_abort_the_rest() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        if !permission_denial_is_enforced() {
            eprintln!(
                "skipping a_removal_failure_is_reported_and_does_not_abort_the_rest: this \
                 process can bypass directory permissions (running as root?), so EACCES \
                 cannot be forced and the assertions would be vacuous"
            );
            return;
        }

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

        let result = prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default());

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

        let removed = prune_ledgers(
            dir,
            chrono::Duration::seconds(1),
            false,
            &Default::default(),
        )
        .unwrap();

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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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

        let mut previewed: Vec<_> =
            prune_ledgers(dir, chrono::Duration::days(30), true, &Default::default())
                .unwrap()
                .into_iter()
                .map(|p| p.path)
                .collect();
        previewed.sort();

        // Dry run touched nothing, so the same two files are still
        // there to select for real.
        let mut removed: Vec<_> =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default())
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

        let removed =
            prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default()).unwrap();

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
        if !permission_denial_is_enforced() {
            eprintln!(
                "skipping a_metadata_read_failure_is_reported_and_does_not_abort_the_rest: \
                 this process can bypass directory permissions (running as root?), so the \
                 metadata read cannot be made to fail"
            );
            return;
        }

        let dir = tmp.path();
        let blocked = dir.join("run-blocked.jsonl");
        std::fs::write(&blocked, "{}\n").unwrap();
        backdate(&blocked, 40);

        let original_perms = std::fs::metadata(dir).unwrap().permissions();
        let mut locked = original_perms.clone();
        locked.set_mode(0o600); // read+write, no execute/search
        std::fs::set_permissions(dir, locked).unwrap();

        let result = prune_ledgers(dir, chrono::Duration::days(30), false, &Default::default());

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
