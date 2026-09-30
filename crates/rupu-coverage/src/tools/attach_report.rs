//! Attach a report to an existing summary finding: the one place a finding
//! line is rewritten rather than appended. Used by `rupu findings import`,
//! the one-time migration of reports that were written beside summary
//! findings before the full profile existed (spec: "Backfill").
//!
//! A report attaches whole or not at all: it goes through the same
//! validation, claim hashing and artifact store as `report_finding`, and a
//! finding that already has a report is never touched. The ledger is
//! replaced atomically under the ledger lock, after a byte-for-byte backup,
//! and every other line (including ones nothing can parse, and keys this
//! version does not know) is written back unchanged.

use crate::ledger::events::FindingRecord;
use crate::ledger::paths::CoveragePaths;
use crate::report::{FindingProfile, FindingReport, FindingWriteOptions};
use crate::tools::report_finding::{
    check_full_report, derived_fields, lock_findings, prepare_full_report, ReportFindingError,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

/// A report to attach to the finding `finding_id`.
#[derive(Debug, Clone)]
pub struct AttachItem {
    pub finding_id: String,
    pub report: FindingReport,
}

#[derive(Debug)]
pub enum AttachOutcome {
    /// Written (on a dry run: would be; the report passed validation).
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
}

#[derive(Debug)]
pub struct AttachBatch {
    /// One outcome per item, in input order.
    pub outcomes: Vec<AttachOutcome>,
    /// A copy of the ledger as it was before the rewrite. `None` when
    /// nothing was written (a dry run, or no item attached).
    pub backup: Option<PathBuf>,
}

pub fn attach_reports(
    paths: &CoveragePaths,
    items: Vec<AttachItem>,
    opts: &FindingWriteOptions,
    dry_run: bool,
) -> std::io::Result<AttachBatch> {
    let _lock = lock_findings(paths)?;
    let raw = match std::fs::read_to_string(&paths.findings) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = raw.lines().map(str::to_owned).collect();
    let mut at: HashMap<String, Vec<usize>> = HashMap::new();
    let mut records: HashMap<usize, FindingRecord> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if let Ok(r) = serde_json::from_str::<FindingRecord>(line) {
            at.entry(r.id.clone()).or_default().push(i);
            records.insert(i, r);
        }
    }
    let known: Vec<String> = at.keys().cloned().collect();

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut changed = false;
    let mut outcomes = Vec::with_capacity(items.len());
    for item in items {
        let outcome = match at.get(&item.finding_id).map(Vec::as_slice) {
            None => AttachOutcome::NotFound,
            Some([i]) if claimed.contains(i) => AttachOutcome::Duplicate,
            Some([i]) => {
                let record = &records[i];
                if record.report.is_some() || record.profile == FindingProfile::Full {
                    AttachOutcome::AlreadyHasReport
                } else if dry_run {
                    match check_full_report(&item.report, &known, opts) {
                        Ok(()) => {
                            claimed.insert(*i);
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                } else {
                    // Only a report that attaches claims the finding: a
                    // rejected one leaves it free for a later item.
                    match prepare_full_report(paths, item.report, &known, opts)
                        .and_then(|report| Ok(upgraded_line(&lines[*i], &report)?))
                    {
                        Ok(line) => {
                            claimed.insert(*i);
                            lines[*i] = line;
                            changed = true;
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                }
            }
            Some(_) => AttachOutcome::Duplicate,
        };
        outcomes.push(outcome);
    }

    let backup = if changed {
        Some(replace_ledger(paths, &lines)?)
    } else {
        None
    };
    Ok(AttachBatch { outcomes, backup })
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

/// Back the ledger up byte for byte, then replace it with `lines` via a
/// temp file and a rename, so a reader sees the old ledger or the new one,
/// never a mix. Returns the backup's path.
fn replace_ledger(paths: &CoveragePaths, lines: &[String]) -> std::io::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let base = paths
        .root
        .join(format!("findings.jsonl.pre-import-{stamp}"));
    let mut backup = base.clone();
    let mut n = 1;
    while backup.exists() {
        n += 1;
        backup = PathBuf::from(format!("{}-{n}", base.display()));
    }
    std::fs::copy(&paths.findings, &backup)?;

    let tmp = paths.root.join("findings.jsonl.import-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        for line in lines {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::metadata(&paths.findings)?.permissions())?;
    std::fs::rename(&tmp, &paths.findings)?;
    Ok(backup)
}
