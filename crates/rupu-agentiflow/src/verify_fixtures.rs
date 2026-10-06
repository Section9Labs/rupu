//! Test-only fixtures for the verification-gate tests (`status_tools`, `run`):
//! a Full-profile finding carrying a report, which is the only kind of finding
//! a verdict can be recorded on.

use chrono::Utc;
use rupu_coverage::{
    Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingReport, FindingScope,
    Severity, Surface,
};
use serde_json::json;

/// A report classified `classification` (as a CWE), with no verification.
/// Built through its serde shape: the type is large and strict.
pub(crate) fn full_report(classification: &str) -> FindingReport {
    serde_json::from_value(json!({
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
        "classifications": [{ "system": "CWE", "id": classification }]
    }))
    .expect("test report parses")
}

/// A Full-profile finding `id`, filed by run `filed_by_run` (so that is the one
/// run that may NOT verify it), classified `classification`, unverified.
pub(crate) fn full_finding(id: &str, filed_by_run: &str, classification: &str) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::Repo,
        summary: "a full-profile finding".into(),
        severity: Severity::High,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: filed_by_run.into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: Some("recon".into()),
            provider: None,
        },
        declared_at: Utc::now(),
        profile: FindingProfile::Full,
        report: Some(full_report(classification)),
    }
}
