use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::Response,
    routing::get,
    Json, Router,
};
use rupu_coverage::report::{
    summarize, ArtifactKind, ArtifactStorage, ArtifactStore, ReportSummary,
};
use rupu_coverage::{discover_targets, read_findings, CoveragePaths, FindingRecord, Severity};
use rupu_orchestrator::{executor::Event, runs::RunStore};
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings", get(list_findings))
        .route("/api/findings/:id", get(get_finding))
        .route("/api/findings/:id/artifacts/:sha256", get(get_artifact))
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

    FindingsResponse { findings, summary }
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

/// Collect every finding across every registered workspace's coverage
/// targets, tagged with provenance (`ws_id` / `project` / `target_id`) but
/// WITHOUT the `RunStore` `workflow_name` join — `list_findings` does that
/// join itself afterward, since it needs `AppState.run_store`.
///
/// `pub(crate)`: this is the one workspace/target walk. `list_findings` and
/// `LocalHostConnector::dashboard_summary`'s open-findings count
/// (`host/local.rs`) both call it rather than each re-implementing the walk.
///
/// Tolerant by design: a workspace whose path is gone, or a target whose
/// `findings.jsonl` is absent/unreadable, is skipped with a `warn!` rather
/// than failing the caller.
pub(crate) fn collect_all_findings(global_dir: &std::path::Path) -> Vec<FindingOut> {
    let workspaces = store_for(global_dir).list().unwrap_or_default();

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
                // `w` (the owning workspace) is already in hand for every
                // finding in this loop — no separate lookup/memoization is
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
                out.push(FindingOut {
                    ws_id: w.id.clone(),
                    project: project.clone(),
                    target_id: t.target_id.clone(),
                    workflow_name: None,
                    permalink,
                    report_summary: None,
                    record,
                });
            }
        }
    }
    out
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
fn resolve_run_scope(store: &RunStore, parent: &str) -> HashSet<String> {
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
    let path = store.events_path(parent);
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };
        let tp = match event {
            Event::UnitStarted {
                transcript_path, ..
            } => Some(transcript_path),
            Event::StepWorking {
                transcript_path: Some(p),
                ..
            } => Some(p),
            _ => None,
        };
        if let Some(tp) = tp {
            let id = match tp.file_stem().and_then(|s| s.to_str()) {
                // Nested sub-run layout: `<parent>/sub/<sub_id>/transcript.jsonl`.
                Some("transcript") => tp
                    .parent()
                    .and_then(|d| d.file_name())
                    .and_then(|s| s.to_str())
                    .map(str::to_string),
                Some(stem) => Some(stem.to_string()),
                None => None,
            };
            if let Some(id) = id {
                out.push(id);
            }
        }
    }
    out
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
    let mut out: Vec<FindingOut> = collect_all_findings(&s.global_dir);

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

    let mut resp = scope_by_run_set(out, &run_ids, &q.ws_id, &q.workflow);
    // List rows never carry the report body; full-profile rows get a summary.
    resp.findings = resp
        .findings
        .into_iter()
        .map(FindingOut::into_list_row)
        .collect();
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
) -> ApiResult<Json<FindingDetail>> {
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
    Ok(Json(FindingDetail {
        finding,
        evidence_status,
    }))
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

/// `GET /api/findings/:id/artifacts/:sha256` — the bytes of an artifact the
/// finding's report references.
///
/// Only artifacts listed in the finding's own `report.artifacts` are served, so
/// a request can reach only a blob some finding in the ledger references. That
/// limits exposure, but it is not a hard boundary: the ledger lives in the
/// agent-writable workspace, so a forged ledger line can list any blob in the
/// store whose sha256 is already known. Artifacts are never rendered as HTML:
/// text is `text/plain` inline, anything else an `application/octet-stream`
/// attachment, always `nosniff` and `Content-Security-Policy: sandbox`.
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
        .and_then(|r| r.artifacts.iter().find(|a| a.sha256 == sha))
        .cloned()
        .ok_or_else(|| ApiError::not_found("this finding does not reference that artifact"))?;

    let file = match (artifact.stored, artifact.host.as_deref()) {
        (Some(ArtifactStorage::External), Some(host)) => {
            return Err(ApiError::not_found(format!(
                "artifact is stored on host {host}; remote fetch is not supported yet"
            )));
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

    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        file,
        STREAM_CHUNK_BYTES,
    ));
    let name = safe_filename(&artifact.path);
    let (ctype, disposition) = match artifact.kind {
        Some(ArtifactKind::Text) => (
            "text/plain; charset=utf-8",
            format!("inline; filename=\"{name}\""),
        ),
        _ => (
            "application/octet-stream",
            format!("attachment; filename=\"{name}\""),
        ),
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
    Ok(resp)
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

    // ── macOS golden fixtures (apps/rupu-macos/Fixtures/) ─────────────────────
    //
    // `FindingsResponse`/`FindingOut` are `pub`, but this fixture lives here
    // (rather than the integration test) per the Phase 2 plan, reusing the
    // `attribution()`/`at()` helpers above. Same `check_fixture` contract as
    // `tests/macos_fixtures.rs` (duplicated: a unit test can't share code
    // with an integration test without a public module) — see
    // `api/host_info.rs`'s test module for the established pattern.

    fn check_fixture(name: &str, value: &impl serde::Serialize) {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/rupu-macos/Fixtures");
        let path = dir.join(name);
        let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
        assert_eq!(
            on_disk.trim_end(),
            rendered,
            "fixture {name} drifted from the Rust types; run `make macos-fixtures`"
        );
    }

    #[test]
    fn findings_run_fixture_is_current() {
        let response = build_response(vec![
            FindingOut {
                ws_id: "ws1".into(),
                project: "rupu".into(),
                target_id: "tgt1".into(),
                workflow_name: Some("nightly-health".into()),
                permalink: Some("https://github.com/o/r/blob/main/src/a.rs#L17-L19".into()),
                report_summary: None,
                record: FindingRecord {
                    id: "fnd_1".into(),
                    file_path: Some("src/a.rs".into()),
                    line_range: Some([17, 19]),
                    target_ref: None,
                    scope: FindingScope::Line,
                    summary: "Potential panic on unwrap".into(),
                    severity: Severity::Critical,
                    concern_id: Some("no-unwrap".into()),
                    evidence: FindingEvidence {
                        code_excerpt: Some("let x = opt.unwrap();".into()),
                        rationale: "unwrap on an Option that can be None in production".into(),
                        references: vec!["https://doc.rust-lang.org/std/option".into()],
                    },
                    declared_by: attribution(),
                    declared_at: at("2026-08-20T12:00:00Z"),
                    profile: rupu_coverage::FindingProfile::Summary,
                    report: None,
                },
            },
            FindingOut {
                ws_id: "ws1".into(),
                project: "rupu".into(),
                target_id: "tgt1".into(),
                workflow_name: None,
                permalink: None,
                report_summary: None,
                record: FindingRecord {
                    id: "fnd_2".into(),
                    file_path: None,
                    line_range: None,
                    target_ref: None,
                    scope: FindingScope::Repo,
                    summary: "No CI workflow configured".into(),
                    severity: Severity::Info,
                    concern_id: None,
                    evidence: FindingEvidence {
                        code_excerpt: None,
                        rationale: "repository has no .github/workflows directory".into(),
                        references: vec![],
                    },
                    declared_by: attribution(),
                    declared_at: at("2026-08-20T11:00:00Z"),
                    profile: rupu_coverage::FindingProfile::Summary,
                    report: None,
                },
            },
        ]);
        check_fixture("findings_run.json", &response);
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
        };
        std::fs::write(&ev_path, serde_json::to_string(&ev).unwrap() + "\n").unwrap();
        let scope = resolve_run_scope(&store, parent);
        assert!(
            scope.contains("sub_child"),
            "nested layout → dir name (was: {scope:?})"
        );
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
        };
        let mut without_loc = with_loc.clone();
        without_loc.id = "fnd_no_loc".to_string();
        without_loc.file_path = None;
        without_loc.line_range = None;

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

    #[test]
    fn claim_states_use_the_dedicated_claim_hash_cap_not_the_artifact_cap() {
        // Claims cite source files: 64 MiB, far below the 500 MiB artifact cap.
        assert_eq!(CLAIM_HASH_MAX_BYTES, 64 * 1024 * 1024);
        assert!(CLAIM_HASH_MAX_BYTES < rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES);
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

    #[tokio::test]
    async fn artifact_external_on_a_remote_host_is_404_naming_the_host() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sha = "b".repeat(64);
        seed_artifact_finding(
            tmp.path(),
            vec![artifact_ref(
                "/var/log/big.bin",
                &sha,
                9_999_999_999,
                Some(ArtifactKind::Binary),
                Some(ArtifactStorage::External),
                Some("build-box-7"),
            )],
        );
        let (status, _, bytes) = get_raw(
            app_for(tmp.path()),
            &format!("/api/findings/fnd_art/artifacts/{sha}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        let msg = error_of(&bytes);
        assert!(msg.contains("build-box-7"), "message: {msg}");
        assert!(
            msg.contains("remote fetch is not supported yet"),
            "message: {msg}"
        );
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
}
