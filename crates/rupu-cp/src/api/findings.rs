use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use rupu_coverage::report::{
    raster_image_type, summarize, ArtifactKind, ArtifactStorage, ArtifactStore, ReportSummary,
};
use rupu_coverage::{
    discover_targets, read_declared_findings, CoveragePaths, FindingRecord, Severity,
};
use rupu_findings_report::model::{ExportFinding, ExportInput, ReportMeta};
use rupu_findings_report::number::{
    assign_numbers, filename, fit_file_name, is_valid_prefix, number_map, sanitize_title,
    DEFAULT_PREFIX,
};
use rupu_findings_report::select::{describe, parse_cwe, select, Selection};
use rupu_findings_report::{
    render_finding, render_project, render_split_zip, Blobs, ExportError, Format,
};
use rupu_orchestrator::runs::RunStore;
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings", get(list_findings))
        .route("/api/findings/export", post(export_findings))
        .route("/api/findings/:id", get(get_finding))
        .route("/api/findings/:id/export", get(export_finding))
        .route("/api/findings/:id/artifacts/:sha256", get(get_artifact))
        .route("/api/findings/artifacts/:sha256", get(get_artifact_blob))
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
    /// Deep link to the finding's location on the SCM's web UI (github/gitlab
    /// blob URL at the recorded line range), derived from the owning
    /// workspace's `repo_remote` + `initial_branch`. `None` when the
    /// workspace has no remote, the host is unrecognized, or the finding has
    /// no `file_path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
    /// Present for full-profile findings in LIST responses: the fields a row
    /// needs, without the report body (which `GET /api/findings/:id` serves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_summary: Option<ReportSummary>,
    /// Declaring agent instance's codename (`declared_by.codename`), or the
    /// crew derived from `declared_by.run_id` for a legacy finding.
    pub codename: String,
    pub codename_derived: bool,
    #[serde(flatten)]
    pub record: FindingRecord,
}

/// The one slimming rule for finding lists: the record without its report
/// body, plus the summary a row needs (present only when the record had a
/// report). Shared by `/api/findings` rows and `/api/coverage/:target`.
pub(crate) fn slim(mut record: FindingRecord) -> (FindingRecord, Option<ReportSummary>) {
    let summary = record.report.take().as_ref().map(summarize);
    (record, summary)
}

impl FindingOut {
    /// The list-endpoint shape: summary fields in, report body out.
    pub(crate) fn into_list_row(mut self) -> Self {
        let (record, summary) = slim(self.record);
        self.record = record;
        self.report_summary = summary;
        self
    }
}

/// How a report evidence claim's file compares with the copy on disk now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimState {
    /// The file still hashes to the value recorded with the claim.
    Current,
    /// The file exists but its contents changed since the finding was recorded.
    Changed,
    /// The file is gone (or escapes the workspace).
    Missing,
    /// No file or no recorded hash to compare (binary targets, summary claims).
    Unknown,
}

/// Largest file a detail request will hash to judge an evidence claim's
/// staleness; a larger file reports [`ClaimState::Unknown`]. Evidence claims
/// cite source files, so this is far below the 500 MiB artifact cap: every
/// `GET /api/findings/:id` re-hashes every claim's file, and a request must not
/// be able to make the server read hundreds of megabytes per claim.
pub(crate) const CLAIM_HASH_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// One [`ClaimState`] per `report.evidence[i]`, comparing each claim's recorded
/// `sha256` against the file's current contents under `workspace`. Files larger
/// than [`CLAIM_HASH_MAX_BYTES`] are not hashed (`Unknown`). Synchronous and
/// potentially slow: async callers must run it under `spawn_blocking`.
pub(crate) fn claim_states(
    workspace: &std::path::Path,
    r: &rupu_coverage::FindingReport,
) -> Vec<ClaimState> {
    claim_states_capped(workspace, r, CLAIM_HASH_MAX_BYTES)
}

fn claim_states_capped(
    workspace: &std::path::Path,
    r: &rupu_coverage::FindingReport,
    max_bytes: u64,
) -> Vec<ClaimState> {
    r.evidence
        .iter()
        .map(|c| match (&c.file, &c.sha256) {
            (Some(file), Some(recorded)) => {
                match crate::api::source::resolve_under_workspace(workspace, file) {
                    Ok(p) if p.is_file() => match std::fs::metadata(&p) {
                        Ok(m) if m.len() > max_bytes => ClaimState::Unknown,
                        Ok(_) => match rupu_coverage::report::sha256_file(&p) {
                            Ok(now) if &now == recorded => ClaimState::Current,
                            Ok(_) => ClaimState::Changed,
                            Err(_) => ClaimState::Missing,
                        },
                        Err(_) => ClaimState::Missing,
                    },
                    _ => ClaimState::Missing,
                }
            }
            _ => ClaimState::Unknown,
        })
        .collect()
}

/// `GET /api/findings/:id` — one finding with its full report body and the
/// staleness of each evidence claim.
#[derive(Debug, Serialize)]
pub struct FindingDetail {
    #[serde(flatten)]
    pub finding: FindingOut,
    pub evidence_status: Vec<ClaimState>,
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
    /// A findings query (`severity>=high tag:needs-poc ...`), applied after
    /// the `ws_id` / `workflow` / `run_id` scope. A bad one is a 400.
    pub q: Option<String>,
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
    /// Values in use per query key over the scope (ws/workflow/run), before
    /// `q`: autocomplete and the severity tiles.
    pub facets: std::collections::BTreeMap<&'static str, Vec<rupu_coverage::FacetValue>>,
    /// Workspaces whose finding-tag log could not be read: their findings are
    /// served with declared tags only, and tag filters may miss them.
    pub tags_unavailable: Vec<TagsUnavailable>,
}

/// A workspace whose finding-tag log could not be read: its id, and the
/// project name (the workspace path's basename — what `FindingOut.project`
/// carries) so a client can name it even when no row of it is in the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagsUnavailable {
    pub ws_id: String,
    pub project: String,
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
/// joined `workflow_name`. Applies the optional `ws_id` / `workflow` scope plus
/// an optional run-id SET (a finding must pass every provided filter), then
/// sorts (severity critical→info, then `declared_at` DESC) and tallies the
/// per-severity summary over the FILTERED set. Server-free so it can be
/// unit-tested directly.
///
/// `run_ids` is the resolved match set — the parent run id UNIONED with every
/// `for_each` unit sub-run id of that parent (each fan-out unit is its own
/// sub-run). A finding whose `declared_by.run_id` is in the set is kept, which
/// is why fan-out findings (attributed to the unit's sub-run, not the parent)
/// survive the per-run view. Resolving that set needs `RunStore`, so the handler
/// builds it and passes it in here.
fn scope_by_run_set(
    findings: Vec<FindingOut>,
    run_ids: &Option<HashSet<String>>,
    ws_id: &Option<String>,
    workflow: &Option<String>,
) -> FindingsResponse {
    let filtered: Vec<FindingOut> = findings
        .into_iter()
        .filter(|f| match ws_id {
            Some(ws) => &f.ws_id == ws,
            None => true,
        })
        .filter(|f| match workflow {
            Some(wf) => f.workflow_name.as_deref() == Some(wf.as_str()),
            None => true,
        })
        .filter(|f| match run_ids {
            Some(ids) => ids.contains(&f.record.declared_by.run_id),
            None => true,
        })
        .collect();
    build_response(filtered)
}

/// Pure transform over the collected findings: sort by severity (critical→info)
/// then `declared_at` DESC, and tally the per-severity summary. Factored out of
/// the handler so it can be unit-tested without a server.
///
/// `pub(crate)`: also called by `LocalHostConnector::dashboard_summary`
/// (`host/local.rs`) to derive `findings_open` from `.summary.total` rather
/// than re-walking the findings store.
pub(crate) fn build_response(mut findings: Vec<FindingOut>) -> FindingsResponse {
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

    FindingsResponse {
        findings,
        summary,
        facets: Default::default(),
        tags_unavailable: Vec::new(),
    }
}

fn store_for(global_dir: &std::path::Path) -> WorkspaceStore {
    WorkspaceStore {
        root: global_dir.join("workspaces"),
    }
}

/// Workspace path basename, falling back to the full path.
fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Every registered workspace's coverage targets, with their findings ledger
/// read: `f(workspace, target_id, ledger paths, records)` once per target.
///
/// This is the one workspace/target walk. Tolerant by design: a workspace
/// whose path is gone, or a target whose `findings.jsonl` is absent/unreadable,
/// is skipped with a `warn!` rather than failing the caller. A workspace whose
/// tag log cannot be read is still walked (declared tags only) and it is
/// pushed to `tags_unavailable` (id + project name).
fn each_ledger(
    global_dir: &std::path::Path,
    mut f: impl FnMut(&rupu_workspace::Workspace, &str, CoveragePaths, Vec<FindingRecord>),
    tags_unavailable: &mut Vec<TagsUnavailable>,
) {
    let workspaces = store_for(global_dir).list().unwrap_or_default();
    for w in &workspaces {
        let wp = std::path::Path::new(&w.path);
        let targets = match discover_targets(wp) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(ws_id = %w.id, path = %w.path, error = %e, "discover_targets failed; skipping workspace");
                continue;
            }
        };
        // The tag log is workspace-wide: read it once here, not once per
        // target (`read_findings` would re-read it for every target, on
        // page-polled paths). Same tolerance as `read_findings`: an
        // unreadable log is warned about and findings keep declared tags.
        let tag_log = rupu_coverage::TagLog::for_workspace(wp);
        let tag_events = match rupu_coverage::read_tag_events(&tag_log) {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(
                    ws_id = %w.id,
                    path = %tag_log.path.display(),
                    error = %e,
                    "cannot read the finding-tag log; showing declared tags only"
                );
                tags_unavailable.push(TagsUnavailable {
                    ws_id: w.id.clone(),
                    project: project_name(&w.path),
                });
                Vec::new()
            }
        };
        for t in targets {
            let paths = CoveragePaths::new(wp, &t.target_id);
            let mut records = match read_declared_findings(&paths) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(ws_id = %w.id, target_id = %t.target_id, error = %e, "failed to read findings; skipping target");
                    continue;
                }
            };
            rupu_coverage::fold_tags(&mut records, &tag_events);
            f(w, &t.target_id, paths, records);
        }
    }
}

/// Collect every finding across every registered workspace's coverage
/// targets, tagged with provenance (`ws_id` / `project` / `target_id`) but
/// WITHOUT the `RunStore` `workflow_name` join — `list_findings` does that
/// join itself afterward, since it needs `AppState.run_store`.
///
/// `pub`: this is the one workspace/target walk (via [`each_ledger`]).
/// `list_findings`, `LocalHostConnector::dashboard_summary`'s open-findings
/// count (`host/local.rs`) and the report exports (here, and `rupu findings
/// export`) all call it rather than each re-implementing the walk.
///
/// Tolerant by design: a workspace whose path is gone, or a target whose
/// `findings.jsonl` is absent/unreadable, is skipped with a `warn!` rather
/// than failing the caller.
pub fn collect_all_findings(global_dir: &std::path::Path) -> Vec<FindingOut> {
    collect_all_findings_reporting(global_dir).0
}

/// [`collect_all_findings`], plus the workspaces (id + project) whose finding-tag
/// log could not be read (their findings carry declared tags only).
pub fn collect_all_findings_reporting(
    global_dir: &std::path::Path,
) -> (Vec<FindingOut>, Vec<TagsUnavailable>) {
    let mut out: Vec<FindingOut> = Vec::new();
    let mut tags_unavailable: Vec<TagsUnavailable> = Vec::new();
    each_ledger(
        global_dir,
        |w, target_id, _paths, records| {
            let project = project_name(&w.path);
            for record in records {
                // `w` (the owning workspace) is already in hand for every
                // finding in this target — no separate lookup/memoization is
                // needed, unlike a flat finding list without provenance.
                let permalink = match (w.repo_remote.as_deref(), record.file_path.as_deref()) {
                    (Some(remote), Some(path)) => rupu_scm::weburl::repo_permalink(
                        remote,
                        w.initial_branch.as_deref(),
                        path,
                        record.line_range,
                    ),
                    _ => None,
                };
                let (codename, codename_derived) = crate::codename::named(
                    record.declared_by.codename.as_deref(),
                    &record.declared_by.run_id,
                    None,
                );
                out.push(FindingOut {
                    codename,
                    codename_derived,
                    ws_id: w.id.clone(),
                    project: project.clone(),
                    target_id: target_id.to_string(),
                    workflow_name: None,
                    permalink,
                    report_summary: None,
                    record,
                });
            }
        },
        &mut tags_unavailable,
    );
    (out, tags_unavailable)
}

/// Which ledger holds each finding: id → the coverage paths of every ledger
/// it appears in (normally exactly one). Read-only; `rupu findings import`
/// uses it to find the finding a report belongs to.
pub fn finding_ledgers(global_dir: &std::path::Path) -> HashMap<String, Vec<CoveragePaths>> {
    let mut out: HashMap<String, Vec<CoveragePaths>> = HashMap::new();
    each_ledger(
        global_dir,
        |_, _, paths, records| {
            for r in records {
                out.entry(r.id).or_default().push(paths.clone());
            }
        },
        &mut Vec::new(),
    );
    out
}

/// One workspace's part of a cross-workspace tag change.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceTagResult {
    pub ws_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<rupu_coverage::TagOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a tag change did across the registered workspaces. Each workspace's
/// batch is atomic; the whole is not.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TagAcrossResult {
    pub workspaces: Vec<WorkspaceTagResult>,
    /// Ids no registered workspace holds.
    pub unknown: Vec<String>,
}

/// Apply `change` to findings wherever they live: ids are grouped by the
/// registered workspace holding them and `rupu_coverage::apply` runs once
/// per workspace (spec "Batches that span workspaces"). An id found in two
/// distinct workspace paths is tagged in both. Used by `rupu findings tag`
/// and, in Plan 2, `POST /api/findings/tags`.
pub fn tag_findings_across(
    global_dir: &std::path::Path,
    change: &rupu_coverage::TagChange,
    by: &rupu_coverage::TagActor,
) -> Result<TagAcrossResult, rupu_coverage::TagError> {
    change.check()?;
    // finding id → each (ws_id, workspace path) holding it, once per path.
    let mut homes: HashMap<String, Vec<(String, std::path::PathBuf)>> = HashMap::new();
    each_ledger(
        global_dir,
        |w, _, _, records| {
            let path = std::path::PathBuf::from(&w.path);
            for r in records {
                let entry = homes.entry(r.id).or_default();
                if !entry.iter().any(|(_, p)| *p == path) {
                    entry.push((w.id.clone(), path.clone()));
                }
            }
        },
        &mut Vec::new(),
    );
    let mut out = TagAcrossResult::default();
    let mut by_ws: std::collections::BTreeMap<(String, std::path::PathBuf), Vec<String>> =
        std::collections::BTreeMap::new();
    for id in &change.finding_ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        match homes.get(id) {
            Some(hs) => {
                for h in hs {
                    by_ws.entry(h.clone()).or_default().push(id.to_string());
                }
            }
            None if !out.unknown.iter().any(|u| u == id) => out.unknown.push(id.to_string()),
            None => {}
        }
    }
    for ((ws_id, path), ids) in by_ws {
        let one = rupu_coverage::TagChange {
            finding_ids: ids,
            add: change.add.clone(),
            remove: change.remove.clone(),
        };
        let log = rupu_coverage::TagLog::for_workspace(&path);
        out.workspaces
            .push(match rupu_coverage::apply(&log, &one, by) {
                Ok(outcomes) => WorkspaceTagResult {
                    ws_id,
                    outcomes,
                    error: None,
                },
                Err(e) => WorkspaceTagResult {
                    ws_id,
                    outcomes: vec![],
                    error: Some(e.to_string()),
                },
            });
    }
    Ok(out)
}

/// The set of run ids a finding may be attributed to, for a Findings-tab query
/// scoped to `parent`. A `for_each` / `panel` unit runs as its own sub-run, so
/// its findings carry the UNIT's run id (`declared_by.run_id`), not `parent` —
/// a bare parent match drops them.
///
/// The set is `parent` unioned with every sub-run id we can resolve, from TWO
/// sources so a **running** run is covered, not just a finished one:
///   1. unit checkpoints — durable, but written only when a unit COMPLETES;
///   2. the parent's `events.jsonl` — written LIVE: each `UnitStarted` /
///      `StepWorking` carries the unit's `transcript_path`, whose file stem is
///      the unit's run id. This is what catches in-flight units (and panel
///      units, which are never checkpointed at all — see `graph.rs`).
///
/// Without (2), the Findings tab shows nothing for an in-progress run even
/// though the findings are already on disk.
pub fn resolve_run_scope(store: &RunStore, parent: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    set.insert(parent.to_string());
    for cp in store.read_unit_checkpoints(parent).unwrap_or_default() {
        set.insert(cp.run_id);
    }
    for id in sub_run_ids_from_events(store, parent) {
        set.insert(id);
    }
    set
}

/// Sub-run ids recovered from the parent's `events.jsonl` (written live). Each
/// `UnitStarted` (and `StepWorking`) carries the unit's `transcript_path`; its
/// file stem is the unit's run id — the id a finding declared by that unit
/// agent is attributed to. A transcript stored under the nested sub-run layout
/// (`.../<sub_id>/transcript.jsonl`) has a generic `transcript` stem, so fall
/// back to the parent directory name there. Missing/garbled file → empty.
fn sub_run_ids_from_events(store: &RunStore, parent: &str) -> Vec<String> {
    let Ok(body) = std::fs::read_to_string(store.events_path(parent)) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(rupu_orchestrator::runs::known_transcript_from_event_line)
        // Findings scope: step/unit transcripts only. A `dispatch_started`
        // child has no step id and was never part of this scope.
        .filter(|k| k.step_id.is_some())
        .map(|k| k.key)
        .collect()
}

/// `run_id → workflow_name` for each distinct, non-empty id in `run_ids` whose
/// run the `RunStore` can load. Each id is loaded once; an id that can't be
/// resolved (an agent/session-local id with no `run.json`) is simply absent.
fn workflow_names_by_run<'a>(
    store: &RunStore,
    run_ids: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for run_id in run_ids {
        if run_id.is_empty() || !seen.insert(run_id) {
            continue;
        }
        if let Ok(rec) = store.load(run_id) {
            names.insert(run_id.to_string(), rec.workflow_name);
        }
    }
    names
}

/// Fill `workflow_name` (joined via `declared_by.run_id`) on every row that
/// lacks one, loading each distinct run id once. A row that already carries
/// a name is left as it is; a run the `RunStore` can't load leaves `None`.
/// Callers join before [`query_findings`] so `workflow:` can match.
pub fn join_workflow_names(store: &RunStore, findings: &mut [FindingOut]) {
    let names = workflow_names_by_run(
        store,
        findings
            .iter()
            .filter(|f| f.workflow_name.is_none())
            .map(|f| f.record.declared_by.run_id.as_str()),
    );
    for f in findings.iter_mut().filter(|f| f.workflow_name.is_none()) {
        f.workflow_name = names.get(&f.record.declared_by.run_id).cloned();
    }
}

/// A finding as the query evaluator sees it: its record plus the project,
/// workspace and workflow this surface knows.
pub fn finding_view(f: &FindingOut) -> rupu_coverage::FindingView<'_> {
    rupu_coverage::FindingView {
        record: &f.record,
        project: Some(&f.project),
        ws_id: Some(&f.ws_id),
        workflow: f.workflow_name.as_deref(),
    }
}

/// The findings a parsed query selects, sorted severity (worst first), then
/// newest, then id. Expands each `run:` value to the run plus its sub-runs
/// ([`resolve_run_scope`]). It does NOT join `workflow_name`: callers run
/// [`join_workflow_names`] first, or `workflow:` matches nothing.
/// `rupu findings list|tags` and `GET /api/findings` both go through this.
pub fn query_findings(
    store: &RunStore,
    mut findings: Vec<FindingOut>,
    q: &rupu_coverage::ParsedQuery,
) -> Vec<FindingOut> {
    if !q.terms.is_empty() {
        findings = select_findings(store, findings, q);
    }
    findings.sort_by(|a, b| {
        rupu_coverage::severity_rank(b.record.severity)
            .cmp(&rupu_coverage::severity_rank(a.record.severity))
            .then_with(|| b.record.declared_at.cmp(&a.record.declared_at))
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    findings
}

/// [`query_findings`]' filter step, for a query with at least one term.
fn select_findings(
    store: &RunStore,
    mut findings: Vec<FindingOut>,
    q: &rupu_coverage::ParsedQuery,
) -> Vec<FindingOut> {
    let runs: rupu_coverage::RunScopes = rupu_coverage::run_values(q)
        .into_iter()
        .map(|r| {
            let scope = resolve_run_scope(store, &r);
            (r, scope)
        })
        .collect();
    // Ids repeat across workspaces and targets: key by all three.
    let key = |f: &FindingOut| format!("{}/{}/{}", f.ws_id, f.target_id, f.record.id);
    let keep: HashSet<String> = rupu_coverage::select(&findings, finding_view, q, &runs)
        .into_iter()
        .map(key)
        .collect();
    findings.retain(|f| keep.contains(&key(f)));
    findings
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
) -> Result<Json<FindingsResponse>, axum::response::Response> {
    use axum::response::IntoResponse;
    let parsed = rupu_coverage::parse_query(q.q.as_deref().unwrap_or("")).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": e.message,
                "token": e.token,
                "code": e.code,
                "start": e.start,
                "end": e.end,
            })),
        )
            .into_response()
    })?;
    let (mut out, tags_unavailable) = collect_all_findings_reporting(&s.global_dir);

    // Join `declared_by.run_id → workflow_name` once, for both the
    // `workflow` scope and `q`'s `workflow:` key. A run the store can't load
    // leaves the finding's `workflow_name: None`.
    join_workflow_names(&s.run_store, &mut out);

    // Resolve the run-id match SET when filtering by run: the parent unioned
    // with its sub-runs (each `for_each`/`panel` unit is its own sub-run, and
    // its findings carry the UNIT's run id). See [`resolve_run_scope`] — it
    // draws sub-run ids from checkpoints AND the live event stream, so a
    // RUNNING run's in-flight/panel findings aren't dropped. Only one level of
    // fan-out is resolved (a unit that itself fans out isn't followed).
    let run_ids: Option<HashSet<String>> = q
        .run_id
        .as_ref()
        .map(|parent| resolve_run_scope(&s.run_store, parent));

    let scoped = scope_by_run_set(out, &run_ids, &q.ws_id, &q.workflow).findings;
    // Facets describe the scope, before `q` narrows it.
    let facets = rupu_coverage::facets(scoped.iter().map(finding_view));
    // Warn only about the workspaces this request covers: the requested one,
    // or one with a finding in the scope.
    let tags_unavailable: Vec<TagsUnavailable> = tags_unavailable
        .into_iter()
        .filter(|w| q.ws_id.as_ref() == Some(&w.ws_id) || scoped.iter().any(|f| f.ws_id == w.ws_id))
        .collect();
    let filtered = query_findings(&s.run_store, scoped, &parsed);
    let mut resp = build_response(filtered);
    // List rows never carry the report body; full-profile rows get a summary.
    resp.findings = resp
        .findings
        .into_iter()
        .map(FindingOut::into_list_row)
        .collect();
    resp.facets = facets;
    resp.tags_unavailable = tags_unavailable;
    Ok(Json(resp))
}

/// Find one finding by id across every registered workspace. Synchronous and
/// potentially slow (it reads every coverage ledger): async callers must run
/// it under `spawn_blocking`.
fn find_finding(global: &std::path::Path, id: &str) -> Option<FindingOut> {
    collect_all_findings(global)
        .into_iter()
        .find(|f| f.record.id == id)
}

/// `GET /api/findings/:id` — the full finding (report body included) plus a
/// per-claim staleness verdict against the owning workspace's current files.
async fn get_finding(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let global = s.global_dir.clone();
    let id_for_lookup = id.clone();
    let found = tokio::task::spawn_blocking(move || find_finding(&global, &id_for_lookup))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut finding =
        found.ok_or_else(|| ApiError::not_found(format!("finding {id} not found")))?;
    if let Ok(run) = s.run_store.load(&finding.record.declared_by.run_id) {
        finding.workflow_name = Some(run.workflow_name);
    }
    let evidence_status = match (
        finding.record.report.clone(),
        crate::api::code::load_workspace(&s, &finding.ws_id),
    ) {
        // Hashing reads arbitrary workspace files, so keep it off the runtime.
        (Some(report), Ok(ws)) => {
            let root = std::path::PathBuf::from(ws.path);
            tokio::task::spawn_blocking(move || claim_states(&root, &report))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
        }
        (Some(report), Err(_)) => vec![ClaimState::Unknown; report.evidence.len()],
        (None, _) => Vec::new(),
    };
    let mut body = serde_json::to_value(FindingDetail {
        finding,
        evidence_status,
    })
    .map_err(|e| ApiError::internal(e.to_string()))?;
    add_hex_siblings(&mut body);
    Ok(Json(body))
}

/// `0x` + lowercase hex of `n`, unpadded: the exact text for a 64-bit address.
fn hex_string(n: u64) -> String {
    format!("0x{n:x}")
}

/// Adds exact string siblings for the 64-bit numbers in a serialized finding
/// body: `address_hex` on every disasm listing line and `base_hex` on every
/// hexdump block (`report.blocks`). The numeric `address` / `base` stay as
/// they are, but a JSON number above 2^53 is rounded by `JSON.parse` in the
/// browser, so clients display the string. A wire-only transform: the stored
/// report and its schema are untouched.
fn add_hex_siblings(body: &mut serde_json::Value) {
    use serde_json::Value;
    let Some(blocks) = body
        .get_mut("report")
        .and_then(|r| r.get_mut("blocks"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for block in blocks {
        match block.get("kind").and_then(Value::as_str) {
            Some("hexdump") => {
                if let Some(base) = block.get("base").and_then(Value::as_u64) {
                    block["base_hex"] = Value::String(hex_string(base));
                }
            }
            Some("disasm") => {
                let lines = block.get_mut("listing").and_then(Value::as_array_mut);
                for line in lines.into_iter().flatten() {
                    if let Some(addr) = line.get("address").and_then(Value::as_u64) {
                        line["address_hex"] = Value::String(hex_string(addr));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The `filename` an artifact is offered under: the last `/`-separated segment
/// of its recorded path, with `"`, `\` and control characters replaced by `_`
/// so the value can never break out of the quoted `Content-Disposition`
/// parameter.
fn safe_filename(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "artifact".to_string()
    } else {
        cleaned
    }
}

/// Read size for streaming an artifact body (tokio's default is 4 KiB, which
/// is a syscall and an allocation per tiny chunk for multi-hundred-MB files).
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Open `path`, confirm the handle is a regular file whose contents hash to
/// `expected_sha`, and return that SAME handle rewound to the start, so the
/// path cannot be re-pointed between the hash and the stream. That is not a
/// content guarantee: the file is streamed after it was hashed, and an
/// in-place rewrite of the same inode while it streams is not prevented (the
/// workspace is agent-writable). Synchronous and potentially slow (it reads the
/// whole file): async callers must run it under `spawn_blocking`.
///
/// `recorded_size` (0 = not recorded) short-circuits an obviously changed file
/// with a 409 before paying for the hash.
fn open_verified(
    path: &std::path::Path,
    expected_sha: &str,
    recorded_size: u64,
) -> Result<std::fs::File, ApiError> {
    use std::io::{Seek, SeekFrom};
    let gone = || ApiError::not_found("artifact is no longer in the workspace");
    let changed = || ApiError::conflict("artifact changed since the finding was recorded");
    let mut file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            gone()
        } else {
            ApiError::internal(e.to_string())
        }
    })?;
    let meta = file
        .metadata()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !meta.is_file() {
        return Err(gone());
    }
    if recorded_size != 0 && meta.len() != recorded_size {
        return Err(changed());
    }
    let now = rupu_coverage::report::sha256_reader(&mut file)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if now != expected_sha {
        return Err(changed());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(file)
}

/// `GET /api/findings/:id/artifacts/:sha256` — the bytes of an artifact or
/// evidence-block file the finding's report references.
///
/// Only artifacts or evidence-block files the finding itself references
/// (`FindingReport::artifact_refs`: `report.artifacts` plus the files its
/// `image`/`hexdump`/`pcap_ref` blocks name) are served, so
/// a request to THIS endpoint can reach only a blob some finding in the ledger
/// references (the host blob endpoint, `get_artifact_blob`, serves any stored
/// blob by hash behind the CP token, for coordinator pulls). That limits
/// exposure, but it is not a hard boundary: the ledger lives in the
/// agent-writable workspace, so a forged ledger line can list any blob whose
/// sha256 is already known — in this store, or in the store of the registered
/// host it names. Artifacts are never rendered as HTML: text is `text/plain`
/// inline, a raster image (PNG/JPEG/GIF/WebP, recognised by its magic bytes,
/// never by name) is inline as its `image/*` type, anything else an
/// `application/octet-stream` attachment; always `nosniff` and
/// `Content-Security-Policy: sandbox`.
///
/// Where the bytes come from (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B2):
/// - `copied`: this CP's content-addressed store.
/// - `external` with a `host` (a remote unit's artifact): this CP's store
///   once pulled; on first view, ONE shared pull from exactly that host —
///   resolved through the host registry, capped at the recorded size,
///   verified by size and sha256 before it enters the store. Any failure is
///   a 404 `{"unavailable": "<reason>"}`.
/// - `external` with no `host` (a local over-cap file): the workspace file,
///   from one verified handle; 404 once gone, 409 once changed.
async fn get_artifact(
    State(s): State<AppState>,
    Path((id, sha)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let store = ArtifactStore::new(s.global_dir.join("findings").join("artifacts"));
    let blob = store.blob_path_checked(&sha).ok_or_else(|| {
        ApiError::bad_request("artifact id must be a 64-character lowercase sha256")
    })?;
    let global = s.global_dir.clone();
    let id_for_lookup = id.clone();
    let finding = tokio::task::spawn_blocking(move || find_finding(&global, &id_for_lookup))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("finding {id} not found")))?;
    let artifact = finding
        .record
        .report
        .as_ref()
        .and_then(|r| r.artifact_refs().find(|a| a.sha256 == sha))
        .cloned()
        .ok_or_else(|| ApiError::not_found("this finding does not reference that artifact"))?;

    let file = match (artifact.stored, artifact.host.as_deref()) {
        (Some(ArtifactStorage::External), Some(host)) => {
            match remote_artifact(&s, &blob, host, &sha, artifact.size).await {
                Ok(file) => file,
                Err(reason) => return Ok(unavailable(reason)),
            }
        }
        (Some(ArtifactStorage::External), None) => {
            let ws = crate::api::code::load_workspace(&s, &finding.ws_id)?;
            let gone = || ApiError::not_found("artifact is no longer in the workspace");
            let p = crate::api::source::resolve_under_workspace(
                std::path::Path::new(&ws.path),
                &artifact.path,
            )
            .map_err(|_| gone())?;
            // Cheap early refusal so a FIFO or directory is never opened.
            if !p.is_file() {
                return Err(gone());
            }
            // Open, verify and hash ONE handle off the runtime, then serve that
            // same handle: a path in an agent-writable workspace can be
            // re-pointed between a check and a later open, a handle cannot.
            let (expected, recorded_size) = (sha.clone(), artifact.size);
            let verified =
                tokio::task::spawn_blocking(move || open_verified(&p, &expected, recorded_size))
                    .await
                    .map_err(|e| ApiError::internal(e.to_string()))??;
            tokio::fs::File::from_std(verified)
        }
        _ => tokio::fs::File::open(&blob).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found("artifact blob missing from the store")
            } else {
                ApiError::internal(e.to_string())
            }
        })?,
    };

    Ok(artifact_response(file, artifact.kind, &artifact.path).await)
}

/// A 404 saying why an artifact cannot be had (spec B2.5):
/// `{"unavailable": "<reason>"}`. Never a stored blob, never a 200.
fn unavailable(reason: impl Into<String>) -> Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "unavailable": reason.into() })),
    )
        .into_response()
}

/// Blob `path` from this CP's store, or `Ok(None)` when there is none. The
/// open is non-blocking and refuses anything but a regular file, so a FIFO
/// or directory squatting on the address reads as absent rather than parking
/// a blocking-pool thread.
async fn open_store_blob(path: &std::path::Path) -> Result<Option<tokio::fs::File>, String> {
    let p = path.to_path_buf();
    let opened = tokio::task::spawn_blocking(move || crate::api::fs_open::open_regular_file(&p))
        .await
        .map_err(|e| format!("opening this control plane's stored copy failed: {e}"))?;
    match opened {
        Ok(f) => Ok(Some(tokio::fs::File::from_std(f))),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(format!(
            "this control plane's stored copy is unreadable: {e}"
        )),
    }
}

/// An artifact recorded `external` on `host`: this CP's stored copy when it
/// has one (blobs only ever enter the store verified, so a hit never contacts
/// any host), otherwise pulled from `host` — and only `host`, resolved
/// through the host registry — into the store first. `Err` is the
/// `unavailable` reason.
///
/// The recorded `size` caps the pull, but it comes from an agent-writable
/// ledger: a forged size of many gigabytes would let a host (or a bucket
/// writer) fill this CP's disk before the hash check fails. A copied artifact
/// cannot legitimately exceed its host's copy cap, so a recorded size over
/// this CP's own `[findings].artifact_max_bytes` is refused before any host
/// is contacted.
async fn remote_artifact(
    s: &AppState,
    blob: &std::path::Path,
    host: &str,
    sha256: &str,
    size: u64,
) -> Result<tokio::fs::File, String> {
    if let Some(file) = open_store_blob(blob).await? {
        return Ok(file);
    }
    let limit = s
        .config
        .read()
        .ok()
        .and_then(|c| c.findings.artifact_max_bytes)
        .unwrap_or(rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES);
    if size > limit {
        return Err(format!(
            "artifact {sha256} is recorded as {size} bytes, over this control plane's \
             [findings].artifact_max_bytes of {limit} bytes"
        ));
    }
    let conn = s.hosts.resolve(host).map_err(|e| match e {
        crate::host::connector::HostConnectorError::NotFound(_) => {
            format!("host {host} is not registered with this control plane")
        }
        other => format!("host {host}: {other}"),
    })?;
    let store = ArtifactStore::new(s.global_dir.join("findings").join("artifacts"));
    let (host_owned, sha_owned) = (host.to_string(), sha256.to_string());
    shared_pull(blob.to_path_buf(), move || {
        pull_into_store(conn, store, host_owned, sha_owned, size)
    })
    .await?;
    open_store_blob(blob).await?.ok_or_else(|| {
        "the pulled blob left this control plane's store before it could be served".to_string()
    })
}

/// What one remote artifact pull came to: `Err` is the `unavailable` reason.
type PullOutcome = Result<(), String>;

/// One pull in flight, as every request for its blob awaits it.
struct InFlightPull {
    /// Never reused, so a finished pull retires only its own entry.
    id: u64,
    outcome: futures_util::future::Shared<futures_util::future::BoxFuture<'static, PullOutcome>>,
}

#[derive(Default)]
struct InFlightPulls {
    next_id: u64,
    /// Keyed by the destination blob path (store root + sha), not the bare
    /// sha: one process may serve several stores (tests run many `AppState`s).
    by_dest: HashMap<std::path::PathBuf, InFlightPull>,
}

/// Every remote artifact pull in flight in this process. Holds IN-FLIGHT
/// pulls only: each pull's task removes its own entry when it ends, so the
/// map never outgrows the pulls running right now, and a failed pull is not
/// remembered (the next view tries again).
fn in_flight_pulls() -> std::sync::MutexGuard<'static, InFlightPulls> {
    static PULLS: std::sync::OnceLock<std::sync::Mutex<InFlightPulls>> = std::sync::OnceLock::new();
    PULLS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
fn pull_in_flight(dest: &std::path::Path) -> bool {
    in_flight_pulls().by_dest.contains_key(dest)
}

/// Owned by a pull's task: removes the pull's entry when the task ends,
/// however it ends — finished, panicked, or dropped by a shutting-down
/// runtime.
struct RetireOnDrop {
    dest: std::path::PathBuf,
    id: u64,
}

impl Drop for RetireOnDrop {
    fn drop(&mut self) {
        let retired = {
            let mut pulls = in_flight_pulls();
            if pulls
                .by_dest
                .get(&self.dest)
                .is_some_and(|p| p.id == self.id)
            {
                pulls.by_dest.remove(&self.dest)
            } else {
                None
            }
        };
        // The entry holds this pull's shared outcome; let it go unlocked.
        drop(retired);
    }
}

/// Run `pull` as THE pull into `dest`, or join the one already in flight.
///
/// A request that joins an in-flight pull for the same destination gets that
/// pull's outcome, even if its own record names a different host or size: a
/// successful pull is still sha-verified into the content address, so only a
/// failure can be shared.
///
/// Cancel-safe: the pull runs in its own spawned task, to completion,
/// whether or not anyone still waits on it. A request whose client goes away
/// (axum drops the handler future) only stops awaiting the shared outcome:
/// the pull other requests share keeps going, its temp file is cleaned up by
/// the task itself, and its entry is retired by the task. The entry is
/// inserted under the lock and the task spawned right after it is released,
/// with no `.await` in between (so no cancellation point): the task's retire
/// guard takes the same lock, and a runtime that is shutting down drops a
/// newly spawned future synchronously, inside `spawn`.
async fn shared_pull<F, Fut>(dest: std::path::PathBuf, pull: F) -> PullOutcome
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = PullOutcome> + Send + 'static,
{
    use futures_util::FutureExt as _;
    let (outcome, start) = {
        let mut pulls = in_flight_pulls();
        if let Some(p) = pulls.by_dest.get(&dest) {
            (p.outcome.clone(), None)
        } else {
            pulls.next_id += 1;
            let id = pulls.next_id;
            let (tx, rx) = tokio::sync::oneshot::channel::<PullOutcome>();
            let outcome = rx
                .map(|r| {
                    r.unwrap_or_else(|_| {
                        Err(
                            "the artifact pull did not finish (it panicked or was shut down)"
                                .to_string(),
                        )
                    })
                })
                .boxed()
                .shared();
            pulls.by_dest.insert(
                dest.clone(),
                InFlightPull {
                    id,
                    outcome: outcome.clone(),
                },
            );
            (outcome, Some((id, tx)))
        }
    };
    if let Some((id, tx)) = start {
        // Armed before `pull()` runs, so even a panic building the future
        // retires the entry (and drops `tx`, failing its waiters).
        let retire = RetireOnDrop { dest, id };
        let fut = pull();
        tokio::spawn(async move {
            let _retire = retire;
            let _ = tx.send(fut.await);
        });
    }
    outcome.await
}

/// Removes a pull's temp file when the pull ends, however it ends. After a
/// successful install the file was renamed away and there is nothing left.
struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The body of one pull's task: blob `sha256` from `host` into a temp file in
/// `store`, capped at the recorded `size`, then installed at its content
/// address only if it is exactly `size` bytes hashing to `sha256` (the
/// transports only bound the size; a short transfer can return `Ok`).
async fn pull_into_store(
    conn: Arc<dyn crate::host::connector::HostConnector>,
    store: ArtifactStore,
    host: String,
    sha256: String,
    size: u64,
) -> PullOutcome {
    let interrupted = |e: tokio::task::JoinError| format!("the artifact pull was interrupted: {e}");
    // A pull that finished between this request's store check and its
    // joining the in-flight map has already installed the blob.
    if tokio::fs::metadata(store.blob_path(&sha256))
        .await
        .is_ok_and(|m| m.is_file())
    {
        return Ok(());
    }
    let tmp = {
        let (store, sha) = (store.clone(), sha256.clone());
        tokio::task::spawn_blocking(move || store.pull_temp_path(&sha))
            .await
            .map_err(interrupted)?
            .map_err(|e| format!("this control plane's artifact store is not writable: {e}"))?
    };
    let _cleanup = RemoveOnDrop(tmp.clone());
    // The recorded size caps the transfer, 0 included: an ingested remote
    // artifact always carries its real size, so 0 is a genuinely empty file.
    // The error variant for one cause differs by transport, so the reason is
    // the connector's own message, not a branch on the variant.
    conn.pull_finding_artifact(&sha256, &tmp, size)
        .await
        .map_err(|e| format!("host {host}: {e}"))?;
    let installed = {
        let (store, sha, tmp) = (store, sha256.clone(), tmp.clone());
        tokio::task::spawn_blocking(move || store.install_verified(&tmp, &sha, size))
            .await
            .map_err(interrupted)?
    };
    match installed {
        Ok(_) => Ok(()),
        Err(rupu_coverage::report::ArtifactInstallError::Mismatch {
            got_size,
            got_sha256,
            ..
        }) => Err(format!(
            "hash mismatch: host {host} returned {got_size} bytes hashing to {got_sha256}, \
             but the finding recorded {size} bytes hashing to {sha256}"
        )),
        Err(e) => Err(format!(
            "could not store the artifact pulled from host {host}: {e}"
        )),
    }
}

/// The one way artifact bytes leave this CP: streamed from `file`, never
/// renderable as HTML. Text is `text/plain; charset=utf-8` inline; a binary
/// (or unknown-kind) file whose leading bytes are a PNG / JPEG / GIF / WebP
/// is served inline as that `image/*` type (so the web can show an `image`
/// evidence block); anything else an `application/octet-stream` attachment.
/// ALWAYS `X-Content-Type-Options: nosniff` and
/// `Content-Security-Policy: sandbox`. `name` (a recorded path or any label)
/// becomes the download's `filename`, reduced to a safe basename.
///
/// `Content-Length` is the served handle's own length (never the path's, which
/// may have been re-pointed), so a client can refuse an oversize body before
/// reading it — a coordinator pulling CP-to-CP does — and the body is cut at
/// that length should the file grow while it streams.
pub(crate) async fn artifact_response(
    file: tokio::fs::File,
    kind: Option<ArtifactKind>,
    name: &str,
) -> Response {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let mut file = file;
    let len = file.metadata().await.ok().map(|m| m.len());

    // Peek the head of anything that is not declared text, to type raster
    // images by their magic bytes. A failed read just means "not an image";
    // but the handle must be rewound before it streams, and a handle that
    // cannot be rewound is never served half-consumed (a 500, not wrong bytes).
    let mut image_type = None;
    if kind != Some(ArtifactKind::Text) {
        let mut head = [0u8; 12];
        let mut got = 0;
        while got < head.len() {
            match file.read(&mut head[got..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        if file.seek(std::io::SeekFrom::Start(0)).await.is_err() {
            return axum::response::IntoResponse::into_response(ApiError::internal(
                "could not read the artifact",
            ));
        }
        image_type = raster_image_type(&head[..got]);
    }

    let body = match len {
        Some(len) => Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
            file.take(len),
            STREAM_CHUNK_BYTES,
        )),
        None => Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
            file,
            STREAM_CHUNK_BYTES,
        )),
    };
    let name = safe_filename(name);
    let (ctype, disposition) = match kind {
        Some(ArtifactKind::Text) => (
            "text/plain; charset=utf-8",
            format!("inline; filename=\"{name}\""),
        ),
        _ => match image_type {
            Some(t) => (t, format!("inline; filename=\"{name}\"")),
            None => (
                "application/octet-stream",
                format!("attachment; filename=\"{name}\""),
            ),
        },
    };
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Belt and braces with `nosniff`: even if a client did render the bytes,
    // `sandbox` gives them an opaque origin with no scripts, forms or plugins.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    if let Some(len) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    resp
}

/// `GET /api/findings/artifacts/:sha256` — a blob from THIS host's artifact
/// store, by hash, for a coordinator pulling a placed unit's artifact (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B1). Unlike
/// `get_artifact` it does not check that any finding references the blob: the
/// coordinator's own `/api/findings/:id/artifacts/:sha256` decides what a
/// browser may see, and this is the host-to-coordinator channel behind the
/// CP's bearer token. Only the content-addressed store is reachable (a
/// malformed digest is a 400 before any path is built), and the bytes leave
/// through [`artifact_response`] like every other artifact.
async fn get_artifact_blob(
    State(s): State<AppState>,
    Path(sha): Path<String>,
) -> Result<Response, ApiError> {
    let blob = ArtifactStore::new(s.global_dir.join("findings").join("artifacts"))
        .blob_path_checked(&sha)
        .ok_or_else(|| {
            ApiError::bad_request("artifact id must be a 64-character lowercase sha256")
        })?;
    let absent = || ApiError::not_found(format!("artifact {sha} is not in this host's store"));
    // Open first, then read the type off the handle we got: a path check
    // followed by a later open is a race, and a plain open of a FIFO parks a
    // blocking-pool thread forever. `open_regular_file` is non-blocking and
    // refuses anything but a regular file.
    let file = tokio::task::spawn_blocking(move || crate::api::fs_open::open_regular_file(&blob))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => absent(),
            _ => ApiError::internal(e.to_string()),
        })?;
    Ok(artifact_response(tokio::fs::File::from_std(file), None, &sha).await)
}

// ── report exports ──────────────────────────────────────────────────────────

/// Title of a project report when the request gives none.
const DEFAULT_EXPORT_TITLE: &str = "Findings report";

/// Longest report title accepted, in characters. The title lands in the
/// document, its page header and the download's file name.
const MAX_EXPORT_TITLE_CHARS: usize = 200;

/// Query of `GET /api/findings/:id/export`.
#[derive(Debug, Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

/// Body of `POST /api/findings/export`. Unknown fields are refused: a
/// misspelt filter (`severity` for `min_severity`) must not quietly widen the
/// report to every finding.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportBody {
    format: Option<String>,
    title: Option<String>,
    /// Only these finding ids (which also opts summary findings in).
    #[serde(default)]
    ids: Vec<String>,
    ws_id: Option<String>,
    /// A run and its sub-runs, resolved as the list endpoint does.
    run_id: Option<String>,
    /// `critical` | `high` | `medium` | `low` | `info`: this severity and worse.
    min_severity: Option<String>,
    owner: Option<String>,
    cwe: Option<String>,
    /// Keep summary-profile findings (off by default).
    #[serde(default)]
    include_summaries: bool,
    /// One file per finding plus an index, as a zip.
    #[serde(default)]
    split: bool,
}

/// A rendered report on its way to a client or a file.
#[derive(Debug)]
pub struct ExportedReport {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// The file name (with extension) to offer it under.
    pub name: String,
    /// Rendered HTML: additionally served with a sandboxing CSP.
    pub html: bool,
}

/// Why a report could not be produced. The HTTP handlers map it to a status
/// (`ApiError`), `rupu findings export` prints it.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// The selection matched no finding (or the named finding does not exist).
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Render(#[from] ExportError),
    #[error("{0}")]
    Internal(String),
}

impl From<ReportError> for ApiError {
    fn from(e: ReportError) -> Self {
        match e {
            ReportError::NotFound(msg) => ApiError::not_found(msg),
            ReportError::Render(e) => export_error(e),
            ReportError::Internal(msg) => ApiError::internal(msg),
        }
    }
}

/// The requested format, refused up front when it is unknown or this build
/// cannot produce it (PDF without the `pdf` feature is 501).
fn export_format(raw: Option<&str>) -> Result<Format, ApiError> {
    let fmt = raw
        .and_then(Format::parse)
        .ok_or_else(|| ApiError::bad_request("format must be one of md, html, pdf"))?;
    if !fmt.is_available() {
        return Err(export_error(ExportError::PdfUnavailable));
    }
    Ok(fmt)
}

fn export_error(e: ExportError) -> ApiError {
    match e {
        ExportError::PdfUnavailable => ApiError::not_available(e.to_string()),
        other => ApiError::internal(other.to_string()),
    }
}

/// A severity name (`critical` | `high` | `medium` | `low` | `info`, any
/// case), or `None` for anything else.
pub fn parse_min_severity(raw: &str) -> Option<Severity> {
    serde_json::from_value(serde_json::Value::String(raw.to_lowercase())).ok()
}

fn min_severity_error() -> ApiError {
    ApiError::bad_request("min_severity must be one of critical, high, medium, low, info")
}

/// The display-number prefix: `[findings].export_id_prefix` when it is a
/// sane identifier, else `SEC`. The value ends up in file names and document
/// text and the project layer of the config is repo-controlled, so an invalid
/// one is refused (with a warning) rather than escaped.
pub fn resolve_export_prefix(configured: Option<&str>) -> String {
    match configured {
        None => DEFAULT_PREFIX.to_string(),
        Some(p) if is_valid_prefix(p) => p.to_string(),
        Some(p) => {
            tracing::warn!(
                prefix = ?p.chars().take(32).collect::<String>(),
                "ignoring invalid [findings].export_id_prefix (a letter, then up to 15 of \
                 A-Z a-z 0-9 _ -); using {DEFAULT_PREFIX}"
            );
            DEFAULT_PREFIX.to_string()
        }
    }
}

fn export_prefix(s: &AppState) -> String {
    let configured = s
        .config
        .read()
        .ok()
        .and_then(|c| c.findings.export_id_prefix.clone());
    resolve_export_prefix(configured.as_deref())
}

/// The title of a project report: `raw` trimmed, or [`DEFAULT_EXPORT_TITLE`]
/// when it is absent or blank. Refused when longer than
/// [`MAX_EXPORT_TITLE_CHARS`] (the title lands in the document, its page
/// header and the file name).
pub fn normalize_export_title(raw: Option<&str>) -> Result<String, String> {
    match raw.map(str::trim) {
        Some(t) if t.chars().count() > MAX_EXPORT_TITLE_CHARS => Err(format!(
            "title is longer than {MAX_EXPORT_TITLE_CHARS} characters"
        )),
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => Ok(DEFAULT_EXPORT_TITLE.to_string()),
    }
}

/// Resolve a project given as a workspace id or as a path to its checkout to
/// the workspace id. A path is matched against the registered workspaces'
/// (canonicalised) paths.
pub fn resolve_project(global_dir: &std::path::Path, project: &str) -> Result<String, String> {
    let workspaces = store_for(global_dir).list().unwrap_or_default();
    if let Some(w) = workspaces.iter().find(|w| w.id == project) {
        return Ok(w.id.clone());
    }
    let wanted = std::path::Path::new(project);
    let wanted = wanted
        .canonicalize()
        .unwrap_or_else(|_| wanted.to_path_buf());
    workspaces
        .iter()
        .find(|w| {
            let p = std::path::Path::new(&w.path);
            p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) == wanted
        })
        .map(|w| w.id.clone())
        .ok_or_else(|| {
            format!("no project matches `{project}` (expected a workspace id or the path of a registered project)")
        })
}

fn export_input(f: FindingOut) -> ExportInput {
    ExportInput {
        ws_id: f.ws_id,
        project: f.project,
        workflow_name: f.workflow_name,
        record: f.record,
    }
}

/// Fill in `workflow_name` (joined via `declared_by.run_id`, as the list
/// endpoint does) on findings that are about to be rendered.
fn attach_workflow_names(runs: &RunStore, findings: &mut [ExportFinding]) {
    let names = workflow_names_by_run(
        runs,
        findings
            .iter()
            .map(|f| f.input.record.declared_by.run_id.as_str()),
    );
    for f in findings {
        f.input.workflow_name = names.get(&f.input.record.declared_by.run_id).cloned();
    }
}

/// A `Content-Disposition` value that always downloads `name`. The quoted
/// `filename` is ASCII with `"`, `\` and anything unprintable replaced by
/// `_`; a name that has more than that also gets the RFC 6266 `filename*`
/// form so a UTF-8 title survives.
fn attachment_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut out = format!("attachment; filename=\"{fallback}\"");
    if fallback != name {
        let mut encoded = String::new();
        for b in name.bytes() {
            if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
                encoded.push(b as char);
            } else {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
        out.push_str(&format!("; filename*=UTF-8''{encoded}"));
    }
    out
}

/// The download response: always an attachment (rendered HTML is never shown
/// inline from this origin), `nosniff`, and for HTML a `sandbox` CSP as well.
fn download_response(d: ExportedReport) -> Response {
    let mut resp = Response::new(Body::from(d.bytes));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(d.content_type),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if d.html {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox"),
        );
    }
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&attachment_disposition(&d.name))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    resp
}

/// The file name (no extension) a project report is offered under: the
/// title, cleaned as a finding title is for [`filename`]. A title that
/// cleans to nothing, or would start a hidden file, falls back to the default.
fn report_file_stem(title: &str) -> String {
    match sanitize_title(title).trim_start_matches('.') {
        "" => DEFAULT_EXPORT_TITLE.to_string(),
        stem => stem.to_string(),
    }
}

/// Renders an export against this machine's finding-artifact store: an
/// `image` evidence block's file is embedded from it when it was copied
/// there, verified and within the renderer's cap. Nothing is fetched from
/// another host to do so.
fn with_local_blobs<T>(global: &std::path::Path, render: impl FnOnce(Blobs<'_>) -> T) -> T {
    let store = ArtifactStore::new(global.join("findings").join("artifacts"));
    let read = |sha256: &str, max_bytes: u64| store.read_verified(sha256, max_bytes);
    render(Blobs::new(&read))
}

/// One finding rendered as a stand-alone report, numbered within its own
/// project (so it carries the number it has in the project's full report).
pub fn export_finding_report(
    global: &std::path::Path,
    runs: &RunStore,
    id: &str,
    prefix: &str,
    fmt: Format,
) -> Result<ExportedReport, ReportError> {
    let all = collect_all_findings(global);
    let ws_id = all
        .iter()
        .find(|f| f.record.id == id)
        .map(|f| f.ws_id.clone())
        .ok_or_else(|| ReportError::NotFound(format!("finding {id} not found")))?;
    // Numbers are per project: number only the finding's own.
    let project: Vec<ExportInput> = all
        .into_iter()
        .filter(|f| f.ws_id == ws_id)
        .map(export_input)
        .collect();
    let mut numbered = assign_numbers(project, prefix);
    let numbers = number_map(&numbered);
    let pos = numbered
        .iter()
        .position(|f| f.input.record.id == id)
        .ok_or_else(|| ReportError::Internal(format!("finding {id} was not numbered")))?;
    let mut finding = numbered.swap_remove(pos);
    attach_workflow_names(runs, std::slice::from_mut(&mut finding));
    let bytes = with_local_blobs(global, |blobs| {
        render_finding(&finding, &numbers, fmt, blobs)
    })?;
    Ok(ExportedReport {
        bytes,
        content_type: fmt.content_type(),
        name: filename(&finding, fmt.ext()),
        html: fmt == Format::Html,
    })
}

/// How many PDF renders `cp serve` runs at once. A Typst compile is CPU- and
/// memory-heavy (fonts, layout, a whole project in one document), and `cp
/// serve` is a long-running process: without a bound, a burst of export
/// clicks would pin every blocking thread and grow the heap together.
/// Markdown and HTML are cheap and are not gated.
const MAX_CONCURRENT_PDF_RENDERS: usize = 2;

static PDF_RENDERS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_CONCURRENT_PDF_RENDERS);

/// A slot to render `fmt` in: for PDF, one of `gate`'s permits (waiting for
/// one to free up), for everything else nothing. The permit is moved into the
/// blocking render and released when the render ends, not when the request
/// does: a client that disconnects does not cancel the compile.
async fn render_slot(
    gate: &'static tokio::sync::Semaphore,
    fmt: Format,
) -> Result<Option<tokio::sync::SemaphorePermit<'static>>, ApiError> {
    if fmt != Format::Pdf {
        return Ok(None);
    }
    gate.acquire()
        .await
        .map(Some)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/findings/:id/export?format=md|html|pdf` — one finding as a
/// downloadable report, numbered within its own project.
async fn export_finding(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let fmt = export_format(q.format.as_deref())?;
    let prefix = export_prefix(&s);
    let (global, runs) = (s.global_dir.clone(), Arc::clone(&s.run_store));
    let slot = render_slot(&PDF_RENDERS, fmt).await?;
    let download = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        export_finding_report(&global, &runs, &id, &prefix, fmt)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(download_response(download))
}

/// What a project report covers. Every filter narrows the selection; unset
/// ones keep everything.
#[derive(Debug, Clone, Default)]
pub struct ReportRequest {
    /// The report's title (already through [`normalize_export_title`]).
    pub title: String,
    /// Only these finding ids (which also opts summary findings in).
    pub ids: Vec<String>,
    pub ws_id: Option<String>,
    /// A run and its sub-runs, resolved as the list endpoint does.
    pub run_id: Option<String>,
    /// This severity and worse.
    pub min_severity: Option<Severity>,
    pub owner: Option<String>,
    pub cwe: Option<String>,
    /// Keep summary-profile findings (off by default).
    pub include_summaries: bool,
    /// One file per finding plus an index, as a zip.
    pub split: bool,
}

/// A report over the findings `req` selects: one file, or with `req.split` a
/// zip of one file per finding plus an index. Every finding is numbered
/// within its project before any is dropped, so a number never depends on
/// what else was left out. [`ReportError::NotFound`] when nothing matches.
pub fn export_project_report(
    global: &std::path::Path,
    runs: &RunStore,
    prefix: &str,
    req: ReportRequest,
    fmt: Format,
) -> Result<ExportedReport, ReportError> {
    let run_ids = req
        .run_id
        .as_deref()
        .map(|parent| resolve_run_scope(runs, parent));
    // Number everything first, then select: a finding keeps the number it has
    // in the whole project whatever else is left out.
    let numbered = assign_numbers(
        collect_all_findings(global)
            .into_iter()
            .map(export_input)
            .collect(),
        prefix,
    );
    // Every finding's number, with its project, taken before selecting: a
    // cross-reference to a finding the selection leaves out still prints the
    // number its own export carries.
    let all_numbers: Vec<(String, String, String)> = numbered
        .iter()
        .map(|f| {
            (
                f.input.ws_id.clone(),
                f.input.record.id.clone(),
                f.number.clone(),
            )
        })
        .collect();
    let sel = Selection {
        ids: req.ids,
        ws_id: req.ws_id,
        run_ids,
        min_severity: req.min_severity,
        owner: req.owner,
        cwe: req.cwe,
        include_summaries: req.include_summaries,
    };
    let mut chosen = select(numbered, &sel);
    if chosen.is_empty() {
        return Err(ReportError::NotFound(
            "no findings match this selection".to_string(),
        ));
    }
    attach_workflow_names(runs, &mut chosen);
    // Numbers are per project, so only the projects in the report lend theirs:
    // another project's `SEC-004` would name a finding the reader cannot see.
    let in_report: HashSet<&str> = chosen.iter().map(|f| f.input.ws_id.as_str()).collect();
    let numbers: HashMap<String, String> = all_numbers
        .into_iter()
        .filter(|(ws_id, _, _)| in_report.contains(ws_id.as_str()))
        .map(|(_, id, number)| (id, number))
        .collect();
    let meta = ReportMeta {
        scope: describe(&sel, req.run_id.as_deref(), &chosen),
        title: req.title,
        generated_at: chrono::Utc::now(),
    };
    let stem = report_file_stem(&meta.title);
    if req.split {
        let bytes = with_local_blobs(global, |blobs| {
            render_split_zip(&meta, &chosen, &numbers, fmt, blobs)
        })?;
        Ok(ExportedReport {
            bytes,
            content_type: "application/zip",
            name: fit_file_name(&stem, "zip"),
            html: false,
        })
    } else {
        let bytes = with_local_blobs(global, |blobs| {
            render_project(&meta, &chosen, &numbers, fmt, blobs)
        })?;
        Ok(ExportedReport {
            bytes,
            content_type: fmt.content_type(),
            name: fit_file_name(&stem, fmt.ext()),
            html: fmt == Format::Html,
        })
    }
}

/// `POST /api/findings/export` — a report over the findings the body selects:
/// one file, or with `split` a zip of one file per finding plus an index.
/// 404 when nothing matches.
async fn export_findings(
    State(s): State<AppState>,
    Json(body): Json<ExportBody>,
) -> Result<Response, ApiError> {
    let fmt = export_format(body.format.as_deref())?;
    let min_severity = body
        .min_severity
        .as_deref()
        .map(|raw| parse_min_severity(raw).ok_or_else(min_severity_error))
        .transpose()?;
    let title = normalize_export_title(body.title.as_deref()).map_err(ApiError::bad_request)?;
    // Compared by number (CWE-79 never selects CWE-798); an unreadable value
    // is refused rather than matching nothing.
    let cwe = body
        .cwe
        .as_deref()
        .map(|raw| {
            parse_cwe(raw)
                .map(|n| format!("CWE-{n}"))
                .ok_or_else(|| ApiError::bad_request("cwe must be a CWE id such as CWE-79"))
        })
        .transpose()?;
    let prefix = export_prefix(&s);
    let req = ReportRequest {
        title,
        ids: body.ids,
        ws_id: body.ws_id,
        run_id: body.run_id,
        min_severity,
        owner: body.owner,
        cwe,
        include_summaries: body.include_summaries,
        split: body.split,
    };
    let (global, runs) = (s.global_dir.clone(), Arc::clone(&s.run_store));
    let slot = render_slot(&PDF_RENDERS, fmt).await?;
    let download = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        export_project_report(&global, &runs, &prefix, req, fmt)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(download_response(download))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rupu_coverage::{Attribution, FindingEvidence, FindingScope, Surface};
    use rupu_orchestrator::executor::Event;

    fn attribution() -> Attribution {
        attribution_run("run_01KS19A4MQXP")
    }

    fn attribution_run(run_id: &str) -> Attribution {
        Attribution {
            run_id: run_id.to_string(),
            model: "claude-sonnet-4-6".to_string(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
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
            codename: "jade-reef".to_string(),
            codename_derived: true,
            ws_id: ws_id.to_string(),
            project: "proj".to_string(),
            target_id: "tgt".to_string(),
            workflow_name: workflow_name.map(|s| s.to_string()),
            permalink: None,
            report_summary: None,
            record: FindingRecord {
                id: id.to_string(),
                file_path: Some("src/a.rs".to_string()),
                line_range: Some([1, 10]),
                target_ref: None,
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
                profile: rupu_coverage::FindingProfile::Summary,
                report: None,
                tags: Vec::new(),
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
            finding_in(
                "ws1",
                Some("wfA"),
                "a",
                Severity::Critical,
                "2026-01-01T00:00:00Z",
            ),
            finding_in(
                "ws1",
                Some("wfB"),
                "b",
                Severity::High,
                "2026-01-02T00:00:00Z",
            ),
            finding_in(
                "ws2",
                Some("wfA"),
                "c",
                Severity::Medium,
                "2026-01-03T00:00:00Z",
            ),
            finding_in("ws2", None, "d", Severity::Low, "2026-01-04T00:00:00Z"),
        ]
    }

    /// Build the run-id match set the handler would resolve from a parent run
    /// id plus its fan-out unit sub-run ids.
    fn run_set(ids: &[&str]) -> Option<HashSet<String>> {
        Some(ids.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn no_filter_keeps_all() {
        let resp = scope_by_run_set(mixed_findings(), &None, &None, &None);
        assert_eq!(resp.findings.len(), 4);
        assert_eq!(resp.summary.total, 4);
        assert_eq!(resp.summary.critical, 1);
        assert_eq!(resp.summary.high, 1);
        assert_eq!(resp.summary.medium, 1);
        assert_eq!(resp.summary.low, 1);
    }

    #[test]
    fn ws_id_filter_scopes_findings_and_summary() {
        let resp = scope_by_run_set(mixed_findings(), &None, &Some("ws2".to_string()), &None);
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
        let resp = scope_by_run_set(mixed_findings(), &None, &None, &Some("wfA".to_string()));
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
        let resp = scope_by_run_set(input, &None, &None, &Some("anything".to_string()));
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
        // A set of one parent id still matches top-level findings.
        let resp = scope_by_run_set(run_findings(), &run_set(&["runA"]), &None, &None);
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
        let resp = scope_by_run_set(run_findings(), &run_set(&["nope"]), &None, &None);
        assert!(resp.findings.is_empty());
        assert_eq!(resp.summary, FindingsSummary::default());
        assert_eq!(resp.summary.total, 0);
    }

    /// The core fan-out fix: a finding attributed to a `for_each` unit's sub-run
    /// id is included when filtering by the PARENT run id, because the handler
    /// resolves the parent into a set that contains the sub-run id.
    #[test]
    fn run_set_includes_for_each_sub_run_findings() {
        let findings = vec![
            // Declared at the top level under the parent run.
            finding_run("parent", "top", Severity::High, "2026-01-01T00:00:00Z"),
            // Declared inside a for_each unit → attributed to the unit sub-run.
            finding_run("unit-1", "fanA", Severity::Critical, "2026-01-02T00:00:00Z"),
            finding_run("unit-2", "fanB", Severity::Medium, "2026-01-03T00:00:00Z"),
            // An unrelated run's finding must NOT leak in.
            finding_run("other", "nope", Severity::Low, "2026-01-04T00:00:00Z"),
        ];
        // Set the handler would build: parent ∪ {unit-1, unit-2}.
        let set = run_set(&["parent", "unit-1", "unit-2"]);
        let resp = scope_by_run_set(findings, &set, &None, &None);
        let ids: Vec<&str> = resp.findings.iter().map(|f| f.record.id.as_str()).collect();
        // Severity-sorted: critical(fanA), high(top), medium(fanB). "nope" excluded.
        assert_eq!(ids, vec!["fanA", "top", "fanB"]);
        // Summary reflects the matched union, not the parent id alone.
        assert_eq!(resp.summary.total, 3);
        assert_eq!(resp.summary.critical, 1);
        assert_eq!(resp.summary.high, 1);
        assert_eq!(resp.summary.medium, 1);
        assert_eq!(resp.summary.low, 0);
    }

    /// The running-run fix: `resolve_run_scope` recovers an IN-FLIGHT unit's
    /// sub-run id from the parent's live `events.jsonl` (a `UnitStarted` whose
    /// transcript stem is the unit run id), even with NO checkpoint written yet
    /// — which is why the Findings tab used to look empty mid-run.
    #[test]
    fn resolve_run_scope_picks_up_inflight_units_from_events() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let parent = "run_parent";
        let ev_path = store.events_path(parent);
        std::fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        let events = [
            Event::UnitStarted {
                run_id: parent.into(),
                step_id: "assess".into(),
                index: 0,
                unit_key: "crates/db".into(),
                agent: Some("oracle".into()),
                transcript_path: tmp.path().join("transcripts/run_unit0.jsonl"),
                host: None,
                codename: None,
            },
            Event::StepWorking {
                run_id: parent.into(),
                step_id: "recon".into(),
                note: Some("scanning".into()),
                transcript_path: Some(tmp.path().join("transcripts/run_recon.jsonl")),
            },
        ];
        let body: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&ev_path, body).unwrap();

        let scope = resolve_run_scope(&store, parent);
        assert!(scope.contains("run_parent"), "parent always in scope");
        assert!(
            scope.contains("run_unit0"),
            "in-flight unit sub-run id must be recovered from events (was: {scope:?})"
        );
        assert!(scope.contains("run_recon"));
    }

    /// Nested sub-run transcript layout (`.../<sub_id>/transcript.jsonl`)
    /// resolves to the directory name, not the generic `transcript` stem.
    #[test]
    fn resolve_run_scope_handles_nested_sub_run_transcript_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let parent = "run_parent";
        let ev_path = store.events_path(parent);
        std::fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        let ev = Event::UnitStarted {
            run_id: parent.into(),
            step_id: "s".into(),
            index: 0,
            unit_key: "u".into(),
            agent: None,
            transcript_path: tmp
                .path()
                .join("runs/run_parent/sub/sub_child/transcript.jsonl"),
            host: None,
            codename: None,
        };
        std::fs::write(&ev_path, serde_json::to_string(&ev).unwrap() + "\n").unwrap();
        let scope = resolve_run_scope(&store, parent);
        assert!(
            scope.contains("sub_child"),
            "nested layout → dir name (was: {scope:?})"
        );
    }

    /// `query_findings` joins provenance (project) and expands `run:` to the
    /// run's sub-runs recovered from its events.
    #[test]
    fn query_findings_matches_project_and_expands_run_to_sub_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let ev_path = store.events_path("run_parent");
        std::fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        let ev = Event::UnitStarted {
            run_id: "run_parent".into(),
            step_id: "assess".into(),
            index: 0,
            unit_key: "u".into(),
            agent: None,
            transcript_path: tmp.path().join("transcripts/run_unit0.jsonl"),
            host: None,
            codename: None,
        };
        std::fs::write(&ev_path, serde_json::to_string(&ev).unwrap() + "\n").unwrap();
        let mut other = finding_run(
            "run_other",
            "nope",
            Severity::Critical,
            "2026-01-04T00:00:00Z",
        );
        other.project = "elsewhere".to_string();
        let all = vec![
            finding_run("run_parent", "top", Severity::High, "2026-01-01T00:00:00Z"),
            finding_run(
                "run_unit0",
                "unit",
                Severity::Medium,
                "2026-01-02T00:00:00Z",
            ),
            other,
        ];
        let ids = |q: &str| -> Vec<String> {
            let parsed = rupu_coverage::parse_query(q).unwrap();
            query_findings(&store, all.clone(), &parsed)
                .into_iter()
                .map(|f| f.record.id)
                .collect()
        };
        assert_eq!(ids("run:run_parent"), ["top", "unit"]);
        assert_eq!(ids("project:proj"), ["top", "unit"]);
        assert_eq!(ids("project:elsewhere"), ["nope"]);
        assert_eq!(ids(""), ["nope", "top", "unit"]);
    }

    /// `workflow:` matches the joined workflow name: callers join with
    /// [`join_workflow_names`] first, and the join leaves a row that already
    /// carries a name (or whose run can't be loaded) as it is.
    #[test]
    fn query_findings_matches_workflow_after_the_join() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        store
            .create(run_record("run_wf", "audit-flow"), "name: t\nsteps: []\n")
            .unwrap();
        let mut named = finding_run("run_named", "named", Severity::Low, "2026-01-03T00:00:00Z");
        named.workflow_name = Some("audit-flow".to_string());
        let mut all = vec![
            finding_run("run_wf", "joined", Severity::High, "2026-01-01T00:00:00Z"),
            finding_run("run_gone", "gone", Severity::Medium, "2026-01-02T00:00:00Z"),
            named,
        ];
        join_workflow_names(&store, &mut all);
        assert_eq!(all[0].workflow_name.as_deref(), Some("audit-flow"));
        assert_eq!(all[1].workflow_name, None);
        assert_eq!(all[2].workflow_name.as_deref(), Some("audit-flow"));
        let ids = |q: &str| -> Vec<String> {
            let parsed = rupu_coverage::parse_query(q).unwrap();
            query_findings(&store, all.clone(), &parsed)
                .into_iter()
                .map(|f| f.record.id)
                .collect()
        };
        assert_eq!(ids("workflow:audit-flow"), ["joined", "named"]);
        assert_eq!(ids("-workflow:audit-flow"), ["gone"]);
    }

    #[test]
    fn permalink_built_from_github_remote() {
        // Given a workspace remote + branch and a finding with file+lines,
        // the FindingOut permalink is the github blob URL.
        let url = rupu_scm::weburl::repo_permalink(
            "git@github.com:o/r.git",
            Some("main"),
            "src/a.rs",
            Some([17, 19]),
        );
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/o/r/blob/main/src/a.rs#L17-L19")
        );
    }

    /// End-to-end wiring check (not just the pure `repo_permalink` fn):
    /// `collect_all_findings` reads a real on-disk workspace TOML with a
    /// github `repo_remote` + `initial_branch`, discovers a coverage target
    /// with one finding that has `file_path`/`line_range`, and the resulting
    /// `FindingOut.permalink` is the exact github blob URL. Also covers the
    /// negative case: a finding with no `file_path` gets `permalink: None`.
    #[test]
    fn collect_all_findings_computes_permalink_from_workspace_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        // Workspace record with a github remote + branch.
        let ws = rupu_workspace::Workspace {
            id: "ws1".to_string(),
            path: repo.to_str().unwrap().to_string(),
            repo_remote: Some("git@github.com:o/r.git".to_string()),
            initial_branch: Some("main".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        };
        let workspaces_dir = global.join("workspaces");
        std::fs::create_dir_all(&workspaces_dir).unwrap();
        std::fs::write(
            workspaces_dir.join("ws1.toml"),
            toml::to_string(&ws).unwrap(),
        )
        .unwrap();

        // One coverage target with two findings: one with file+lines (gets a
        // permalink), one without a file_path (stays None).
        let paths = CoveragePaths::new(&repo, "tgt1");
        paths.ensure_dir().unwrap();
        let with_loc = FindingRecord {
            id: "fnd_with_loc".to_string(),
            file_path: Some("src/a.rs".to_string()),
            line_range: Some([17, 19]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: "s".to_string(),
            severity: Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".to_string(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: at("2026-01-01T00:00:00Z"),
            profile: rupu_coverage::FindingProfile::Summary,
            report: None,
            tags: Vec::new(),
        };
        let mut without_loc = with_loc.clone();
        without_loc.id = "fnd_no_loc".to_string();
        without_loc.file_path = None;
        without_loc.line_range = None;
        without_loc.declared_by.codename = Some("cobalt-harbor/heron#3".to_string());

        let jsonl = format!(
            "{}\n{}\n",
            serde_json::to_string(&with_loc).unwrap(),
            serde_json::to_string(&without_loc).unwrap()
        );
        std::fs::write(&paths.findings, jsonl).unwrap();

        let out = collect_all_findings(global);
        assert_eq!(out.len(), 2);
        let by_id = |id: &str| out.iter().find(|f| f.record.id == id).unwrap();
        assert_eq!(
            by_id("fnd_with_loc").permalink.as_deref(),
            Some("https://github.com/o/r/blob/main/src/a.rs#L17-L19")
        );
        assert_eq!(by_id("fnd_no_loc").permalink, None);

        // Legacy finding (no stored codename) -> crew derived from run_id.
        let legacy = by_id("fnd_with_loc");
        assert_eq!(
            legacy.codename,
            rupu_codename::derive_legacy("run_01KS19A4MQXP", None)
        );
        assert!(legacy.codename_derived);
        // Stored `declared_by.codename` wins.
        let stored = by_id("fnd_no_loc");
        assert_eq!(stored.codename, "cobalt-harbor/heron#3");
        assert!(!stored.codename_derived);
    }

    /// `finding_ledgers` maps each finding id to the coverage paths of the
    /// ledger holding it, using the same walk as `collect_all_findings`.
    #[test]
    fn finding_ledgers_maps_ids_to_their_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let ws = rupu_workspace::Workspace {
            id: "ws1".to_string(),
            path: repo.to_str().unwrap().to_string(),
            repo_remote: None,
            initial_branch: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        };
        std::fs::create_dir_all(global.join("workspaces")).unwrap();
        std::fs::write(
            global.join("workspaces").join("ws1.toml"),
            toml::to_string(&ws).unwrap(),
        )
        .unwrap();

        let paths = CoveragePaths::new(&repo, "tgt1");
        paths.ensure_dir().unwrap();
        let a = finding("fnd_a", Severity::High, "2026-01-01T00:00:00Z").record;
        let b = finding("fnd_b", Severity::Low, "2026-01-02T00:00:00Z").record;
        std::fs::write(
            &paths.findings,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&a).unwrap(),
                serde_json::to_string(&b).unwrap()
            ),
        )
        .unwrap();

        let map = finding_ledgers(global);
        assert_eq!(map.len(), 2);
        for id in ["fnd_a", "fnd_b"] {
            let held = &map[id];
            assert_eq!(held.len(), 1, "{id}");
            assert_eq!(held[0].findings, paths.findings, "{id}");
        }
        assert!(!map.contains_key("fnd_missing"));
        // An empty registry yields an empty map rather than an error.
        let empty = tempfile::tempdir().unwrap();
        assert!(finding_ledgers(empty.path()).is_empty());
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

    fn full_record(id: &str) -> FindingRecord {
        let report: rupu_coverage::FindingReport = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let mut rec = finding(id, Severity::Critical, "2026-09-29T00:00:00Z").record;
        rec.profile = rupu_coverage::FindingProfile::Full;
        rec.summary = report.title.clone();
        rec.report = Some(report);
        rec
    }

    #[test]
    fn list_rows_carry_report_summary_not_report() {
        let mut out = finding("f1", Severity::Critical, "2026-09-29T00:00:00Z");
        out.record = full_record("f1");
        let row = out.into_list_row();
        assert!(row.record.report.is_none());
        let s = row
            .report_summary
            .clone()
            .expect("summary for full-profile rows");
        assert_eq!(s.completeness.total, 11);
        let json = serde_json::to_value(&row).unwrap();
        assert!(json.get("report").is_none());
        assert!(json["report_summary"]["root_cause"].is_string());
    }

    #[test]
    fn summary_rows_have_no_report_summary_key() {
        let row = finding("f2", Severity::High, "2026-09-29T00:00:00Z").into_list_row();
        let json = serde_json::to_value(&row).unwrap();
        assert!(json.get("report_summary").is_none());
    }

    #[test]
    fn claim_states_compare_file_hashes() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(ws.path().join("src/routes/notes.rs"), "fn a() {}\n").unwrap();
        let mut report = full_record("f3").report.unwrap();
        let current =
            rupu_coverage::report::sha256_file(&ws.path().join("src/routes/notes.rs")).unwrap();
        report.evidence[0].sha256 = Some(current.clone());
        let mut changed = report.evidence[0].clone();
        changed.sha256 = Some("0".repeat(64));
        let mut missing = report.evidence[0].clone();
        missing.file = Some("src/gone.rs".into());
        let mut unknown = report.evidence[0].clone();
        unknown.sha256 = None;
        report.evidence = vec![report.evidence[0].clone(), changed, missing, unknown];
        assert_eq!(
            claim_states(ws.path(), &report),
            vec![
                ClaimState::Current,
                ClaimState::Changed,
                ClaimState::Missing,
                ClaimState::Unknown
            ]
        );
    }

    /// Register a workspace at `<global>/repo` and write `records` as the
    /// findings ledger of one coverage target under it.
    fn seed_workspace_findings(
        global: &std::path::Path,
        records: &[FindingRecord],
    ) -> std::path::PathBuf {
        let repo = global.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let ws = rupu_workspace::Workspace {
            id: "ws1".to_string(),
            path: repo.to_str().unwrap().to_string(),
            repo_remote: None,
            initial_branch: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        };
        let workspaces_dir = global.join("workspaces");
        std::fs::create_dir_all(&workspaces_dir).unwrap();
        std::fs::write(
            workspaces_dir.join("ws1.toml"),
            toml::to_string(&ws).unwrap(),
        )
        .unwrap();
        let paths = CoveragePaths::new(&repo, "tgt1");
        paths.ensure_dir().unwrap();
        let jsonl: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.findings, jsonl).unwrap();
        repo
    }

    async fn get_json(app: Router, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
        use tower::ServiceExt as _;
        let req = axum::http::Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn get_finding_serves_report_and_evidence_status() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut rec = full_record("fnd_detail");
        // Pin the first claim's hash to a real file so it reports `current`.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src/routes")).unwrap();
        std::fs::write(repo.join("src/routes/notes.rs"), "fn a() {}\n").unwrap();
        {
            let report = rec.report.as_mut().unwrap();
            let file = report.evidence[0].file.clone().unwrap();
            let sha = rupu_coverage::report::sha256_file(&repo.join(&file)).unwrap();
            report.evidence[0].sha256 = Some(sha);
        }
        let evidence_len = rec.report.as_ref().unwrap().evidence.len();
        seed_workspace_findings(tmp.path(), &[rec, full_record("fnd_other")]);

        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let app = routes().with_state(state);

        let (status, json) = get_json(app.clone(), "/api/findings/fnd_detail").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["id"], "fnd_detail");
        assert!(json["report"]["title"].is_string());
        let ev = json["evidence_status"].as_array().unwrap();
        assert_eq!(ev.len(), evidence_len);
        assert_eq!(ev[0], "current");
        // The detail endpoint serves the body, not the list-row summary.
        assert!(json.get("report_summary").is_none());

        let (status, json) = get_json(app.clone(), "/api/findings/fnd_nope").await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "finding fnd_nope not found");

        // The list endpoint slims the same finding to a summary.
        let (status, json) = get_json(app, "/api/findings").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let rows = json["findings"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert!(row.get("report").is_none());
            assert!(row["report_summary"]["root_cause"].is_string());
        }
    }

    #[tokio::test]
    async fn get_finding_adds_exact_hex_for_64_bit_addresses() {
        use rupu_coverage::report::{ArtifactRef, DisasmLine, EvidenceBlock};
        let tmp = tempfile::TempDir::new().unwrap();
        let mut rec = full_record("fnd_hex");
        rec.report.as_mut().unwrap().blocks = vec![
            EvidenceBlock::Hexdump {
                base: 0xffff_ffff_8123_4567,
                artifact: ArtifactRef {
                    path: "dump.bin".into(),
                    sha256: String::new(),
                    size: 0,
                    kind: None,
                    stored: None,
                    host: None,
                },
                rendered: None,
            },
            EvidenceBlock::Disasm {
                arch: "x86_64".into(),
                listing: vec![
                    DisasmLine {
                        address: 0xffff_ffff_8123_4567,
                        bytes: "90".into(),
                        mnemonic: "nop".into(),
                        ops: String::new(),
                    },
                    DisasmLine {
                        address: 0x40,
                        bytes: "c3".into(),
                        mnemonic: "ret".into(),
                        ops: String::new(),
                    },
                ],
            },
        ];
        seed_workspace_findings(tmp.path(), &[rec]);
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let app = routes().with_state(state);
        let (status, json) = get_json(app, "/api/findings/fnd_hex").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let blocks = &json["report"]["blocks"];
        assert_eq!(blocks[0]["base_hex"], "0xffffffff81234567");
        assert_eq!(blocks[1]["listing"][0]["address_hex"], "0xffffffff81234567");
        assert_eq!(blocks[1]["listing"][1]["address_hex"], "0x40");
    }

    #[test]
    fn claim_states_reject_paths_that_escape_the_workspace() {
        let root = tempfile::TempDir::new().unwrap();
        let ws = root.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let outside = root.path().join("outside.rs");
        std::fs::write(&outside, "fn secret() {}\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&outside).unwrap();

        let mut report = full_record("f4").report.unwrap();
        let mut dotdot = report.evidence[0].clone();
        dotdot.file = Some("../outside.rs".into());
        dotdot.sha256 = Some(sha.clone());
        let mut absolute = report.evidence[0].clone();
        absolute.file = Some(outside.to_str().unwrap().to_string());
        absolute.sha256 = Some(sha);
        report.evidence = vec![dotdot, absolute];
        // The files exist and hash correctly: only the escape check can
        // explain a `Missing` (never `Current`).
        assert_eq!(
            claim_states(&ws, &report),
            vec![ClaimState::Missing, ClaimState::Missing]
        );
    }

    #[test]
    fn claim_states_skip_hashing_files_over_the_size_cap() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("big.bin"), vec![7u8; 64]).unwrap();
        let sha = rupu_coverage::report::sha256_file(&ws.path().join("big.bin")).unwrap();
        let mut report = full_record("f5").report.unwrap();
        report.evidence[0].file = Some("big.bin".into());
        report.evidence[0].sha256 = Some(sha);
        report.evidence.truncate(1);
        assert_eq!(
            claim_states_capped(ws.path(), &report, 63),
            vec![ClaimState::Unknown]
        );
        assert_eq!(
            claim_states_capped(ws.path(), &report, 64),
            vec![ClaimState::Current]
        );
    }

    // Claims cite source files: far below the 500 MiB artifact cap.
    const _: () = assert!(CLAIM_HASH_MAX_BYTES < rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES);

    #[test]
    fn claim_states_use_the_dedicated_claim_hash_cap_not_the_artifact_cap() {
        assert_eq!(CLAIM_HASH_MAX_BYTES, 64 * 1024 * 1024);
        let ws = tempfile::TempDir::new().unwrap();
        // A sparse file one byte over the cap: never read, so this stays cheap.
        let big = std::fs::File::create(ws.path().join("big.log")).unwrap();
        big.set_len(CLAIM_HASH_MAX_BYTES + 1).unwrap();
        drop(big);
        let mut report = full_record("f6").report.unwrap();
        report.evidence[0].file = Some("big.log".into());
        report.evidence[0].sha256 = Some("0".repeat(64));
        report.evidence.truncate(1);
        assert_eq!(claim_states(ws.path(), &report), vec![ClaimState::Unknown]);
    }

    #[tokio::test]
    async fn get_finding_with_an_unloadable_workspace_reports_every_claim_unknown() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Every claim points at a real file with its real hash, so a loadable
        // workspace would report `current` for all of them: `unknown` below can
        // only come from the workspace failing to load.
        let mut rec = full_record("fnd_orphan");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "fn a() {}\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&repo.join("src/a.rs")).unwrap();
        {
            let report = rec.report.as_mut().unwrap();
            for c in &mut report.evidence {
                c.file = Some("src/a.rs".into());
                c.sha256 = Some(sha.clone());
            }
        }
        let claims = rec.report.as_ref().unwrap().evidence.len();
        assert!(claims > 0);
        seed_workspace_findings(tmp.path(), &[rec]);
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );

        let (status, ok) = get_json(
            routes().with_state(state.clone()),
            "/api/findings/fnd_orphan",
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            ok["evidence_status"],
            serde_json::json!(vec!["current"; claims])
        );

        // Rename the record so the store still LISTS the workspace (the
        // finding stays discoverable through its ledger) but `load(ws_id)`,
        // which reads `<id>.toml`, no longer finds it.
        let dir = tmp.path().join("workspaces");
        std::fs::rename(dir.join("ws1.toml"), dir.join("moved.toml")).unwrap();
        let (status, json) = get_json(routes().with_state(state), "/api/findings/fnd_orphan").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["id"], "fnd_orphan");
        assert_eq!(
            json["evidence_status"],
            serde_json::json!(vec!["unknown"; claims])
        );
    }

    #[tokio::test]
    async fn get_finding_for_a_summary_finding_has_empty_evidence_status() {
        let tmp = tempfile::TempDir::new().unwrap();
        let summary = finding("fnd_summary", Severity::High, "2026-09-29T00:00:00Z").record;
        seed_workspace_findings(tmp.path(), &[summary]);
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        let (status, json) =
            get_json(routes().with_state(state), "/api/findings/fnd_summary").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["id"], "fnd_summary");
        assert_eq!(json["evidence_status"], serde_json::json!([]));
        assert!(json.get("report").is_none());
    }

    // ---- GET /api/findings/:id/artifacts/:sha256 ----

    use rupu_coverage::report::{ArtifactKind, ArtifactRef, ArtifactStorage, ArtifactStore};

    fn artifact_ref(
        path: &str,
        sha: &str,
        size: u64,
        kind: Option<ArtifactKind>,
        stored: Option<ArtifactStorage>,
        host: Option<&str>,
    ) -> ArtifactRef {
        ArtifactRef {
            path: path.to_string(),
            sha256: sha.to_string(),
            size,
            kind,
            stored,
            host: host.map(str::to_string),
        }
    }

    /// Copy `bytes` into the content-addressed store under
    /// `<global>/findings/artifacts` and return its sha256.
    fn store_blob(global: &std::path::Path, bytes: &[u8]) -> String {
        let scratch = global.join("blob-scratch.tmp");
        std::fs::write(&scratch, bytes).unwrap();
        let sha = rupu_coverage::report::sha256_file(&scratch).unwrap();
        let dest = ArtifactStore::new(global.join("findings").join("artifacts")).blob_path(&sha);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&scratch, &dest).unwrap();
        std::fs::remove_file(&scratch).unwrap();
        sha
    }

    /// Seed workspace `ws1` with one full-profile finding `fnd_art` whose
    /// report lists exactly `artifacts`.
    fn seed_artifact_finding(global: &std::path::Path, artifacts: Vec<ArtifactRef>) {
        let mut rec = full_record("fnd_art");
        rec.report.as_mut().unwrap().artifacts = artifacts;
        seed_workspace_findings(global, &[rec]);
    }

    fn app_for(global: &std::path::Path) -> Router {
        let state = AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default());
        routes().with_state(state)
    }

    async fn get_raw(
        app: Router,
        uri: &str,
    ) -> (axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>) {
        use tower::ServiceExt as _;
        let req = axum::http::Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
    }

    fn error_of(body: &[u8]) -> String {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn header_str<'a>(h: &'a axum::http::HeaderMap, name: &str) -> &'a str {
        h.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
    }

    #[tokio::test]
    async fn artifact_copied_text_is_served_inline_as_plain_text() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body = b"<html><script>alert(1)</script></html>\n";
        let sha = store_blob(tmp.path(), body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "logs/notes.txt",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        // Even HTML-looking text is plain text, never rendered.
        assert!(header_str(&headers, "content-type").starts_with("text/plain"));
        assert_eq!(header_str(&headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&headers, "content-security-policy"), "sandbox");
        let disp = header_str(&headers, "content-disposition");
        assert!(disp.starts_with("inline"), "disposition: {disp}");
        assert!(
            disp.contains("filename=\"notes.txt\""),
            "disposition: {disp}"
        );
    }

    #[tokio::test]
    async fn artifact_copied_binary_is_an_octet_stream_attachment() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body: &[u8] = &[0, 159, 146, 150, 1, 2, 255];
        let sha = store_blob(tmp.path(), body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "out/capture.pcap",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        assert_eq!(
            header_str(&headers, "content-type"),
            "application/octet-stream"
        );
        assert_eq!(header_str(&headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&headers, "content-security-policy"), "sandbox");
        let disp = header_str(&headers, "content-disposition");
        assert!(disp.starts_with("attachment"), "disposition: {disp}");
        assert!(
            disp.contains("filename=\"capture.pcap\""),
            "disposition: {disp}"
        );
    }

    #[tokio::test]
    async fn artifact_not_listed_on_the_finding_is_404_even_if_the_blob_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let listed = store_blob(tmp.path(), b"listed\n");
        // A blob that is in the store (e.g. another finding's artifact) but is
        // not referenced by `fnd_art`.
        let unlisted = store_blob(tmp.path(), b"someone else's secret\n");
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "a.txt",
                &listed,
                7,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{unlisted}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            error_of(&bytes),
            "this finding does not reference that artifact"
        );
    }

    #[tokio::test]
    async fn artifact_unknown_finding_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = store_blob(tmp.path(), b"x\n");
        seed_artifact_finding(tmp.path(), vec![]);
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_nope/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error_of(&bytes), "finding fnd_nope not found");
    }

    #[tokio::test]
    async fn artifact_malformed_sha_is_400() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = store_blob(tmp.path(), b"x\n");
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "x.txt",
                &sha,
                2,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let app = app_for(tmp.path());
        for bad in [
            sha.to_uppercase(),
            sha[..63].to_string(),
            format!("{sha}0"),
            "z".repeat(64),
            "abc".to_string(),
        ] {
            let (status, _, _) = get_raw(
                app.clone(),
                &format!("/api/findings/fnd_art/artifacts/{bad}"),
            )
            .await;
            assert_eq!(
                status,
                axum::http::StatusCode::BAD_REQUEST,
                "sha `{bad}` must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn artifact_copied_but_missing_from_the_store_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Listed, but no blob was ever written to the store.
        let sha = "a".repeat(64);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "gone.txt",
                &sha,
                1,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error_of(&bytes), "artifact blob missing from the store");
    }

    fn unavailable_of(body: &[u8]) -> Option<String> {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["unavailable"].as_str().map(str::to_string))
    }

    #[tokio::test]
    async fn artifact_external_on_an_unregistered_host_is_404_unavailable_naming_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = "b".repeat(64);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "/var/log/big.bin",
                &sha,
                // Under the coordinator's own size limit, which is refused
                // before the host is even resolved.
                4_096,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::External),
                Some("build-box-7"),
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        let reason = unavailable_of(&bytes).expect("an `unavailable` body");
        assert!(reason.contains("build-box-7"), "reason: {reason}");
        assert!(reason.contains("not registered"), "reason: {reason}");
        assert_eq!(error_of(&bytes), "", "not a plain `error` body");
        assert!(header_str(&headers, "content-type").starts_with("application/json"));
        // Nothing was pulled, so nothing was stored or left behind.
        let store = tmp.path().join("findings").join("artifacts");
        assert!(!store.exists() || std::fs::read_dir(&store).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn artifact_external_on_a_host_already_in_the_store_is_served_without_contacting_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let body: &[u8] = &[0, 1, 2, 0xfe, 0xff];
        let sha = store_blob(tmp.path(), body);
        // `build-box-7` is not registered: any attempt to reach it would be an
        // `unavailable` 404, so a 200 proves the store answered alone.
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "out/dump.bin",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::External),
                Some("build-box-7"),
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        assert_eq!(
            header_str(&headers, "content-type"),
            "application/octet-stream"
        );
        assert_eq!(header_str(&headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&headers, "content-security-policy"), "sandbox");
        assert_eq!(
            header_str(&headers, "content-disposition"),
            "attachment; filename=\"dump.bin\""
        );
        assert_eq!(header_str(&headers, "content-length"), "5");
    }

    #[tokio::test]
    async fn artifact_responses_declare_the_served_handles_length() {
        // A store blob.
        let tmp = tempfile::TempDir::new().unwrap();
        let body = b"twelve bytes";
        let sha = store_blob(tmp.path(), body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "a.txt",
                &sha,
                12,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, _) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&headers, "content-length"), "12");

        // A local external file, whose length comes off the verified handle.
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("big.log"), b"seventeen bytes!\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&repo.join("big.log")).unwrap();
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "big.log",
                &sha,
                17,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        let (status, headers, _) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&headers, "content-length"), "17");

        // The host blob endpoint, too (a coordinator's HTTP pull refuses an
        // oversize blob off this header before writing a byte).
        let (status, headers, _) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/artifacts/{}", store_blob(tmp.path(), b"abc")),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&headers, "content-length"), "3");
    }

    // ---- raster images served inline; evidence-block files servable ----

    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    #[tokio::test]
    async fn a_png_artifact_is_served_inline_as_an_image() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut body = PNG_MAGIC.to_vec();
        body.extend_from_slice(b"fake image body");
        let sha = store_blob(tmp.path(), &body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "shots/crash.png",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&headers, "content-type"), "image/png");
        assert!(header_str(&headers, "content-disposition").starts_with("inline"));
        assert_eq!(header_str(&headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&headers, "content-security-policy"), "sandbox");
        assert_eq!(bytes, body, "the peek must not consume bytes");
        assert_eq!(
            header_str(&headers, "content-length"),
            body.len().to_string()
        );
    }

    #[tokio::test]
    async fn non_images_and_text_are_never_typed_as_images() {
        // A binary that is not a raster image stays an attachment.
        let tmp = tempfile::TempDir::new().unwrap();
        let body = b"%PDF-1.7 not an image";
        let sha = store_blob(tmp.path(), body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "doc.pdf",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        assert_eq!(
            header_str(&headers, "content-type"),
            "application/octet-stream"
        );
        assert!(header_str(&headers, "content-disposition").starts_with("attachment"));

        // `Text` is never sniffed, even with a PNG head.
        let tmp = tempfile::TempDir::new().unwrap();
        let mut body = PNG_MAGIC.to_vec();
        body.extend_from_slice(b" but declared text");
        let sha = store_blob(tmp.path(), &body);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "notes.txt",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        assert!(header_str(&headers, "content-type").starts_with("text/plain"));
    }

    #[tokio::test]
    async fn an_evidence_block_artifact_is_servable_by_its_sha() {
        use rupu_coverage::report::EvidenceBlock;
        let tmp = tempfile::TempDir::new().unwrap();
        let mut body = PNG_MAGIC.to_vec();
        body.extend_from_slice(b"block image");
        let sha = store_blob(tmp.path(), &body);
        let mut rec = full_record("fnd_art");
        {
            let report = rec.report.as_mut().unwrap();
            report.artifacts = vec![];
            report.blocks.push(EvidenceBlock::Image {
                artifact: artifact_ref(
                    "shots/block.png",
                    &sha,
                    body.len() as u64,
                    Some(ArtifactKind::Binary),
                    Some(ArtifactStorage::Copied),
                    None,
                ),
                caption: None,
            });
        }
        seed_workspace_findings(tmp.path(), &[rec]);

        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&headers, "content-type"), "image/png");
        assert_eq!(bytes, body);

        // A sha in neither `artifacts` nor any block is still refused.
        let other = "c".repeat(64);
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{other}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            error_of(&bytes),
            "this finding does not reference that artifact"
        );
    }

    // ---- the shared, cancel-safe in-flight pull ----

    use std::sync::atomic::{AtomicU32, Ordering};

    /// A pull that counts its runs, parks until `release` fires, then
    /// returns `out`.
    fn gated_pull(
        runs: &Arc<AtomicU32>,
        release: &Arc<tokio::sync::Notify>,
        out: PullOutcome,
    ) -> impl FnOnce() -> futures_util::future::BoxFuture<'static, PullOutcome> {
        let (runs, release) = (Arc::clone(runs), Arc::clone(release));
        move || {
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                out
            })
        }
    }

    /// Poll (bounded) until `dest` has no in-flight pull.
    async fn until_retired(dest: &std::path::Path) {
        for _ in 0..500 {
            if !pull_in_flight(dest) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the pull into {} never retired", dest.display());
    }

    fn unique_dest(tag: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(format!(
            "/nonexistent/test-pulls/{tag}-{}",
            ulid::Ulid::new()
        ))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_pull_concurrent_callers_share_one_pull() {
        let dest = unique_dest("share");
        let runs = Arc::new(AtomicU32::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let a = tokio::spawn(shared_pull(
            dest.clone(),
            gated_pull(&runs, &release, Ok(())),
        ));
        // `a` is in flight before `b` asks.
        while !pull_in_flight(&dest) {
            tokio::task::yield_now().await;
        }
        let b = tokio::spawn(shared_pull(
            dest.clone(),
            gated_pull(&runs, &release, Err("a second pull ran".into())),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        release.notify_one();
        assert_eq!(a.await.unwrap(), Ok(()));
        assert_eq!(b.await.unwrap(), Ok(()), "b joined a's pull");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        until_retired(&dest).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_pull_outlives_every_caller_and_retires_itself() {
        let dest = unique_dest("cancel");
        let runs = Arc::new(AtomicU32::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(AtomicU32::new(0));
        let make = {
            let (runs, release, finished) = (
                Arc::clone(&runs),
                Arc::clone(&release),
                Arc::clone(&finished),
            );
            move || -> futures_util::future::BoxFuture<'static, PullOutcome> {
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    release.notified().await;
                    finished.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        };
        // The only caller gives up (a browser disconnecting drops the
        // handler's future the same way).
        let caller = tokio::spawn(shared_pull(dest.clone(), make));
        while runs.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(pull_in_flight(&dest), "the pull keeps running");
        // A request arriving now joins it rather than starting another.
        let late = tokio::spawn(shared_pull(
            dest.clone(),
            gated_pull(&runs, &release, Err("a second pull ran".into())),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        release.notify_one();
        assert_eq!(late.await.unwrap(), Ok(()));
        assert_eq!(finished.load(Ordering::SeqCst), 1, "ran to completion");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        until_retired(&dest).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_pull_holds_in_flight_pulls_only() {
        let dest = unique_dest("again");
        let runs = Arc::new(AtomicU32::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let first = tokio::spawn(shared_pull(
            dest.clone(),
            gated_pull(&runs, &release, Err("host offline".into())),
        ));
        while runs.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        release.notify_one();
        assert_eq!(first.await.unwrap(), Err("host offline".to_string()));
        until_retired(&dest).await;
        // A failure is not remembered: the next view pulls again.
        let again = shared_pull(
            dest.clone(),
            || -> futures_util::future::BoxFuture<'static, PullOutcome> {
                Box::pin(async { Ok(()) })
            },
        )
        .await;
        assert_eq!(again, Ok(()));
        until_retired(&dest).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_pull_a_panicking_pull_fails_its_waiters_and_retires() {
        let dest = unique_dest("panic");
        let out = shared_pull(
            dest.clone(),
            || -> futures_util::future::BoxFuture<'static, PullOutcome> {
                Box::pin(async { panic!("the pull blew up") })
            },
        )
        .await;
        let reason = out.unwrap_err();
        assert!(reason.contains("did not finish"), "{reason}");
        until_retired(&dest).await;
    }

    #[tokio::test]
    async fn artifact_external_local_streams_when_the_workspace_file_is_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("dumps")).unwrap();
        let body = b"external but intact\n";
        std::fs::write(repo.join("dumps/big.log"), body).unwrap();
        let sha = rupu_coverage::report::sha256_file(&repo.join("dumps/big.log")).unwrap();
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "dumps/big.log",
                &sha,
                body.len() as u64,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(bytes, body);
        assert!(header_str(&headers, "content-type").starts_with("text/plain"));
        assert_eq!(header_str(&headers, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&headers, "content-security-policy"), "sandbox");
    }

    #[tokio::test]
    async fn artifact_external_local_changed_since_recording_is_409() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("dump.log"), b"as recorded\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&repo.join("dump.log")).unwrap();
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "dump.log",
                &sha,
                12,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        // The workspace file is edited after the finding was recorded.
        std::fs::write(repo.join("dump.log"), b"edited later\n").unwrap();
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            error_of(&bytes),
            "artifact changed since the finding was recorded"
        );
    }

    #[tokio::test]
    async fn artifact_external_local_missing_from_the_workspace_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = "c".repeat(64);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "deleted.log",
                &sha,
                3,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error_of(&bytes), "artifact is no longer in the workspace");
    }

    /// Seed a finding whose only artifact is an EXTERNAL local one at
    /// `artifact_path`, whose recorded hash is `outside`'s real hash (so a
    /// refusal can only come from the containment checks), then request it.
    /// Asserts a 404 that leaks none of `outside`'s bytes.
    async fn assert_outside_file_is_refused(
        global: &std::path::Path,
        artifact_path: &str,
        outside: &std::path::Path,
    ) {
        let secret = std::fs::read(outside).unwrap();
        let sha = rupu_coverage::report::sha256_file(outside).unwrap();
        seed_artifact_finding(
            global,
            vec![artifact_ref(
                artifact_path,
                &sha,
                secret.len() as u64,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        let (status, headers, bytes) = get_raw(
            app_for(global),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error_of(&bytes), "artifact is no longer in the workspace");
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_slice()));
        assert_ne!(
            header_str(&headers, "content-type"),
            "text/plain; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn artifact_external_local_relative_dotdot_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        // `<global>/outside.txt` sits next to the workspace at `<global>/repo`.
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"not yours\n").unwrap();
        assert_outside_file_is_refused(tmp.path(), "../outside.txt", &outside).await;
    }

    #[tokio::test]
    async fn artifact_external_local_absolute_path_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"not yours\n").unwrap();
        assert_outside_file_is_refused(tmp.path(), outside.to_str().unwrap(), &outside).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn artifact_external_local_symlink_pointing_outside_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"not yours\n").unwrap();
        // A perfectly relative, `..`-free path that is a symlink INSIDE the
        // workspace pointing OUTSIDE it: only canonicalize + starts_with can
        // catch this one.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("link.txt")).unwrap();
        // The link really does reach the outside bytes.
        assert_eq!(
            std::fs::read(repo.join("link.txt")).unwrap(),
            b"not yours\n"
        );
        assert_outside_file_is_refused(tmp.path(), "link.txt", &outside).await;
    }

    #[tokio::test]
    async fn artifact_external_local_same_size_edit_is_409() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("dump.log"), b"as recorded\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&repo.join("dump.log")).unwrap();
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "dump.log",
                &sha,
                12,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::External),
                None,
            )],
        );
        // Same length, different bytes: only the hash can tell.
        std::fs::write(repo.join("dump.log"), b"AS recorded\n").unwrap();
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            error_of(&bytes),
            "artifact changed since the finding was recorded"
        );
    }

    #[tokio::test]
    async fn artifact_filename_quotes_are_sanitized_in_the_disposition() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = store_blob(tmp.path(), b"hi\n");
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "dir/evil\"; x=\"y.txt",
                &sha,
                3,
                Some(ArtifactKind::Text),
                Some(ArtifactStorage::Copied),
                None,
            )],
        );
        let (status, headers, _) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            header_str(&headers, "content-disposition"),
            "inline; filename=\"evil_; x=_y.txt\""
        );
    }

    #[test]
    fn safe_filename_takes_the_basename_and_neutralizes_header_breakers() {
        assert_eq!(safe_filename("a/b/notes.txt"), "notes.txt");
        assert_eq!(safe_filename("notes.txt"), "notes.txt");
        assert_eq!(safe_filename("we\"ird\\name.txt"), "we_ird_name.txt");
        assert_eq!(safe_filename("x\r\ny\tz.txt"), "x__y_z.txt");
        // A trailing slash leaves nothing to name the download with.
        assert_eq!(safe_filename("dir/"), "artifact");
        assert_eq!(safe_filename(""), "artifact");
    }

    #[test]
    fn open_verified_hands_back_the_hashed_handle_rewound_to_the_start() {
        use std::io::Read as _;
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.log");
        let body = b"hashed and served from one handle\n";
        std::fs::write(&p, body).unwrap();
        let sha = rupu_coverage::report::sha256_file(&p).unwrap();
        // Hashing leaves the cursor at EOF; the returned handle must serve the
        // whole file, not the empty tail.
        let mut f = open_verified(&p, &sha, body.len() as u64).unwrap();
        let mut got = Vec::new();
        f.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        // A recorded size of 0 means "not recorded": only the hash decides.
        assert!(open_verified(&p, &sha, 0).is_ok());
    }

    #[test]
    fn open_verified_refuses_changed_missing_and_non_regular_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.log");
        std::fs::write(&p, b"as recorded\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&p).unwrap();

        // Same size, different bytes: caught by the hash.
        std::fs::write(&p, b"AS recorded\n").unwrap();
        assert_eq!(
            open_verified(&p, &sha, 12).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        // Different size: caught before hashing.
        std::fs::write(&p, b"longer than recorded\n").unwrap();
        assert_eq!(
            open_verified(&p, &sha, 12).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        // Gone.
        assert_eq!(
            open_verified(&dir.path().join("nope.log"), &sha, 12)
                .unwrap_err()
                .0,
            axum::http::StatusCode::NOT_FOUND
        );
        // Not a regular file (a directory opens fine on unix; the handle's own
        // metadata is what refuses it).
        assert_eq!(
            open_verified(dir.path(), &sha, 0).unwrap_err().0,
            axum::http::StatusCode::NOT_FOUND
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_verified_handle_keeps_the_hashed_bytes_if_the_path_is_repointed_after() {
        use std::io::Read as _;
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.log");
        let elsewhere = dir.path().join("elsewhere.txt");
        std::fs::write(&p, b"the hashed bytes\n").unwrap();
        std::fs::write(&elsewhere, b"an arbitrary other file\n").unwrap();
        let sha = rupu_coverage::report::sha256_file(&p).unwrap();
        let mut f = open_verified(&p, &sha, 17).unwrap();
        // The swap an agent-writable workspace allows between check and stream.
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &p).unwrap();
        let mut got = String::new();
        f.read_to_string(&mut got).unwrap();
        assert_eq!(got, "the hashed bytes\n");
    }

    // ── report exports ────────────────────────────────────────────────────────

    /// Like `seed_workspace_findings`, for any workspace id and directory.
    fn seed_named_workspace(
        global: &std::path::Path,
        ws_id: &str,
        dir: &str,
        records: &[FindingRecord],
    ) {
        let repo = global.join(dir);
        std::fs::create_dir_all(&repo).unwrap();
        let ws = rupu_workspace::Workspace {
            id: ws_id.to_string(),
            path: repo.to_str().unwrap().to_string(),
            repo_remote: None,
            initial_branch: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        };
        let workspaces_dir = global.join("workspaces");
        std::fs::create_dir_all(&workspaces_dir).unwrap();
        std::fs::write(
            workspaces_dir.join(format!("{ws_id}.toml")),
            toml::to_string(&ws).unwrap(),
        )
        .unwrap();
        let paths = CoveragePaths::new(&repo, "tgt1");
        paths.ensure_dir().unwrap();
        let jsonl: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.findings, jsonl).unwrap();
    }

    /// A full-profile finding with a chosen severity and declaring run.
    fn full_in_run(id: &str, sev: Severity, run: &str) -> FindingRecord {
        let mut r = full_record(id);
        r.severity = sev;
        r.declared_by = attribution_run(run);
        r
    }

    async fn post_raw(
        app: Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, axum::http::HeaderMap, Vec<u8>) {
        use tower::ServiceExt as _;
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
    }

    async fn export_post(
        app: Router,
        body: serde_json::Value,
    ) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
        let (status, headers, bytes) = post_raw(app, "/api/findings/export", body).await;
        (
            status,
            headers,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    /// Three full findings in `ws1`: `crit` (SEC-001), `high` (SEC-002) and
    /// `med` (SEC-003), declared by run_crit / run_high / run_med.
    fn app_with_three_full(tmp: &tempfile::TempDir) -> Router {
        seed_workspace_findings(
            tmp.path(),
            &[
                full_in_run("fnd_med", Severity::Medium, "run_med"),
                full_in_run("fnd_crit", Severity::Critical, "run_crit"),
                full_in_run("fnd_high", Severity::High, "run_high"),
            ],
        );
        app_for(tmp.path())
    }

    fn disposition(h: &axum::http::HeaderMap) -> &str {
        header_str(h, "content-disposition")
    }

    #[tokio::test]
    async fn export_md_is_a_named_attachment_starting_with_its_filename() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let (status, h, body) =
            get_raw(app_for(tmp.path()), "/api/findings/fnd_a/export?format=md").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            header_str(&h, "content-type"),
            "text/markdown; charset=utf-8"
        );
        assert_eq!(header_str(&h, "x-content-type-options"), "nosniff");
        let d = disposition(&h);
        assert!(d.starts_with("attachment; filename=\"SEC-001 - "), "{d}");
        assert!(d.ends_with(".md\""), "{d}");
        let body = String::from_utf8(body).unwrap();
        assert!(
            body.starts_with("Filename:"),
            "{}",
            &body[..40.min(body.len())]
        );
    }

    #[tokio::test]
    async fn export_html_is_an_attachment_never_inline() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let (status, h, body) = get_raw(
            app_for(tmp.path()),
            "/api/findings/fnd_a/export?format=html",
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&h, "content-type"), "text/html; charset=utf-8");
        let d = disposition(&h);
        assert!(d.starts_with("attachment;"), "{d}");
        assert!(!d.contains("inline"), "{d}");
        assert!(d.contains(".html"), "{d}");
        assert_eq!(header_str(&h, "x-content-type-options"), "nosniff");
        assert_eq!(header_str(&h, "content-security-policy"), "sandbox");
        assert!(String::from_utf8_lossy(&body).contains("<html"));
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn export_pdf_is_a_pdf_attachment() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let (status, h, body) =
            get_raw(app_for(tmp.path()), "/api/findings/fnd_a/export?format=pdf").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&h, "content-type"), "application/pdf");
        assert!(disposition(&h).starts_with("attachment; filename=\"SEC-001 - "));
        assert_eq!(header_str(&h, "x-content-type-options"), "nosniff");
        assert!(body.starts_with(b"%PDF"));
    }

    #[cfg(not(feature = "pdf"))]
    #[tokio::test]
    async fn export_pdf_without_pdf_support_is_501() {
        // Another crate in the build graph can switch the renderer's `pdf`
        // feature on even when this crate's is off (feature unification).
        if Format::Pdf.is_available() {
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let app = app_for(tmp.path());
        let (status, _, body) = get_raw(app.clone(), "/api/findings/fnd_a/export?format=pdf").await;
        assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED);
        assert!(error_of(&body).contains("PDF"), "{}", error_of(&body));
        // Fails before any lookup, even for an empty selection.
        let (status, _, _) =
            export_post(app, serde_json::json!({"format": "pdf", "ids": ["x"]})).await;
        assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn export_rejects_an_unknown_or_missing_format() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let app = app_for(tmp.path());
        for uri in [
            "/api/findings/fnd_a/export?format=exe",
            "/api/findings/fnd_a/export",
            "/api/findings/fnd_a/export?format=",
        ] {
            let (status, _, body) = get_raw(app.clone(), uri).await;
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{uri}");
            assert!(error_of(&body).contains("format"), "{uri}");
        }
        for body in [
            serde_json::json!({"format": "exe"}),
            serde_json::json!({"ids": ["fnd_a"]}),
        ] {
            let (status, _, _) = export_post(app.clone(), body).await;
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn export_of_an_unknown_finding_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let (status, _, body) = get_raw(
            app_for(tmp.path()),
            "/api/findings/fnd_nope/export?format=md",
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(error_of(&body), "finding fnd_nope not found");
    }

    #[tokio::test]
    async fn a_finding_is_numbered_within_its_own_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_named_workspace(
            tmp.path(),
            "ws_a",
            "repo_a",
            &[
                full_in_run("fnd_crit", Severity::Critical, "r"),
                full_in_run("fnd_high", Severity::High, "r"),
            ],
        );
        seed_named_workspace(
            tmp.path(),
            "ws_b",
            "repo_b",
            &[full_in_run("fnd_other", Severity::Low, "r")],
        );
        let app = app_for(tmp.path());
        for (id, number) in [
            ("fnd_crit", "SEC-001"),
            ("fnd_high", "SEC-002"),
            ("fnd_other", "SEC-001"),
        ] {
            let (status, h, body) =
                get_raw(app.clone(), &format!("/api/findings/{id}/export?format=md")).await;
            assert_eq!(status, axum::http::StatusCode::OK);
            let want = format!("attachment; filename=\"{number} - ");
            assert!(
                disposition(&h).starts_with(&want),
                "{id}: {}",
                disposition(&h)
            );
            let body = String::from_utf8(body).unwrap();
            assert!(body.starts_with(&format!("Filename: {number} - ")), "{id}");
        }
    }

    #[tokio::test]
    async fn the_configured_prefix_replaces_sec() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        state.config.write().unwrap().findings.export_id_prefix = Some("VULN".into());
        let app = routes().with_state(state);
        let (status, h, _) = get_raw(app.clone(), "/api/findings/fnd_a/export?format=md").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(
            disposition(&h).starts_with("attachment; filename=\"VULN-001 - "),
            "{}",
            disposition(&h)
        );
        let (_, _, body) = export_post(app, serde_json::json!({"format": "md"})).await;
        assert!(body.contains("VULN-001") && !body.contains("SEC-001"));
    }

    #[tokio::test]
    async fn an_invalid_configured_prefix_falls_back_to_sec() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        for bad in [
            "../x",
            "SEC 1",
            "",
            "1ABC",
            "A23456789012345678",
            "SEC\"",
            "É",
        ] {
            let state = AppState::new(
                tmp.path().to_path_buf(),
                rupu_config::PricingConfig::default(),
            );
            state.config.write().unwrap().findings.export_id_prefix = Some(bad.into());
            let (status, h, _) = get_raw(
                routes().with_state(state),
                "/api/findings/fnd_a/export?format=md",
            )
            .await;
            assert_eq!(status, axum::http::StatusCode::OK, "{bad:?}");
            assert!(
                disposition(&h).starts_with("attachment; filename=\"SEC-001 - "),
                "{bad:?}: {}",
                disposition(&h)
            );
        }
    }

    fn run_record(id: &str, workflow: &str) -> rupu_orchestrator::runs::RunRecord {
        use rupu_orchestrator::runs::{RunRecord, RunStatus};
        RunRecord {
            id: id.into(),
            workflow_name: workflow.into(),
            codename: None,
            status: RunStatus::Completed,
            inputs: Default::default(),
            event: None,
            workspace_id: "ws1".into(),
            workspace_path: "/tmp/x".into(),
            transcript_dir: "/tmp/x/.rupu/transcripts".into(),
            started_at: at("2026-09-29T00:00:00Z"),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            cause: None,
        }
    }

    #[tokio::test]
    async fn exports_carry_the_workflow_name_joined_from_the_run_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(
            tmp.path(),
            &[
                full_in_run("fnd_wf", Severity::Critical, "run_wf"),
                full_in_run("fnd_lost", Severity::High, "run_lost"),
            ],
        );
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        state
            .run_store
            .create(run_record("run_wf", "audit-flow"), "name: t\nsteps: []\n")
            .unwrap();
        let app = routes().with_state(state);
        let (_, _, body) = get_raw(app.clone(), "/api/findings/fnd_wf/export?format=md").await;
        assert!(String::from_utf8(body).unwrap().contains("audit-flow"));
        let (_, _, body) = export_post(app, serde_json::json!({"format": "md"})).await;
        assert!(body.contains("audit-flow"));
    }

    #[tokio::test]
    async fn project_report_is_an_attachment_named_by_its_title() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        let (status, h, body) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "title": "Q3 / \"audit\"\n review"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            header_str(&h, "content-type"),
            "text/markdown; charset=utf-8"
        );
        assert_eq!(header_str(&h, "x-content-type-options"), "nosniff");
        assert_eq!(
            disposition(&h),
            "attachment; filename=\"Q3 audit review.md\""
        );
        for n in ["SEC-001", "SEC-002", "SEC-003"] {
            assert!(body.contains(n), "{n}");
        }
        // No title: the default.
        let (_, h, _) = export_post(app.clone(), serde_json::json!({"format": "html"})).await;
        assert_eq!(
            disposition(&h),
            "attachment; filename=\"Findings report.html\""
        );
        let (_, h, _) =
            export_post(app, serde_json::json!({"format": "md", "title": "  ... "})).await;
        assert_eq!(
            disposition(&h),
            "attachment; filename=\"Findings report.md\""
        );
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn project_report_as_pdf() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        let (status, h, bytes) = post_raw(
            app,
            "/api/findings/export",
            serde_json::json!({"format": "pdf"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&h, "content-type"), "application/pdf");
        assert_eq!(
            disposition(&h),
            "attachment; filename=\"Findings report.pdf\""
        );
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[tokio::test]
    async fn split_export_of_two_ids_is_a_zip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        let (status, h, bytes) = post_raw(
            app,
            "/api/findings/export",
            serde_json::json!({
                "format": "md", "title": "Split set", "split": true,
                "ids": ["fnd_crit", "fnd_med"],
            }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(header_str(&h, "content-type"), "application/zip");
        assert_eq!(disposition(&h), "attachment; filename=\"Split set.zip\"");
        assert_eq!(header_str(&h, "x-content-type-options"), "nosniff");
        assert!(bytes.starts_with(b"PK"));
        // Zip entry names are stored uncompressed: the index, and the two
        // chosen findings under their stable numbers (the high one is absent).
        let hay = String::from_utf8_lossy(&bytes);
        assert!(hay.contains("index.md"));
        assert!(hay.contains("SEC-001 - "));
        assert!(hay.contains("SEC-003 - "));
        assert!(!hay.contains("SEC-002 - "));
    }

    #[tokio::test]
    async fn a_selection_that_matches_nothing_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        for body in [
            serde_json::json!({"format": "md", "ids": ["fnd_nope"]}),
            serde_json::json!({"format": "md", "owner": "Nobody At All"}),
            serde_json::json!({"format": "md", "cwe": "CWE-1"}),
            serde_json::json!({"format": "md", "ws_id": "ws_missing"}),
            serde_json::json!({"format": "md", "run_id": "run_missing"}),
            serde_json::json!({"format": "md", "split": true, "ids": ["fnd_nope"]}),
        ] {
            let (status, h, text) = export_post(app.clone(), body.clone()).await;
            assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{body}");
            assert_eq!(
                error_of(text.as_bytes()),
                "no findings match this selection",
                "{body}"
            );
            assert!(!disposition(&h).contains("attachment"), "{body}");
        }
    }

    #[tokio::test]
    async fn no_findings_at_all_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (status, _, text) =
            export_post(app_for(tmp.path()), serde_json::json!({"format": "md"})).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            error_of(text.as_bytes()),
            "no findings match this selection"
        );
    }

    #[tokio::test]
    async fn selection_filters_narrow_the_report_and_keep_numbers_stable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        let sel = |v: serde_json::Value| {
            let app = app.clone();
            async move {
                let (status, _, text) = export_post(app, v).await;
                assert_eq!(status, axum::http::StatusCode::OK, "{text}");
                text
            }
        };
        // Severity floor: critical + high, not medium.
        let text = sel(serde_json::json!({"format": "md", "min_severity": "high"})).await;
        assert!(text.contains("SEC-001") && text.contains("SEC-002"));
        assert!(!text.contains("SEC-003"));
        // One id keeps its project-wide number (medium is SEC-003, not 001).
        let text = sel(serde_json::json!({"format": "md", "ids": ["fnd_med"]})).await;
        assert!(text.contains("SEC-003") && !text.contains("SEC-001"));
        // One run.
        let text = sel(serde_json::json!({"format": "md", "run_id": "run_high"})).await;
        assert!(text.contains("SEC-002") && !text.contains("SEC-001") && !text.contains("SEC-003"));
        // One workspace / a CWE from the report / its exact owner.
        let text =
            sel(serde_json::json!({"format": "md", "ws_id": "ws1", "min_severity": "critical"}))
                .await;
        assert!(text.contains("SEC-001") && !text.contains("SEC-002"));
        let text =
            sel(serde_json::json!({"format": "md", "cwe": "cwe-639", "min_severity": "critical"}))
                .await;
        assert!(text.contains("SEC-001") && !text.contains("SEC-002"));
        let text = sel(
            serde_json::json!({"format": "md", "owner": "Unknown", "min_severity": "critical"}),
        )
        .await;
        assert!(text.contains("SEC-001") && !text.contains("SEC-002"));
    }

    #[tokio::test]
    async fn a_run_selection_includes_its_sub_runs_like_the_list_endpoint() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(
            tmp.path(),
            &[
                full_in_run("fnd_top", Severity::Critical, "run_parent"),
                full_in_run("fnd_unit", Severity::High, "run_unit1"),
                full_in_run("fnd_else", Severity::Medium, "run_other"),
            ],
        );
        let state = AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        );
        // A fan-out unit is its own sub-run; its id is only in the parent's
        // live event stream (the transcript stem).
        let ev_path = state.run_store.events_path("run_parent");
        std::fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        let ev = Event::UnitStarted {
            run_id: "run_parent".into(),
            step_id: "assess".into(),
            index: 0,
            unit_key: "u".into(),
            agent: None,
            codename: None,
            transcript_path: tmp.path().join("transcripts/run_unit1.jsonl"),
            host: None,
        };
        std::fs::write(&ev_path, serde_json::to_string(&ev).unwrap() + "\n").unwrap();
        let (status, _, text) = export_post(
            routes().with_state(state),
            serde_json::json!({"format": "md", "run_id": "run_parent"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(text.contains("SEC-001") && text.contains("SEC-002"));
        assert!(!text.contains("SEC-003"));
    }

    #[tokio::test]
    async fn summary_findings_are_opt_in_except_when_listed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let summary = finding("fnd_sum", Severity::High, "2026-01-01T00:00:00Z").record;
        seed_workspace_findings(
            tmp.path(),
            &[full_in_run("fnd_full", Severity::Critical, "r"), summary],
        );
        let app = app_for(tmp.path());
        let (_, _, text) = export_post(app.clone(), serde_json::json!({"format": "md"})).await;
        assert!(text.contains("SEC-001") && !text.contains("SEC-002"));
        let (_, _, text) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "include_summaries": true}),
        )
        .await;
        assert!(text.contains("SEC-001") && text.contains("SEC-002"));
        let (status, _, text) =
            export_post(app, serde_json::json!({"format": "md", "ids": ["fnd_sum"]})).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(text.contains("SEC-002") && !text.contains("SEC-001"));
    }

    #[tokio::test]
    async fn a_cross_reference_to_a_finding_left_out_prints_its_project_number() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut citing = full_in_run("fnd_crit", Severity::Critical, "run_crit");
        citing.report.as_mut().unwrap().cross_references =
            rupu_coverage::report::OrSentinel::Value(vec![rupu_coverage::report::CrossRef {
                finding_id: "fnd_med".to_string(),
                relation: rupu_coverage::report::Relation::Sibling,
                note: None,
            }]);
        seed_workspace_findings(
            tmp.path(),
            &[
                citing,
                full_in_run("fnd_high", Severity::High, "run_high"),
                full_in_run("fnd_med", Severity::Medium, "run_med"),
            ],
        );
        let app = app_for(tmp.path());
        // fnd_med (SEC-003) is left out by both selections.
        for body in [
            serde_json::json!({"format": "md", "min_severity": "high"}),
            serde_json::json!({"format": "md", "ids": ["fnd_crit"]}),
        ] {
            let (status, _, text) = export_post(app.clone(), body.clone()).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{body}");
            assert!(text.contains("- SEC-003 (sibling)"), "{body}\n{text}");
            assert!(!text.contains("fnd_med"), "{body}\n{text}");
        }
        // The single-finding export of the same finding prints the same
        // number. (The split archive is handed the same map; that path is
        // covered by rupu-findings-report's
        // `a_cross_reference_to_a_finding_left_out_keeps_its_project_number`.)
        let (_, _, single) = get_raw(app, "/api/findings/fnd_crit/export?format=md").await;
        assert!(String::from_utf8_lossy(&single).contains("- SEC-003 (sibling)"));
    }

    #[tokio::test]
    async fn a_cwe_selection_compares_numbers_not_substrings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut xss = full_in_run("fnd_xss", Severity::Critical, "r");
        xss.report.as_mut().unwrap().cwe = vec!["CWE-79".to_string()];
        let mut creds = full_in_run("fnd_creds", Severity::High, "r");
        creds.report.as_mut().unwrap().cwe = vec![];
        creds.concern_id = Some("cwe-top25-2023:cwe-798-hardcoded-credentials".to_string());
        seed_workspace_findings(tmp.path(), &[xss, creds]);
        let app = app_for(tmp.path());
        for cwe in ["CWE-79", "cwe-79", "79"] {
            let (status, _, text) =
                export_post(app.clone(), serde_json::json!({"format": "md", "cwe": cwe})).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{cwe}");
            assert!(
                text.contains("SEC-001") && !text.contains("SEC-002"),
                "{cwe}"
            );
            // The scope line names the CWE in its canonical form.
            assert!(text.contains("CWE CWE-79"), "{cwe}");
        }
        let (status, _, text) =
            export_post(app, serde_json::json!({"format": "md", "cwe": "CWE-798"})).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(text.contains("SEC-002") && !text.contains("SEC-001"));
    }

    #[tokio::test]
    async fn only_pdf_renders_wait_for_a_render_slot() {
        static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let a = render_slot(&GATE, Format::Pdf).await.unwrap();
        let b = render_slot(&GATE, Format::Pdf).await.unwrap();
        assert!(a.is_some() && b.is_some());
        // Both permits are out: a third PDF waits...
        let third = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            render_slot(&GATE, Format::Pdf),
        )
        .await;
        assert!(
            third.is_err(),
            "a third concurrent PDF render was let through"
        );
        // ...while Markdown and HTML never do.
        assert!(render_slot(&GATE, Format::Markdown)
            .await
            .unwrap()
            .is_none());
        assert!(render_slot(&GATE, Format::Html).await.unwrap().is_none());
        // A finished render frees its slot.
        drop(a);
        assert!(render_slot(&GATE, Format::Pdf).await.unwrap().is_some());
        drop(b);
        assert_eq!(MAX_CONCURRENT_PDF_RENDERS, 2);
    }

    #[tokio::test]
    async fn export_request_errors_are_client_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        // A severity outside the five words.
        let (status, _, text) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "min_severity": "urgent"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error_of(text.as_bytes()).contains("min_severity"));
        // A CWE that is not a CWE id.
        let (status, _, text) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "cwe": "xss"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error_of(text.as_bytes()).contains("cwe"));
        // A misspelt filter must not silently select everything.
        let (status, _, _) = export_post(
            app,
            serde_json::json!({"format": "md", "severity": "critical"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn attachment_disposition_is_ascii_safe_and_carries_utf8_separately() {
        assert_eq!(
            attachment_disposition("SEC-001 - a.md"),
            "attachment; filename=\"SEC-001 - a.md\""
        );
        // A header breaker can't leave the quotes, and non-ASCII moves to the
        // RFC 6266 `filename*` form.
        assert_eq!(
            attachment_disposition("é \"日本\\\n.md"),
            "attachment; filename=\"_ _____.md\"; \
             filename*=UTF-8''%C3%A9%20%22%E6%97%A5%E6%9C%AC%5C%0A.md"
        );
    }

    #[test]
    fn report_file_stem_is_a_safe_non_hidden_name() {
        assert_eq!(report_file_stem("Q3 review"), "Q3 review");
        assert_eq!(report_file_stem("../../etc/passwd"), "etcpasswd");
        for empty in ["", "   ", "...", "///", "\u{202e}"] {
            assert_eq!(report_file_stem(empty), "Findings report", "{empty:?}");
        }
        assert_eq!(report_file_stem(&"x".repeat(500)).chars().count(), 80);
    }

    #[tokio::test]
    async fn a_hostile_title_cannot_break_out_of_the_content_disposition() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_with_three_full(&tmp);
        let (status, h, _) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "title": "a\r\nX-Evil: 1\"; filename=\"x.exe"}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        // The quotes were dropped from the title, so the `;` stays inside the
        // one quoted string and starts no second parameter.
        assert_eq!(
            disposition(&h),
            "attachment; filename=\"a X-Evil 1; filename=x.exe.md\""
        );
        assert!(h.get("x-evil").is_none());
        // A title that is UTF-8 keeps it, in `filename*`.
        let (_, h, _) = export_post(
            app.clone(),
            serde_json::json!({"format": "md", "title": "Bericht Übersicht"}),
        )
        .await;
        assert!(
            disposition(&h).ends_with("filename*=UTF-8''Bericht%20%C3%9Cbersicht.md"),
            "{}",
            disposition(&h)
        );
        // Too long is refused rather than truncated.
        let (status, _, _) = export_post(
            app,
            serde_json::json!({"format": "md", "title": "t".repeat(201)}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    // ── the export pipeline as a library (used by `rupu findings export`) ────

    #[test]
    fn resolve_project_takes_a_workspace_id_or_a_checkout_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_named_workspace(tmp.path(), "ws_a", "repo_a", &[]);
        seed_named_workspace(tmp.path(), "ws_b", "repo_b", &[]);
        let global = tmp.path();
        assert_eq!(resolve_project(global, "ws_b").unwrap(), "ws_b");
        let repo_a = global.join("repo_a");
        assert_eq!(
            resolve_project(global, repo_a.to_str().unwrap()).unwrap(),
            "ws_a"
        );
        // A path that reaches the same directory another way still matches.
        let indirect = global.join("repo_b").join("..").join("repo_b");
        assert_eq!(
            resolve_project(global, indirect.to_str().unwrap()).unwrap(),
            "ws_b"
        );
        let err = resolve_project(global, "nope").unwrap_err();
        assert!(err.contains("`nope`"), "{err}");
    }

    #[test]
    fn export_titles_are_trimmed_defaulted_and_bounded() {
        assert_eq!(normalize_export_title(None).unwrap(), "Findings report");
        assert_eq!(
            normalize_export_title(Some("   ")).unwrap(),
            "Findings report"
        );
        assert_eq!(normalize_export_title(Some(" Q3 ")).unwrap(), "Q3");
        assert!(normalize_export_title(Some(&"t".repeat(200))).is_ok());
        assert!(normalize_export_title(Some(&"t".repeat(201))).is_err());
    }

    #[test]
    fn export_prefix_is_the_configured_one_when_valid() {
        assert_eq!(resolve_export_prefix(None), "SEC");
        assert_eq!(resolve_export_prefix(Some("ACME")), "ACME");
        for bad in ["", "1A", "A B", "../x"] {
            assert_eq!(resolve_export_prefix(Some(bad)), "SEC", "{bad:?}");
        }
    }

    #[test]
    fn severity_names_parse_in_any_case() {
        assert_eq!(parse_min_severity("High"), Some(Severity::High));
        assert_eq!(parse_min_severity("info"), Some(Severity::Info));
        assert_eq!(parse_min_severity("urgent"), None);
    }

    #[test]
    fn the_report_pipeline_reports_what_it_could_not_find() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_workspace_findings(tmp.path(), &[full_record("fnd_a")]);
        let runs = RunStore::new(tmp.path().join("runs"));
        let missing = export_finding_report(tmp.path(), &runs, "fnd_zzz", "SEC", Format::Markdown);
        assert!(matches!(missing, Err(ReportError::NotFound(m)) if m.contains("fnd_zzz")));
        let nothing = export_project_report(
            tmp.path(),
            &runs,
            "SEC",
            ReportRequest {
                title: "T".into(),
                min_severity: Some(Severity::Info),
                owner: Some("nobody".into()),
                ..ReportRequest::default()
            },
            Format::Markdown,
        );
        assert!(matches!(nothing, Err(ReportError::NotFound(_))));
        let ok =
            export_finding_report(tmp.path(), &runs, "fnd_a", "SEC", Format::Markdown).unwrap();
        assert!(ok.name.ends_with(".md"));
        assert!(String::from_utf8(ok.bytes).unwrap().contains("SEC-001"));
    }

    #[test]
    fn an_exported_image_block_is_embedded_from_the_local_store_only() {
        use base64::Engine as _;
        use rupu_coverage::report::EvidenceBlock;
        let tmp = tempfile::TempDir::new().unwrap();
        let mut png = PNG_MAGIC.to_vec();
        png.extend_from_slice(b"pretend pixels");
        let sha = store_blob(tmp.path(), &png);
        let image = |path: &str, stored, host: Option<&str>, caption: &str| EvidenceBlock::Image {
            artifact: artifact_ref(path, &sha, png.len() as u64, None, Some(stored), host),
            caption: Some(caption.into()),
        };
        let mut rec = full_record("fnd_img");
        rec.report.as_mut().unwrap().blocks = vec![
            image(
                "shots/local.png",
                ArtifactStorage::Copied,
                None,
                "Local screenshot",
            ),
            // The same bytes, recorded as living on another host: shown by
            // reference even though this store happens to hold the blob.
            image(
                "shots/remote.png",
                ArtifactStorage::External,
                Some("node-7"),
                "Remote screenshot",
            ),
        ];
        seed_workspace_findings(tmp.path(), &[rec]);
        let runs = RunStore::new(tmp.path().join("runs"));

        let html = export_finding_report(tmp.path(), &runs, "fnd_img", "SEC", Format::Html)
            .unwrap()
            .bytes;
        let html = String::from_utf8(html).unwrap();
        let data_uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        assert_eq!(html.matches(&data_uri).count(), 1, "{html}");
        assert!(
            html.contains("<figcaption>Local screenshot</figcaption>"),
            "{html}"
        );
        assert!(html.contains("not embedded: the file is on host"), "{html}");

        let md = export_finding_report(tmp.path(), &runs, "fnd_img", "SEC", Format::Markdown)
            .unwrap()
            .bytes;
        let md = String::from_utf8(md).unwrap();
        assert!(
            md.contains("![Local screenshot](<shots/local.png>)"),
            "{md}"
        );
        assert!(!md.contains("base64"), "{md}");
    }

    // ---- GET /api/findings?q= ----

    /// One registered workspace with three findings: `fnd_crit` (critical,
    /// tagged `needs-poc`), `fnd_high` and `fnd_low`. Returns the workspace path.
    fn seed_query_fixture(global: &std::path::Path) -> std::path::PathBuf {
        let records = [
            finding("fnd_crit", Severity::Critical, "2026-09-29T00:00:00Z").record,
            finding("fnd_high", Severity::High, "2026-09-28T00:00:00Z").record,
            finding("fnd_low", Severity::Low, "2026-09-27T00:00:00Z").record,
        ];
        let repo = seed_workspace_findings(global, &records);
        rupu_coverage::apply(
            &rupu_coverage::TagLog::for_workspace(&repo),
            &rupu_coverage::TagChange {
                finding_ids: vec!["fnd_crit".to_string()],
                add: vec![rupu_coverage::Tag::parse("needs-poc").unwrap()],
                remove: vec![],
            },
            &rupu_coverage::TagActor::operator(rupu_coverage::OperatorSurface::Cli),
        )
        .unwrap();
        repo
    }

    fn facet_count(json: &serde_json::Value, key: &str, value: &str) -> Option<u64> {
        json["facets"][key]
            .as_array()?
            .iter()
            .find(|v| v["value"] == value)
            .and_then(|v| v["count"].as_u64())
    }

    #[tokio::test]
    async fn list_findings_q_filters_but_facets_stay_unfiltered() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_query_fixture(tmp.path());
        let (status, json) =
            get_json(app_for(tmp.path()), "/api/findings?q=severity%3E%3Dhigh").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["findings"].as_array().unwrap().len(), 2);
        assert_eq!(json["summary"]["total"], 2);
        assert_eq!(facet_count(&json, "severity", "critical"), Some(1));
        assert_eq!(facet_count(&json, "severity", "high"), Some(1));
        assert_eq!(facet_count(&json, "severity", "low"), Some(1));
    }

    #[tokio::test]
    async fn list_findings_q_tag_selects_the_tagged_row_and_serves_its_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_query_fixture(tmp.path());
        let (status, json) = get_json(app_for(tmp.path()), "/api/findings?q=tag%3Aneeds-poc").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let rows = json["findings"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "fnd_crit");
        assert_eq!(rows[0]["tags"], serde_json::json!(["needs-poc"]));
    }

    #[tokio::test]
    async fn list_findings_bad_q_is_a_structured_400() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_query_fixture(tmp.path());
        let (status, json) = get_json(app_for(tmp.path()), "/api/findings?q=sevrity%3Ahigh").await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(json["code"], "unknown_key");
        assert_eq!(json["token"], 0);
        assert_eq!(json["start"], 0);
        assert!(json["end"].is_number());
        assert!(
            json["error"].as_str().unwrap().contains("unknown key"),
            "{json}"
        );
    }

    #[tokio::test]
    async fn list_findings_reports_a_workspace_whose_tag_log_is_unreadable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = seed_query_fixture(tmp.path());
        // A directory where the log should be: reading it is an I/O error,
        // unlike a missing file.
        let log = rupu_coverage::TagLog::for_workspace(&repo).path;
        std::fs::remove_file(&log).unwrap();
        std::fs::create_dir(&log).unwrap();
        let (status, json) = get_json(app_for(tmp.path()), "/api/findings").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            json["tags_unavailable"],
            serde_json::json!([{ "ws_id": "ws1", "project": "repo" }])
        );
        assert_eq!(json["findings"].as_array().unwrap().len(), 3);
    }

    /// `tags_unavailable` names only workspaces in the request's scope: an
    /// unreadable log in another workspace doesn't warn about this one.
    #[tokio::test]
    async fn list_findings_tags_unavailable_follows_the_ws_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = seed_query_fixture(tmp.path());
        seed_named_workspace(
            tmp.path(),
            "ws2",
            "repo2",
            &[finding("fnd_other", Severity::Medium, "2026-09-26T00:00:00Z").record],
        );
        let log = rupu_coverage::TagLog::for_workspace(&repo).path;
        std::fs::remove_file(&log).unwrap();
        std::fs::create_dir(&log).unwrap();
        let app = app_for(tmp.path());
        let (status, json) = get_json(app.clone(), "/api/findings?ws_id=ws2").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["tags_unavailable"], serde_json::json!([]));
        assert_eq!(json["findings"].as_array().unwrap().len(), 1);
        // In scope by ws_id, even when `q` selects none of its rows.
        let (_, json) = get_json(app.clone(), "/api/findings?ws_id=ws1&q=id%3Anone").await;
        assert_eq!(
            json["tags_unavailable"],
            serde_json::json!([{ "ws_id": "ws1", "project": "repo" }])
        );
        let (_, json) = get_json(app, "/api/findings").await;
        assert_eq!(
            json["tags_unavailable"],
            serde_json::json!([{ "ws_id": "ws1", "project": "repo" }])
        );
    }

    #[tokio::test]
    async fn list_findings_without_q_returns_everything_and_no_unavailable_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_query_fixture(tmp.path());
        let (status, json) = get_json(app_for(tmp.path()), "/api/findings").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(json["findings"].as_array().unwrap().len(), 3);
        assert_eq!(json["tags_unavailable"], serde_json::json!([]));
    }
}
