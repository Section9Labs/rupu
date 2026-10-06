//! Record a verification verdict on a finding: a locked rewrite of its ledger
//! line that merges a `verification` block into the report it already has.
//!
//! Nothing else marks a finding verified: the reporting agent may not supply
//! its own `verification` (`report_finding` rejects it), and an imported
//! report may not either. A verification run reaches the finding by id, after
//! it was filed, and this is the one write it makes.
//!
//! It is the inverse of `attach_report`: that attaches a report to a finding
//! that has none, this touches only a finding that has one, and only its
//! `verification` key. The ledger is replaced by the same locked, atomic,
//! backed-up rewrite (`attach_report::with_findings_line_rewrite`): every
//! other line is written back byte for byte, and the line itself is edited as
//! JSON, so its unknown keys and its line terminator survive. A verdict
//! replaces any earlier one whole.
//!
//! Refused, with the ledger untouched: an id the ledger does not have (or has
//! twice), a finding without a report, a verdict of `unverified` (the state of
//! a finding nobody has verified, not a verdict), a verdict from the run
//! that filed the finding, and a verdict with a blank verifier run id.

use crate::ledger::paths::CoveragePaths;
use crate::report::{Verification, VerificationStatus};
use crate::tools::attach_report::{with_findings_line_rewrite, LineRewrite, RewriteKind};

/// A verdict on the finding `finding_id`.
#[derive(Debug, Clone)]
pub struct VerifyInput {
    pub finding_id: String,
    /// `Confirmed`, `Disputed` or `Inconclusive`; `Unverified` is refused.
    pub status: VerificationStatus,
    /// The verifier's run id. It may not be the run that filed the finding.
    pub by_run: String,
    /// The verifier's agent name.
    pub by_agent: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// No finding with this id in this ledger.
    #[error("no finding with that id in this ledger")]
    NotFound,
    /// The id is on more than one ledger line, so no one of them is the
    /// finding.
    #[error("that finding id is on more than one ledger line; refusing to pick one")]
    Ambiguous,
    /// The finding has no report to carry a verification (a summary finding).
    #[error("that finding has no report to verify")]
    NoReport,
    /// The verifier is the run that filed the finding.
    #[error("a finding cannot be verified by the run that filed it")]
    SelfVerification,
    /// `unverified` is not a verdict.
    #[error("a verdict must be confirmed, disputed or inconclusive, not unverified")]
    BadStatus,
    /// The verifier run id is blank. An empty `by_run` would pass both the
    /// self-verification refusal here and the evaluator's independence check
    /// (`Some("")` reads as a different run than the filer), so it is refused
    /// at this chokepoint. This only enforces non-emptiness; the caller
    /// (`finding.verify`) is responsible for filling it with the verifier's
    /// own run id, never a value from agent input.
    #[error("a verification must name the verifier's run id (by_run is blank)")]
    MissingVerifier,
    /// The ledger could not be locked, read or replaced (it is a symlink, it
    /// changed during the verification, ...). It is as it was.
    #[error("cannot rewrite the findings ledger: {0}")]
    Io(#[from] std::io::Error),
}

/// Merge `input` as the `verification` of the finding it names, under the
/// findings lock. The finding's other keys are untouched, and so is every
/// other ledger line.
pub fn verify_finding(paths: &CoveragePaths, input: &VerifyInput) -> Result<(), VerifyError> {
    verify_finding_with(paths, input, &mut || {})
}

/// [`verify_finding`] with a hook that runs after the replacement ledger is
/// staged and before it is renamed into place: the test seam
/// `attach_report::attach_reports_with` has, to check the lock is held then.
fn verify_finding_with(
    paths: &CoveragePaths,
    input: &VerifyInput,
    before_rename: &mut dyn FnMut(),
) -> Result<(), VerifyError> {
    // Before any lock or file: a refusal that needs no ledger leaves it alone.
    if input.status == VerificationStatus::Unverified {
        return Err(VerifyError::BadStatus);
    }
    // A blank verifier run id would defeat both the self-verification guard
    // below and the evaluator's independence check; refuse it here.
    if input.by_run.trim().is_empty() {
        return Err(VerifyError::MissingVerifier);
    }
    let outcome = with_findings_line_rewrite(
        paths,
        RewriteKind::Verify,
        |record| record.id == input.finding_id,
        |record, line| {
            // The raw line, not `record.report`: a report this build cannot
            // parse (a newer one's) loads as `None`, but is still a report.
            let Some(report) = line.get_mut("report").and_then(|r| r.as_object_mut()) else {
                return Err(VerifyError::NoReport);
            };
            if record.declared_by.run_id == input.by_run {
                return Err(VerifyError::SelfVerification);
            }
            let verification = serde_json::to_value(Verification {
                status: input.status,
                by_run: Some(input.by_run.clone()),
                by_agent: input.by_agent.clone(),
                notes: input.notes.clone(),
            })
            .map_err(|e| VerifyError::Io(e.into()))?;
            report.insert("verification".into(), verification);
            Ok(())
        },
        before_rename,
    )?;
    match outcome {
        LineRewrite::Replaced => Ok(()),
        LineRewrite::NoMatch => Err(VerifyError::NotFound),
        LineRewrite::Ambiguous => Err(VerifyError::Ambiguous),
        LineRewrite::Refused(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingScope, Surface};
    use crate::report::{FindingProfile, FindingReport, FindingWriteOptions};
    use crate::tools::report_finding::{report_finding, ReportFindingInput};

    fn seed_full(paths: &CoveragePaths, run_id: &str) -> String {
        let report: FindingReport = serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let input = ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: None,
            severity: None,
            concern_id: None,
            evidence: None,
            report: Some(report),
            asset: None,
            tags: Vec::new(),
        };
        let attribution = Attribution {
            run_id: run_id.into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        };
        let opts = FindingWriteOptions::default().with_profile(FindingProfile::Full);
        report_finding(paths, attribution, input, &opts).unwrap().id
    }

    fn input(id: &str) -> VerifyInput {
        VerifyInput {
            finding_id: id.into(),
            status: VerificationStatus::Confirmed,
            by_run: "run_B".into(),
            by_agent: None,
            notes: None,
        }
    }

    #[test]
    fn the_ledger_lock_is_held_from_read_to_rename_and_released_after() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
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
        verify_finding_with(&paths, &input(&id), &mut || {
            probed = true;
            assert!(
                matches!(try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "the ledger lock must still be held right before the rename"
            );
        })
        .unwrap();
        assert!(probed, "the hook ran, so the ledger was rewritten");
        assert!(
            try_lock().is_ok(),
            "the lock is released when the verification returns"
        );
    }

    #[test]
    fn an_append_waits_for_a_verification_in_flight_and_lands_after_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let staged_len = std::fs::metadata(&paths.findings).unwrap().len();

        std::thread::scope(|s| {
            let mut appender = None;
            verify_finding_with(&paths, &input(&id), &mut || {
                // The verification is staged and holds the lock: an append
                // started now must wait for it rather than land in the ledger
                // about to be replaced (which the length check would then
                // abandon the verification over).
                appender = Some(s.spawn(|| seed_full(&paths, "run_A")));
                std::thread::sleep(std::time::Duration::from_millis(300));
                assert!(
                    !appender.as_ref().unwrap().is_finished(),
                    "the append must be waiting on the lock"
                );
                assert_eq!(
                    std::fs::metadata(&paths.findings).unwrap().len(),
                    staged_len,
                    "nothing may have been appended yet"
                );
            })
            .expect("no writer got past the lock, so the length guard holds");
            appender.unwrap().join().unwrap();
        });

        let records = crate::ledger::views::read_findings(&paths).unwrap();
        assert_eq!(records.len(), 2, "the verified finding and the new one");
        assert!(records[0].report.as_ref().unwrap().verification.is_some());
        assert!(records[1].report.as_ref().unwrap().verification.is_none());
    }

    #[test]
    fn a_writer_that_ignores_the_lock_abandons_the_verification_without_loss() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");
        let id = seed_full(&paths, "run_A");
        let original = std::fs::read(&paths.findings).unwrap();
        let appended = b"{\"from\":\"a writer that ignores the lock\"}\n";

        let err = verify_finding_with(&paths, &input(&id), &mut || {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&paths.findings)
                .unwrap()
                .write_all(appended)
                .unwrap();
        })
        .unwrap_err();
        match err {
            VerifyError::Io(e) => {
                assert!(e.to_string().contains("changed during verification"), "{e}")
            }
            e => panic!("expected the abandoned rewrite, got {e:?}"),
        }
        let mut expected = original;
        expected.extend_from_slice(appended);
        assert_eq!(std::fs::read(&paths.findings).unwrap(), expected);
        let leftovers: Vec<_> = std::fs::read_dir(&paths.root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.contains("pre-verify") || n.contains("verify-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
