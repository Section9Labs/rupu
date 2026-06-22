use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_coverage::{
    discover_targets, read_findings, CoveragePaths, FindingRecord, Severity,
};
use rupu_workspace::WorkspaceStore;
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/findings", get(list_findings))
}

/// A single finding plus its provenance (which workspace / project / coverage
/// target it was declared under). The `FindingRecord` is flattened so the
/// frontend sees the finding's own fields at the top level alongside the three
/// provenance keys.
#[derive(Debug, Clone, Serialize)]
pub struct FindingOut {
    /// Owning workspace id — target_ids can collide across workspaces.
    pub ws_id: String,
    /// Workspace path basename — display attribution / grouping key.
    pub project: String,
    /// Coverage target the finding belongs to.
    pub target_id: String,
    #[serde(flatten)]
    pub record: FindingRecord,
}

/// Per-severity counts plus the grand total.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FindingsSummary {
    pub total: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub info: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingsResponse {
    pub findings: Vec<FindingOut>,
    pub summary: FindingsSummary,
}

/// Sort rank for a severity: critical (highest) sorts first.
fn severity_rank(sev: Severity) -> u8 {
    match sev {
        Severity::Critical => 4,
        Severity::High => 3,
        Severity::Medium => 2,
        Severity::Low => 1,
        Severity::Info => 0,
    }
}

/// Pure transform over the collected findings: sort by severity (critical→info)
/// then `declared_at` DESC, and tally the per-severity summary. Factored out of
/// the handler so it can be unit-tested without a server.
fn build_response(mut findings: Vec<FindingOut>) -> FindingsResponse {
    findings.sort_by(|a, b| {
        // Severity descending (critical first), then declared_at descending.
        severity_rank(b.record.severity)
            .cmp(&severity_rank(a.record.severity))
            .then_with(|| b.record.declared_at.cmp(&a.record.declared_at))
    });

    let mut summary = FindingsSummary::default();
    for f in &findings {
        summary.total += 1;
        match f.record.severity {
            Severity::Critical => summary.critical += 1,
            Severity::High => summary.high += 1,
            Severity::Medium => summary.medium += 1,
            Severity::Low => summary.low += 1,
            Severity::Info => summary.info += 1,
        }
    }

    FindingsResponse { findings, summary }
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Workspace path basename, falling back to the full path.
fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// `GET /api/findings` — every finding across every registered workspace's
/// coverage targets, tagged with provenance, plus a per-severity summary.
///
/// Tolerant by design: a workspace whose path is gone, or a target whose
/// `findings.jsonl` is absent/unreadable, is skipped with a `warn!` rather than
/// failing the whole request. A missing registry yields an empty response.
async fn list_findings(State(s): State<AppState>) -> ApiResult<Json<FindingsResponse>> {
    let workspaces = store(&s).list().unwrap_or_default();

    let mut out: Vec<FindingOut> = Vec::new();
    for w in &workspaces {
        let wp = std::path::Path::new(&w.path);
        let targets = match discover_targets(wp) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(ws_id = %w.id, path = %w.path, error = %e, "discover_targets failed; skipping workspace");
                continue;
            }
        };
        let project = project_name(&w.path);
        for t in targets {
            let paths = CoveragePaths::new(wp, &t.target_id);
            let records = match read_findings(&paths) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(ws_id = %w.id, target_id = %t.target_id, error = %e, "failed to read findings; skipping target");
                    continue;
                }
            };
            for record in records {
                out.push(FindingOut {
                    ws_id: w.id.clone(),
                    project: project.clone(),
                    target_id: t.target_id.clone(),
                    record,
                });
            }
        }
    }

    Ok(Json(build_response(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rupu_coverage::{Attribution, FindingEvidence, FindingScope, Surface};

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_01KS19A4MQXP".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            surface: Surface::Workflow,
        }
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn finding(id: &str, severity: Severity, declared_at: &str) -> FindingOut {
        FindingOut {
            ws_id: "ws1".to_string(),
            project: "proj".to_string(),
            target_id: "tgt".to_string(),
            record: FindingRecord {
                id: id.to_string(),
                file_path: Some("src/a.rs".to_string()),
                line_range: Some([1, 10]),
                scope: FindingScope::Line,
                summary: "summary".to_string(),
                severity,
                concern_id: None,
                evidence: FindingEvidence {
                    code_excerpt: None,
                    rationale: "why".to_string(),
                    references: vec![],
                },
                declared_by: attribution(),
                declared_at: at(declared_at),
            },
        }
    }

    #[test]
    fn sorts_critical_to_info() {
        let input = vec![
            finding("a", Severity::Info, "2026-01-01T00:00:00Z"),
            finding("b", Severity::Critical, "2026-01-01T00:00:00Z"),
            finding("c", Severity::Medium, "2026-01-01T00:00:00Z"),
            finding("d", Severity::High, "2026-01-01T00:00:00Z"),
            finding("e", Severity::Low, "2026-01-01T00:00:00Z"),
        ];
        let resp = build_response(input);
        let order: Vec<Severity> = resp.findings.iter().map(|f| f.record.severity).collect();
        assert_eq!(
            order,
            vec![
                Severity::Critical,
                Severity::High,
                Severity::Medium,
                Severity::Low,
                Severity::Info,
            ]
        );
    }

    #[test]
    fn within_severity_sorts_declared_at_desc() {
        let input = vec![
            finding("older", Severity::High, "2026-01-01T00:00:00Z"),
            finding("newer", Severity::High, "2026-02-01T00:00:00Z"),
        ];
        let resp = build_response(input);
        let ids: Vec<&str> = resp.findings.iter().map(|f| f.record.id.as_str()).collect();
        assert_eq!(ids, vec!["newer", "older"]);
    }

    #[test]
    fn summary_counts_match_inputs() {
        let input = vec![
            finding("a", Severity::Critical, "2026-01-01T00:00:00Z"),
            finding("b", Severity::Critical, "2026-01-01T00:00:00Z"),
            finding("c", Severity::High, "2026-01-01T00:00:00Z"),
            finding("d", Severity::Medium, "2026-01-01T00:00:00Z"),
            finding("e", Severity::Low, "2026-01-01T00:00:00Z"),
            finding("f", Severity::Info, "2026-01-01T00:00:00Z"),
            finding("g", Severity::Info, "2026-01-01T00:00:00Z"),
        ];
        let resp = build_response(input);
        assert_eq!(
            resp.summary,
            FindingsSummary {
                total: 7,
                critical: 2,
                high: 1,
                medium: 1,
                low: 1,
                info: 2,
            }
        );
    }

    #[test]
    fn empty_yields_zero_summary() {
        let resp = build_response(vec![]);
        assert!(resp.findings.is_empty());
        assert_eq!(resp.summary, FindingsSummary::default());
        assert_eq!(resp.summary.total, 0);
    }
}
