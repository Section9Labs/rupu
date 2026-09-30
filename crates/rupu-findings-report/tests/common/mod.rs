#![allow(dead_code)]

use chrono::{DateTime, Utc};
use rupu_coverage::{
    Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingReport, FindingScope,
    Severity, Surface,
};
use rupu_findings_report::model::{ExportFinding, ExportInput};
use rupu_findings_report::number::assign_numbers;

const FIXTURE: &str =
    include_str!("../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json");

pub fn full_report() -> FindingReport {
    serde_json::from_str(FIXTURE).expect("valid_full.json parses")
}

pub fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

pub fn full_record(id: &str, sev: Severity, report: FindingReport) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::Repo,
        summary: "Notes are readable across accounts".into(),
        severity: sev,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "see report".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_01".into(),
            model: "claude-x".into(),
            surface: Surface::Workflow,
        },
        declared_at: ts("2026-03-01T10:00:00Z"),
        profile: FindingProfile::Full,
        report: Some(report),
    }
}

pub fn summary_record(id: &str, sev: Severity) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: Some("src/store/notes.rs".into()),
        line_range: Some([88, 97]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: "Lookup ignores the owner".into(),
        severity: sev,
        concern_id: Some("authz-idor".into()),
        evidence: FindingEvidence {
            code_excerpt: Some("store.find_by_id(id)".into()),
            rationale: "The id is the only key used.".into(),
            references: vec![
                "CWE-639".into(),
                "https://cwe.mitre.org/data/definitions/639.html".into(),
            ],
        },
        declared_by: Attribution {
            run_id: "run_02".into(),
            model: "claude-y".into(),
            surface: Surface::Agent,
        },
        declared_at: ts("2026-03-02T10:00:00Z"),
        profile: FindingProfile::Summary,
        report: None,
    }
}

pub fn input(ws: &str, workflow: Option<&str>, record: FindingRecord) -> ExportInput {
    ExportInput {
        ws_id: ws.into(),
        project: ws.into(),
        workflow_name: workflow.map(str::to_string),
        record,
    }
}

pub fn numbered(inputs: Vec<ExportInput>) -> Vec<ExportFinding> {
    assign_numbers(inputs, "SEC")
}

/// The fixture finding, numbered SEC-001.
pub fn full_finding() -> ExportFinding {
    numbered(vec![input(
        "notebin",
        Some("audit"),
        full_record("fnd_full", Severity::Critical, full_report()),
    )])
    .remove(0)
}
