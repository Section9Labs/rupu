//! Attach a report to an existing summary finding: the one place a finding
//! line is rewritten rather than appended. Used by `rupu findings import`,
//! the one-time migration of reports that were written beside summary
//! findings before the full profile existed (spec: "Backfill").
//!
//! A report attaches whole or not at all: it goes through the same
//! validation and artifact store as `report_finding` (evidence-block files
//! included: verified, stored, and their refs replaced), and a finding that
//! already has a report is never touched. Unlike `report_finding`, claim
//! files are not hashed: an imported report's claims were made against the
//! code as it was when the report was written, so a hash of today's file
//! would present them as current. Its claims are stored unhashed (a viewer
//! shows their evidence status as unknown). Artifacts are copied from the
//! workspace as it is at import. The ledger is replaced atomically under
//! the ledger lock, after a byte-for-byte backup, and every other line is
//! written back byte for byte (CRLF endings, a missing final newline, lines
//! that are not UTF-8 or not JSON, and keys this version does not know all
//! survive). The line that is replaced keeps its own line terminator. A
//! ledger that is a symlink is never replaced.
//!
//! A record holds no engagement asset, so an attached report goes through no
//! engagement-profile completeness check and stamps no asset (unlike
//! `report_finding` with an active engagement): it is held to the finding
//! report contract only.
//!
//! A dry run reads the ledger and touches nothing else: it takes no lock and
//! creates no file or directory, so it works on a read-only ledger directory.

use crate::ledger::events::FindingRecord;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::stream::lock_findings;
use crate::report::{
    ArtifactError, ArtifactStore, FindingProfile, FindingReport, FindingWriteOptions,
};
use crate::tools::report_finding::{
    check_block_files, check_full_report, check_stored_size, derived_fields, planned_ref,
    prepare_full_report, ClaimHashes, ReportFindingError,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A report to attach to the finding `finding_id`.
#[derive(Debug, Clone)]
pub struct AttachItem {
    pub finding_id: String,
    pub report: FindingReport,
}

#[derive(Debug)]
pub enum AttachOutcome {
    /// Written (on a dry run: would be; the report passed validation and
    /// its artifacts resolve).
    Attached,
    /// No finding with this id in this ledger.
    NotFound,
    /// The finding already has a report (or is a full-profile finding whose
    /// report no longer parses); left unchanged.
    AlreadyHasReport,
    /// The id is on more than one ledger line, or an earlier item in this
    /// batch already targets it; left unchanged.
    Duplicate,
    /// The report failed validation, or its artifacts could not be stored.
    Rejected(ReportFindingError),
    /// The report would have attached, but the ledger was not written (or,
    /// on a dry run, could not be): [`AttachBatch::write_error`] says why.
    NotWritten,
}

#[derive(Debug)]
pub struct AttachBatch {
    /// One outcome per item, in input order.
    pub outcomes: Vec<AttachOutcome>,
    /// A copy of the ledger as it was before the rewrite. `None` when
    /// nothing was written (a dry run, no item attached, or the write
    /// failed).
    pub backup: Option<PathBuf>,
    /// Why the ledger was not written: it could not be locked, it is a
    /// symlink, it changed during the import, or staging or replacing it
    /// failed. When set, the ledger is as it was, every item that would have
    /// attached is [`AttachOutcome::NotWritten`], and the other outcomes
    /// (validation problems included) stand. A dry run sets it only for a
    /// symlinked ledger, which a real run would refuse.
    pub write_error: Option<std::io::Error>,
}

/// Attach each item's report to its finding. With `dry_run`, only report what
/// would happen: the ledger is read, but no lock is taken and nothing is
/// created or written. A dry run's `Attached` means the report passed
/// validation and every artifact it lists, and every evidence-block file,
/// exists inside the workspace, within the count and size limits.
///
/// `Err` only when the ledger cannot be read: nothing was assessed or
/// written. A failure to write it is [`AttachBatch::write_error`], alongside
/// every item's outcome.
pub fn attach_reports(
    paths: &CoveragePaths,
    items: Vec<AttachItem>,
    opts: &FindingWriteOptions,
    dry_run: bool,
) -> std::io::Result<AttachBatch> {
    attach_reports_with(paths, items, opts, dry_run, &mut || {})
}

/// [`attach_reports`] with a hook that runs after the replacement ledger is
/// staged and before it is renamed into place. It is a test seam: the hook is
/// where a test can stand in for a writer that does not take the lock (an
/// older worker, say) or check that the lock is held. The length check that
/// catches such a writer is not limited to this window: an unlocked append
/// anywhere between the read and that check (just before the rename) is
/// detected. One that lands between the check and the rename, or a write
/// through a handle opened on the old ledger that lands after the rename, is
/// not.
fn attach_reports_with(
    paths: &CoveragePaths,
    items: Vec<AttachItem>,
    opts: &FindingWriteOptions,
    dry_run: bool,
    before_rename: &mut dyn FnMut(),
) -> std::io::Result<AttachBatch> {
    // Checked on a dry run too, so the dry run predicts the refusal.
    let mut write_error = refuse_symlinked_ledger(&paths.findings).err();
    // A dry run writes nothing, so it needs no exclusion (and taking the lock
    // would create the sidecar, and fail on a read-only directory).
    let _lock = if dry_run || write_error.is_some() {
        None
    } else {
        match lock_findings(paths) {
            Ok(lock) => Some(lock),
            Err(e) => {
                write_error = Some(e);
                None
            }
        }
    };
    // A ledger that will not be written is only assessed, as a dry run
    // assesses it, so each item still gets its own outcome.
    let assess_only = dry_run || write_error.is_some();
    let raw = match std::fs::read(&paths.findings) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let read_len = raw.len() as u64;
    // One segment per line, each with its own terminator. Segments nothing
    // replaces are written back exactly as read.
    let mut segments: Vec<Vec<u8>> = raw
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let mut at: HashMap<String, Vec<usize>> = HashMap::new();
    let mut records: HashMap<usize, FindingRecord> = HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        if let Some(r) = line_text(seg).and_then(|t| serde_json::from_str::<FindingRecord>(t).ok())
        {
            at.entry(r.id.clone()).or_default().push(i);
            records.insert(i, r);
        }
    }
    let mut known: Vec<String> = at.keys().cloned().collect();

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut changed = false;
    let mut outcomes = Vec::with_capacity(items.len());
    for item in items {
        // A report may not cross-reference the finding it is attached to:
        // its own id is left out of the known ids while it is checked.
        let own = known
            .iter()
            .position(|k| *k == item.finding_id)
            .map(|i| known.swap_remove(i));
        let outcome = match at.get(&item.finding_id).map(Vec::as_slice) {
            None => AttachOutcome::NotFound,
            Some([i]) if claimed.contains(i) => AttachOutcome::Duplicate,
            Some([i]) => {
                let record = &records[i];
                let text = line_text(&segments[*i]).unwrap_or_default();
                // `record.report` is read leniently (a report this build
                // cannot parse loads as `None`), so the raw line is checked
                // too: a stored report is never overwritten.
                if record.report.is_some()
                    || record.profile == FindingProfile::Full
                    || has_raw_report(text)
                {
                    AttachOutcome::AlreadyHasReport
                } else if assess_only {
                    match dry_run_check(paths, &item.report, &known, opts) {
                        Ok(()) => {
                            claimed.insert(*i);
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                } else {
                    // Only a report that attaches claims the finding: a
                    // rejected one leaves it free for a later item.
                    match prepare_full_report(paths, item.report, &known, opts, ClaimHashes::Skip)
                        .and_then(|report| Ok(upgraded_line(text, &report)?))
                    {
                        Ok(mut line) => {
                            line.push_str(terminator(&segments[*i]));
                            claimed.insert(*i);
                            segments[*i] = line.into_bytes();
                            changed = true;
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                }
            }
            Some(_) => AttachOutcome::Duplicate,
        };
        known.extend(own);
        outcomes.push(outcome);
    }

    let mut backup = None;
    if changed {
        match replace_ledger(paths, &segments, read_len, before_rename) {
            Ok(b) => backup = Some(b),
            Err(e) => write_error = Some(e),
        }
    }
    if write_error.is_some() {
        for o in &mut outcomes {
            if matches!(o, AttachOutcome::Attached) {
                *o = AttachOutcome::NotWritten;
            }
        }
    }
    Ok(AttachBatch {
        outcomes,
        backup,
        write_error,
    })
}

/// Refuse a ledger that is a symlink: replacing it by rename would put a
/// regular file where the link was, and the file it points to would silently
/// stop receiving findings.
fn refuse_symlinked_ledger(findings: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(findings) {
        Ok(m) if m.file_type().is_symlink() => Err(std::io::Error::other(format!(
            "{} is a symlink; import does not replace a symlinked ledger (it would replace the link, not the file it points to)",
            findings.display()
        ))),
        _ => Ok(()),
    }
}

/// The rejections a real run gives, minus the ones that need the artifact
/// store to be written to or a file to be read in full: the report
/// validates, an artifact store is configured when the report lists
/// artifacts or evidence-block files, every listed artifact path and block
/// file exists inside the workspace (a block's as one file) and is within
/// the finding's shared count and size limits (`ArtifactError::Missing` / `Escapes` /
/// `TooManyFiles` / `TooLarge`, as a real ingest gives), and the report as it
/// would be stored, its artifacts expanded and recorded, is within
/// `report_max_bytes`. Nothing is created, hashed or copied; each artifact's
/// first 8 KiB is read to tell text from binary, as a real ingest does.
fn dry_run_check(
    paths: &CoveragePaths,
    report: &FindingReport,
    known: &[String],
    opts: &FindingWriteOptions,
) -> Result<(), ReportFindingError> {
    check_full_report(report, known, opts)?;
    // What `prepare_full_report` would store, with `ClaimHashes::Skip`.
    let mut stored = report.clone();
    for claim in &mut stored.evidence {
        claim.sha256 = None;
    }
    let has_block_files = report.blocks.iter().any(|b| b.artifact().is_some());
    if !report.artifacts.is_empty() || has_block_files {
        let root = opts.artifact_root.as_ref().ok_or(ArtifactError::NoStore)?;
        let store = ArtifactStore::new(root);
        let limits = opts.ingest_limits();
        if !report.artifacts.is_empty() {
            // Stand-in digests as long as the ones a real ingest records, so
            // the size checked is the stored report's.
            stored.artifacts = store
                .check(&paths.workspace, &report.artifacts, limits)?
                .into_iter()
                .map(planned_ref)
                .collect();
        }
        // Block files share the finding's budget with `artifacts`, as in
        // `prepare_full_report`.
        check_block_files(&store, &paths.workspace, &mut stored, limits)?;
    }
    check_stored_size(&stored, opts)
}

/// A ledger line as JSON text: `None` when it is not UTF-8, otherwise the
/// text without its `\n` / `\r\n` terminator.
fn line_text(segment: &[u8]) -> Option<&str> {
    std::str::from_utf8(segment)
        .ok()
        .map(|t| t.trim_end_matches(['\n', '\r']))
}

/// The line terminator `segment` ends with: `"\r\n"`, `"\n"`, or `""` for
/// an unterminated final line.
fn terminator(segment: &[u8]) -> &'static str {
    if segment.ends_with(b"\r\n") {
        "\r\n"
    } else if segment.ends_with(b"\n") {
        "\n"
    } else {
        ""
    }
}

/// Whether the raw line carries a non-null `report` key, whether or not this
/// build can parse what is in it.
fn has_raw_report(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("report").map(|r| !r.is_null()))
        .unwrap_or(false)
}

/// `line` with the report attached and the fields a full finding derives
/// from it. Edited as JSON, not re-serialized from `FindingRecord`, so keys
/// this version does not know survive.
fn upgraded_line(line: &str, report: &FindingReport) -> Result<String, serde_json::Error> {
    let mut v: serde_json::Value = serde_json::from_str(line)?;
    let (summary, severity, evidence) = derived_fields(report);
    let Some(obj) = v.as_object_mut() else {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "a finding line is not a JSON object",
        ));
    };
    obj.insert("summary".into(), summary.into());
    obj.insert("severity".into(), serde_json::to_value(severity)?);
    obj.insert("evidence".into(), serde_json::to_value(evidence)?);
    obj.insert(
        "profile".into(),
        serde_json::to_value(FindingProfile::Full)?,
    );
    obj.insert("report".into(), serde_json::to_value(report)?);
    serde_json::to_string(&v)
}

/// The first unused `findings.jsonl.pre-import-<stamp>[-n]` in `root`.
fn backup_path(root: &Path, stamp: &str) -> PathBuf {
    let name = format!("findings.jsonl.pre-import-{stamp}");
    let mut backup = root.join(&name);
    let mut n = 1;
    while backup.exists() {
        n += 1;
        backup = root.join(format!("{name}-{n}"));
    }
    backup
}

/// Back the ledger up byte for byte, then replace it with `segments` via a
/// temp file and a rename, so a reader sees the old ledger or the new one,
/// never a mix. Returns the backup's path. On any failure before the rename
/// nothing is left behind: not the temp file, not the backup (including a
/// partial one).
///
/// The lock only excludes writers that take it. `read_len` is the ledger's
/// length when it was read; if it has changed by the time the replacement is
/// ready, a writer that did not take the lock appended in between and the
/// rename would drop its line, so the import is abandoned instead.
fn replace_ledger(
    paths: &CoveragePaths,
    segments: &[Vec<u8>],
    read_len: u64,
    before_rename: &mut dyn FnMut(),
) -> std::io::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let backup = backup_path(&paths.root, &stamp);
    // Synced before the swap: the backup is the only copy of what the
    // rewrite overwrites, so it must not be lost where the new ledger
    // survives (a crash soon after the rename, with delayed allocation).
    let copied = std::fs::copy(&paths.findings, &backup)
        .and_then(|_| std::fs::File::open(&backup)?.sync_all());
    if let Err(e) = copied {
        // `backup` did not exist before the copy, so anything there is a
        // partial copy of ours.
        let _ = std::fs::remove_file(&backup);
        return Err(e);
    }
    let tmp = paths.root.join("findings.jsonl.import-tmp");
    let swapped = stage_and_swap(paths, segments, read_len, &tmp, before_rename);
    if swapped.is_err() {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&backup);
    }
    swapped?;
    // Make the rename itself durable, not only the file's contents. Best
    // effort: the ledger has been swapped by now, so a filesystem that cannot
    // sync a directory must not turn a completed import into an error (the
    // caller would lose the outcomes, and a retry would find every item
    // already attached).
    if let Err(e) = std::fs::File::open(&paths.root).and_then(|d| d.sync_all()) {
        tracing::warn!(
            ?e,
            dir = ?paths.root,
            "findings ledger replaced, but syncing its directory failed"
        );
    }
    Ok(backup)
}

/// Give the replacement the ledger's owner and group. Replaced by another
/// user (an import run with `sudo`), the ledger would otherwise become that
/// user's, and its owner's agents could no longer append to it; where the
/// owner cannot be kept (an importer who is neither the owner nor root), the
/// import is refused rather than taking the ledger over. A group the
/// importer cannot give it (the owner, not in the ledger's group, as when a
/// container wrote it) changes to the importer's, with no group access and a
/// warning: its owner can still append.
#[cfg(unix)]
fn keep_owner(f: &std::fs::File, ledger: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let now = f.metadata()?;
    let gid = (now.gid() != ledger.gid()).then(|| rustix::fs::Gid::from_raw(ledger.gid()));
    if now.uid() != ledger.uid() {
        let uid = rustix::fs::Uid::from_raw(ledger.uid());
        return rustix::fs::fchown(f, Some(uid), gid).map_err(|e| {
            std::io::Error::other(format!(
                "cannot keep the ledger's owner ({}) on its replacement: {}; run the import as the ledger's owner",
                ledger.uid(),
                std::io::Error::from(e)
            ))
        });
    }
    if let Err(e) = gid.map_or(Ok(()), |g| rustix::fs::fchown(f, None, Some(g))) {
        // Before any content is written: the ledger's group permissions
        // must not pass to the importer's group.
        use std::os::unix::fs::PermissionsExt;
        let mode = ledger.permissions().mode() & !0o070;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        tracing::warn!(
            error = %std::io::Error::from(e),
            group = ledger.gid(),
            "cannot keep the findings ledger's group; the rewritten ledger takes the importer's, with no group access"
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn keep_owner(_: &std::fs::File, _: &std::fs::Metadata) -> std::io::Result<()> {
    Ok(())
}

fn stage_and_swap(
    paths: &CoveragePaths,
    segments: &[Vec<u8>],
    read_len: u64,
    tmp: &Path,
    before_rename: &mut dyn FnMut(),
) -> std::io::Result<()> {
    let ledger = std::fs::metadata(&paths.findings)?;
    let mut f = std::fs::File::create(tmp)?;
    // Before any content is written, so it is never readable at a looser
    // mode than the ledger it replaces.
    f.set_permissions(ledger.permissions())?;
    keep_owner(&f, &ledger)?;
    for segment in segments {
        f.write_all(segment)?;
    }
    f.sync_all()?;
    drop(f);
    before_rename();
    if std::fs::metadata(&paths.findings)?.len() != read_len {
        return Err(std::io::Error::other(
            "the findings ledger changed during import (a writer that does not take the ledger lock appended to it); nothing was written, retry the import",
        ));
    }
    std::fs::rename(tmp, &paths.findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingEvidence, FindingScope, Surface};
    use crate::report::FindingProfile;
    use crate::tools::report_finding::{report_finding, ReportFindingInput};
    use crate::Severity;

    fn report() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn seed_summary(paths: &CoveragePaths) -> String {
        let input = ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: Some("s".into()),
            severity: Some(Severity::Low),
            concern_id: None,
            evidence: Some(FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            }),
            report: None,
            asset: None,
        };
        let attribution = Attribution {
            run_id: "r".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        };
        let opts = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
        report_finding(paths, attribution, input, &opts).unwrap().id
    }

    fn names(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    }

    #[test]
    fn a_ledger_that_changed_during_import_is_left_as_the_other_writer_wrote_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_summary(&paths);
        let other = seed_summary(&paths);
        let original = std::fs::read(&paths.findings).unwrap();
        let mut invalid = report();
        invalid.root_cause = String::new();

        // A writer that does not take the lock appends between the read and
        // the rename.
        let appended = b"{\"from\":\"a writer that ignores the lock\"}\n";
        let batch = attach_reports_with(
            &paths,
            vec![
                AttachItem {
                    finding_id: other,
                    report: invalid,
                },
                AttachItem {
                    finding_id: id,
                    report: report(),
                },
            ],
            &FindingWriteOptions::default().with_profile(FindingProfile::Full),
            false,
            &mut || {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&paths.findings)
                    .unwrap()
                    .write_all(appended)
                    .unwrap();
            },
        )
        .expect("the ledger was read");
        let err = batch.write_error.expect("the import must be abandoned");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(err.to_string().contains("changed during import"), "{err}");
        assert!(batch.backup.is_none());
        // Each item keeps its own outcome: the invalid report its problems,
        // the valid one "not written".
        match &batch.outcomes[0] {
            AttachOutcome::Rejected(ReportFindingError::Report(v)) => {
                assert!(v.0.iter().any(|e| e.path == "report.root_cause"), "{v:?}")
            }
            o => panic!("expected the validation problems, got {o:?}"),
        }
        assert!(
            matches!(batch.outcomes[1], AttachOutcome::NotWritten),
            "{:?}",
            batch.outcomes[1]
        );

        // The other writer's line is intact and nothing of ours is written.
        let mut expected = original;
        expected.extend_from_slice(appended);
        assert_eq!(std::fs::read(&paths.findings).unwrap(), expected);
        let leftovers: Vec<_> = names(&paths.root)
            .into_iter()
            .filter(|n| n.contains("pre-import") || n.contains("import-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn the_ledger_lock_is_held_from_read_to_rename_and_released_after() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_summary(&paths);
        let sidecar = paths.root.join("findings.jsonl.lock");
        let try_lock = || {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&sidecar)
                .unwrap();
            // Dropping `f` releases a lock this probe happened to win.
            f.try_lock()
        };

        let mut probed = false;
        attach_reports_with(
            &paths,
            vec![AttachItem {
                finding_id: id,
                report: report(),
            }],
            &FindingWriteOptions::default().with_profile(FindingProfile::Full),
            false,
            &mut || {
                probed = true;
                assert!(
                    matches!(try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                    "the ledger lock must still be held right before the rename"
                );
            },
        )
        .unwrap();
        assert!(probed, "the hook ran, so the ledger was rewritten");
        assert!(
            try_lock().is_ok(),
            "the lock is released when the import returns"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_rewritten_ledger_keeps_its_group() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_summary(&paths);
        let created = std::fs::metadata(&paths.findings).unwrap().gid();
        // Another group this user belongs to: a file it creates does not get
        // that one, so keeping it takes the chown.
        let groups = std::process::Command::new("id").arg("-G").output().unwrap();
        let other = String::from_utf8(groups.stdout)
            .unwrap()
            .split_whitespace()
            .filter_map(|g| g.parse::<u32>().ok())
            .find(|g| *g != created);
        let Some(other) = other else {
            return; // one group only: nothing to keep
        };
        std::os::unix::fs::chown(&paths.findings, None, Some(other)).unwrap();
        let batch = attach_reports(
            &paths,
            vec![AttachItem {
                finding_id: id,
                report: report(),
            }],
            &FindingWriteOptions::default().with_profile(FindingProfile::Full),
            false,
        )
        .unwrap();
        assert!(
            matches!(batch.outcomes.as_slice(), [AttachOutcome::Attached]),
            "{:?} {:?}",
            batch.outcomes,
            batch.write_error
        );
        assert_eq!(std::fs::metadata(&paths.findings).unwrap().gid(), other);
    }

    #[test]
    fn backups_taken_in_the_same_second_get_distinct_names() {
        let tmp = tempfile::TempDir::new().unwrap();
        let stamp = "20260930T120000Z";
        let first = backup_path(tmp.path(), stamp);
        std::fs::write(&first, "1").unwrap();
        let second = backup_path(tmp.path(), stamp);
        std::fs::write(&second, "2").unwrap();
        let third = backup_path(tmp.path(), stamp);
        assert_eq!(
            first.file_name().unwrap(),
            "findings.jsonl.pre-import-20260930T120000Z"
        );
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }
}
