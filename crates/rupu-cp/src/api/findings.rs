use crate::{error::ApiResult, state::AppState};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::{discover_targets, read_findings, CoveragePaths, FindingRecord, Severity};
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// Workflow that declared this finding, joined from the orchestrator
    /// `RunStore` via `declared_by.run_id`. `None` when the run can't be
    /// resolved (e.g. an agent/session-local id with no `run.json`).
    pub workflow_name: Option<String>,
    #[serde(flatten)]
    pub record: FindingRecord,
}

/// Optional query filters for `GET /api/findings`.
///
/// Plain `Option<String>` fields (NOT `#[serde(flatten)]`): serde_urlencoded
/// — axum's `Query` extractor — cannot deserialize through a flattened struct,
/// so the filters are inlined as string options.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FindingsQuery {
    /// Keep only findings from this workspace id.
    pub ws_id: Option<String>,
    /// Keep only findings whose joined `workflow_name` matches.
    pub workflow: Option<String>,
    /// Keep only findings whose `declared_by.run_id` matches.
    pub run_id: Option<String>,
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

/// Pure filter + sort + summarize step over findings that already carry their
/// joined `workflow_name`. Applies the optional `ws_id` / `workflow` / `run_id`
/// scope (a finding must pass every provided filter),
/// then sorts (severity critical→info, then `declared_at` DESC) and tallies the
/// per-severity summary over the FILTERED set. Server-free so it can be
/// unit-tested directly. The run_id→workflow join (which needs `RunStore`)
/// stays in the handler.
fn scope_and_summarize(findings: Vec<FindingOut>, q: &FindingsQuery) -> FindingsResponse {
    let filtered: Vec<FindingOut> = findings
        .into_iter()
        .filter(|f| match &q.ws_id {
            Some(ws) => &f.ws_id == ws,
            None => true,
        })
        .filter(|f| match &q.workflow {
            Some(wf) => f.workflow_name.as_deref() == Some(wf.as_str()),
            None => true,
        })
        .filter(|f| match &q.run_id {
            Some(rid) => &f.record.declared_by.run_id == rid,
            None => true,
        })
        .collect();
    build_response(filtered)
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
async fn list_findings(
    State(s): State<AppState>,
    Query(q): Query<FindingsQuery>,
) -> ApiResult<Json<FindingsResponse>> {
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
                    workflow_name: None,
                    record,
                });
            }
        }
    }

    // Join `declared_by.run_id → workflow_name` via the RunStore. Load each
    // distinct run id once; a load error / NotFound leaves that id out of the
    // map (finding keeps `workflow_name: None`).
    let mut wf_by_run: HashMap<String, String> = HashMap::new();
    for f in &out {
        let run_id = &f.record.declared_by.run_id;
        if run_id.is_empty() || wf_by_run.contains_key(run_id) {
            continue;
        }
        if let Ok(rec) = s.run_store.load(run_id) {
            wf_by_run.insert(run_id.clone(), rec.workflow_name);
        }
    }
    for f in &mut out {
        f.workflow_name = wf_by_run.get(&f.record.declared_by.run_id).cloned();
    }

    Ok(Json(scope_and_summarize(out, &q)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rupu_coverage::{Attribution, FindingEvidence, FindingScope, Surface};

    fn attribution() -> Attribution {
        attribution_run("run_01KS19A4MQXP")
    }

    fn attribution_run(run_id: &str) -> Attribution {
        Attribution {
            run_id: run_id.to_string(),
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
        finding_in("ws1", None, id, severity, declared_at)
    }

    /// Like `finding`, but lets a test pin the owning workspace and the joined
    /// `workflow_name` so the scope/summary filters can be exercised.
    fn finding_in(
        ws_id: &str,
        workflow_name: Option<&str>,
        id: &str,
        severity: Severity,
        declared_at: &str,
    ) -> FindingOut {
        FindingOut {
            ws_id: ws_id.to_string(),
            project: "proj".to_string(),
            target_id: "tgt".to_string(),
            workflow_name: workflow_name.map(|s| s.to_string()),
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

    /// Like `finding`, but pins the `declared_by.run_id` so the run_id filter
    /// can be exercised.
    fn finding_run(run_id: &str, id: &str, severity: Severity, declared_at: &str) -> FindingOut {
        let mut f = finding(id, severity, declared_at);
        f.record.declared_by = attribution_run(run_id);
        f
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

    /// Two workspaces' worth of findings, with workflow_name pre-attached as the
    /// handler would after the RunStore join.
    fn mixed_findings() -> Vec<FindingOut> {
        vec![
            finding_in("ws1", Some("wfA"), "a", Severity::Critical, "2026-01-01T00:00:00Z"),
            finding_in("ws1", Some("wfB"), "b", Severity::High, "2026-01-02T00:00:00Z"),
            finding_in("ws2", Some("wfA"), "c", Severity::Medium, "2026-01-03T00:00:00Z"),
            finding_in("ws2", None, "d", Severity::Low, "2026-01-04T00:00:00Z"),
        ]
    }

    #[test]
    fn no_filter_keeps_all() {
        let q = FindingsQuery::default();
        let resp = scope_and_summarize(mixed_findings(), &q);
        assert_eq!(resp.findings.len(), 4);
        assert_eq!(resp.summary.total, 4);
        assert_eq!(resp.summary.critical, 1);
        assert_eq!(resp.summary.high, 1);
        assert_eq!(resp.summary.medium, 1);
        assert_eq!(resp.summary.low, 1);
    }

    #[test]
    fn ws_id_filter_scopes_findings_and_summary() {
        let q = FindingsQuery {
            ws_id: Some("ws2".to_string()),
            workflow: None,
            run_id: None,
        };
        let resp = scope_and_summarize(mixed_findings(), &q);
        let ids: Vec<&str> = resp.findings.iter().map(|f| f.record.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "d"]);
        assert!(resp.findings.iter().all(|f| f.ws_id == "ws2"));
        // Summary reflects only the ws2 subset: 1 medium + 1 low.
        assert_eq!(resp.summary.total, 2);
        assert_eq!(resp.summary.medium, 1);
        assert_eq!(resp.summary.low, 1);
        assert_eq!(resp.summary.critical, 0);
        assert_eq!(resp.summary.high, 0);
    }

    #[test]
    fn workflow_filter_matches_attached_name_and_excludes_none() {
        let q = FindingsQuery {
            ws_id: None,
            workflow: Some("wfA".to_string()),
            run_id: None,
        };
        let resp = scope_and_summarize(mixed_findings(), &q);
        let ids: Vec<&str> = resp.findings.iter().map(|f| f.record.id.as_str()).collect();
        // "a" (ws1/wfA) + "c" (ws2/wfA); "b" is wfB, "d" is None — both excluded.
        assert_eq!(ids, vec!["a", "c"]);
        assert!(resp
            .findings
            .iter()
            .all(|f| f.workflow_name.as_deref() == Some("wfA")));
        assert_eq!(resp.summary.total, 2);
        assert_eq!(resp.summary.critical, 1);
        assert_eq!(resp.summary.medium, 1);
    }

    #[test]
    fn workflow_filter_excludes_findings_without_workflow_name() {
        // A workflow filter set to a name only the `None` finding could match
        // must exclude the `None` finding (None never equals Some).
        let input = vec![finding_in(
            "ws1",
            None,
            "x",
            Severity::Info,
            "2026-01-01T00:00:00Z",
        )];
        let q = FindingsQuery {
            ws_id: None,
            workflow: Some("anything".to_string()),
            run_id: None,
        };
        let resp = scope_and_summarize(input, &q);
        assert!(resp.findings.is_empty());
        assert_eq!(resp.summary.total, 0);
    }

    /// Two runs' worth of findings so the run_id filter has something to scope.
    fn run_findings() -> Vec<FindingOut> {
        vec![
            finding_run("runA", "a", Severity::Critical, "2026-01-01T00:00:00Z"),
            finding_run("runA", "b", Severity::High, "2026-01-02T00:00:00Z"),
            finding_run("runB", "c", Severity::Medium, "2026-01-03T00:00:00Z"),
        ]
    }

    #[test]
    fn run_id_filter_scopes_findings_and_summary() {
        let q = FindingsQuery {
            ws_id: None,
            workflow: None,
            run_id: Some("runA".to_string()),
        };
        let resp = scope_and_summarize(run_findings(), &q);
        let ids: Vec<&str> = resp.findings.iter().map(|f| f.record.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert!(resp
            .findings
            .iter()
            .all(|f| f.record.declared_by.run_id == "runA"));
        // Summary reflects only the runA subset: 1 critical + 1 high.
        assert_eq!(resp.summary.total, 2);
        assert_eq!(resp.summary.critical, 1);
        assert_eq!(resp.summary.high, 1);
        assert_eq!(resp.summary.medium, 0);
    }

    #[test]
    fn run_id_filter_with_no_match_is_empty() {
        let q = FindingsQuery {
            ws_id: None,
            workflow: None,
            run_id: Some("nope".to_string()),
        };
        let resp = scope_and_summarize(run_findings(), &q);
        assert!(resp.findings.is_empty());
        assert_eq!(resp.summary, FindingsSummary::default());
        assert_eq!(resp.summary.total, 0);
    }

    #[test]
    fn findings_query_deserializes_from_uri() {
        let uri: axum::http::Uri = "http://x/?ws_id=ws9&workflow=wfZ&run_id=run7"
            .parse()
            .unwrap();
        let Query(q) = Query::<FindingsQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(q.ws_id.as_deref(), Some("ws9"));
        assert_eq!(q.workflow.as_deref(), Some("wfZ"));
        assert_eq!(q.run_id.as_deref(), Some("run7"));

        let empty: axum::http::Uri = "http://x/".parse().unwrap();
        let Query(q2) = Query::<FindingsQuery>::try_from_uri(&empty).unwrap();
        assert_eq!(q2.ws_id, None);
        assert_eq!(q2.workflow, None);
        assert_eq!(q2.run_id, None);
    }
}
