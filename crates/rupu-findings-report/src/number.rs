use crate::model::{ExportFinding, ExportInput};
use rupu_coverage::Severity;
use std::collections::{BTreeMap, HashMap};

fn rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
        Severity::Info => 4,
    }
}

/// Number findings per project: severity (critical first), then declared_at
/// ascending, then id. Output is grouped by project (ws_id order) and in
/// number order within each project.
pub fn assign_numbers(all: Vec<ExportInput>, prefix: &str) -> Vec<ExportFinding> {
    let mut by_ws: BTreeMap<String, Vec<ExportInput>> = BTreeMap::new();
    for i in all {
        by_ws.entry(i.ws_id.clone()).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, mut items) in by_ws {
        items.sort_by(|a, b| {
            rank(a.record.severity)
                .cmp(&rank(b.record.severity))
                .then(a.record.declared_at.cmp(&b.record.declared_at))
                .then(a.record.id.cmp(&b.record.id))
        });
        for (n, input) in items.into_iter().enumerate() {
            out.push(ExportFinding {
                number: format!("{prefix}-{:03}", n + 1),
                input,
            });
        }
    }
    out
}

pub fn number_map(findings: &[ExportFinding]) -> HashMap<String, String> {
    findings
        .iter()
        .map(|f| (f.input.record.id.clone(), f.number.clone()))
        .collect()
}

/// The finding's display title: the report title for full findings, the
/// summary otherwise.
pub fn title(f: &ExportFinding) -> &str {
    f.input
        .record
        .report
        .as_ref()
        .map(|r| r.title.as_str())
        .unwrap_or(&f.input.record.summary)
}

pub fn filename(f: &ExportFinding, ext: &str) -> String {
    let cleaned: String = title(f)
        .chars()
        .filter(|c| {
            !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = collapsed.chars().take(80).collect();
    format!("{} - {}.{ext}", f.number, short.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExportInput;
    use rupu_coverage::{
        Attribution, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Severity,
        Surface,
    };

    fn rec(id: &str, sev: Severity, at: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: format!("Summary of {id}"),
            severity: sev,
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
            },
            declared_at: at.parse().unwrap(),
            profile: FindingProfile::Summary,
            report: None,
        }
    }
    fn input(ws: &str, r: FindingRecord) -> ExportInput {
        ExportInput {
            ws_id: ws.into(),
            project: ws.into(),
            workflow_name: None,
            record: r,
        }
    }

    #[test]
    fn numbers_per_project_by_severity_then_time() {
        let out = assign_numbers(
            vec![
                input("a", rec("fnd_low", Severity::Low, "2026-01-01T00:00:00Z")),
                input(
                    "a",
                    rec("fnd_crit_late", Severity::Critical, "2026-02-01T00:00:00Z"),
                ),
                input(
                    "a",
                    rec("fnd_crit_early", Severity::Critical, "2026-01-15T00:00:00Z"),
                ),
                input("b", rec("fnd_b", Severity::High, "2026-01-01T00:00:00Z")),
            ],
            "SEC",
        );
        let m = number_map(&out);
        assert_eq!(m["fnd_crit_early"], "SEC-001");
        assert_eq!(m["fnd_crit_late"], "SEC-002");
        assert_eq!(m["fnd_low"], "SEC-003");
        assert_eq!(m["fnd_b"], "SEC-001");
    }

    #[test]
    fn filename_sanitizes_title() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "Path: a/b\\c *weird* \"quoted\"   title".into();
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(
            filename(&f, "pdf"),
            "SEC-001 - Path abc weird quoted title.pdf"
        );
    }

    #[test]
    fn filename_truncates_long_titles() {
        let mut r = rec("fnd_x", Severity::High, "2026-01-01T00:00:00Z");
        r.summary = "x".repeat(200);
        let f = assign_numbers(vec![input("a", r)], "SEC").remove(0);
        assert_eq!(
            filename(&f, "md"),
            format!("SEC-001 - {}.md", "x".repeat(80))
        );
    }
}
