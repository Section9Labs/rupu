//! Goal evaluation: is an objective's measurable target met by the evidence
//! on disk? Finding goals are a pure predicate over the findings ledger;
//! asset goals are added alongside.

use crate::def::{FindingSelector, Goal};
use rupu_coverage::{read_findings, ActiveSet, CoveragePaths, FindingRecord, VerificationStatus};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GoalEvalError {
    #[error("reading evidence: {0}")]
    Io(String),
    #[error("goal `{0}`: {1}")]
    Bad(String, String),
}

/// The result of evaluating one [`Goal`] against the evidence on disk.
#[derive(Debug, Clone)]
pub struct GoalOutcome {
    pub id: String,
    pub met: bool,
    pub current: u64,
    pub target: u64,
    pub detail: String,
}

/// A finding is verified only when its report carries a `Confirmed`
/// verification. A record with no report (Summary profile) or no verification
/// block is unverified.
fn is_verified(rec: &FindingRecord) -> bool {
    rec.report
        .as_ref()
        .and_then(|r| r.verification.as_ref())
        .map(|v| v.status == VerificationStatus::Confirmed)
        .unwrap_or(false)
}

/// Whether the finding's report lists `class_id` among its classifications
/// (legacy `cwe` entries folded in). A record with no report matches nothing.
fn finding_has_classification(rec: &FindingRecord, class_id: &str) -> bool {
    match rec.report.as_ref() {
        Some(report) => report
            .all_classifications()
            .iter()
            .any(|c| c.id == class_id),
        None => false,
    }
}

/// Count the findings matching `sel`, optionally only the verified ones.
/// A selector with no classification matches every finding.
pub fn count_matching_findings(
    records: &[FindingRecord],
    sel: &FindingSelector,
    verified: bool,
) -> u64 {
    records
        .iter()
        .filter(|r| match &sel.classification {
            Some(id) => finding_has_classification(r, id),
            None => true,
        })
        .filter(|r| !verified || is_verified(r))
        .count() as u64
}

pub struct GoalEvaluator;

impl GoalEvaluator {
    /// Evaluate `goal` against the ledgers under `paths`. Finding goals count
    /// matching findings; asset goals are evaluated against `active`.
    pub fn evaluate(
        goal: &Goal,
        paths: &CoveragePaths,
        active: &ActiveSet,
    ) -> Result<GoalOutcome, GoalEvalError> {
        if let Some(sel) = &goal.target.findings {
            let recs = read_findings(paths).map_err(|e| GoalEvalError::Io(e.to_string()))?;
            let target = goal.target.count_gte.unwrap_or(1);
            let current = count_matching_findings(&recs, sel, goal.target.verified);
            return Ok(GoalOutcome {
                id: goal.id.clone(),
                met: current >= target,
                current,
                target,
                detail: format!(
                    "{}/{} {}findings{}",
                    current,
                    target,
                    if goal.target.verified {
                        "verified "
                    } else {
                        ""
                    },
                    sel.classification
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                ),
            });
        }
        // asset arm: Task 4
        Self::evaluate_asset(goal, paths, active)
    }

    /// TEMPORARY placeholder; Task 4 replaces this with the asset predicate.
    fn evaluate_asset(
        goal: &Goal,
        _paths: &CoveragePaths,
        _active: &ActiveSet,
    ) -> Result<GoalOutcome, GoalEvalError> {
        Err(GoalEvalError::Bad(
            goal.id.clone(),
            "asset goals land in Task 4".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingReport, FindingScope,
        Severity, Surface, VerificationStatus,
    };
    use serde_json::json;

    /// Build a report through its serde shape (the type is large and strict;
    /// every required field is filled with a sentinel or a short string).
    fn mk_report(classification: &str, verified: Option<VerificationStatus>) -> FindingReport {
        let mut v = json!({
            "title": "t",
            "ownership": {
                "owner": "Unknown", "product": "p",
                "affected_component": "c", "source_repository": "r"
            },
            "tickets": "None Provided",
            "rating": {
                "impact": "High", "likelihood": "High", "risk_rating": "High",
                "risk_factor": "High", "cvss_v3": "Unknown"
            },
            "category": "cat",
            "attack_vector": "av",
            "description": "d",
            "impact": "i",
            "location": { "input": "in", "output": "out" },
            "root_cause": "rc",
            "call_chain": "None",
            "evidence": [],
            "remediation": "rem",
            "recommended_patch": "None",
            "ci_cd_detection": "None",
            "regression_test": "None",
            "replication_steps": [],
            "cross_references": "None",
            "references": "none",
            "classifications": [
                { "system": "CWE", "id": classification }
            ]
        });
        if let Some(status) = verified {
            v["verification"] = json!({ "status": status });
        }
        serde_json::from_value(v).expect("test report parses")
    }

    /// `report: None` models a Summary-profile record; otherwise a Full record
    /// carrying one classification and an optional verification.
    fn mk_finding(
        classification: Option<&str>,
        verified: Option<VerificationStatus>,
    ) -> FindingRecord {
        let report = classification.map(|c| mk_report(c, verified));
        // A verification without a classification is not a shape these tests
        // need; a `None` classification means "no report at all".
        FindingRecord {
            id: "fnd_test".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "s".into(),
            severity: Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_1".into(),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc::now(),
            profile: if report.is_some() {
                FindingProfile::Full
            } else {
                FindingProfile::Summary
            },
            report,
        }
    }

    #[test]
    fn counts_only_verified_findings_of_the_classification() {
        let recs = vec![
            mk_finding(Some("CWE-94"), Some(VerificationStatus::Confirmed)),
            mk_finding(Some("CWE-94"), Some(VerificationStatus::Unverified)),
            mk_finding(Some("CWE-94"), None), // full report, no verification block
            mk_finding(None, None),           // Summary profile, report: None
            mk_finding(Some("CWE-79"), Some(VerificationStatus::Confirmed)),
        ];
        let sel = crate::def::FindingSelector {
            classification: Some("CWE-94".into()),
        };
        // only the confirmed CWE-94
        assert_eq!(count_matching_findings(&recs, &sel, true), 1);
        // all CWE-94 regardless of verification (the report-less record has no classification)
        assert_eq!(count_matching_findings(&recs, &sel, false), 3);
    }
}
