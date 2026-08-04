//! Netflow API — run-scoped read of the workspace ledger + run transcript,
//! merged, with read-time ASN enrichment.
//!
//! ASN is resolved HERE, at render time, not stamped into the record (spec
//! §6.2). A table that arrives later therefore improves every historical
//! flow with no backfill.
//!
//! ## Why this reads the ledger AND the transcript (do not "simplify")
//!
//! Only *provider* flows carry `ctx.run_id` — `Origin::Scm`, `Auth`/
//! `System`, `Update` and `Cp` flows are ALWAYS `run_id: None` by design
//! (see `rupu_netflow::ctx::FlowCtx::system`). Filtering the
//! workspace-level ledger by `run_id` alone therefore silently drops every
//! SCM/auth/update call the process made *while this run was active* — the
//! response would look complete but isn't. The run's own transcript carries
//! an `Event::NetFlow` line for every flow the process observed during the
//! run regardless of `ctx.run_id`, which is exactly the set that recovers
//! that gap. [`merge_with_transcript`] does the merge; see its doc for the
//! dedup rule.

use crate::{
    api::run_resolve::{resolve_run_location, RunLocation},
    api::runs::{resolve_host, run_not_found_or_internal},
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_netflow::{AsnInfo, AsnTable, FlowId, FlowRecord, NetflowPaths};
use rupu_orchestrator::runs::RunStore;
use rupu_transcript::{Event as TranscriptEvent, JsonlReader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path as StdPath, PathBuf};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/:id/netflow", get(get_run_netflow))
}

/// A flow plus its read-time enrichment.
///
/// `Deserialize` is derived (not just `Serialize`) so the `Host` branch of
/// `get_run_netflow` can parse a remote CP's JSON response straight off the
/// wire when proxying — the same struct is both the producer's and the
/// proxying consumer's type, so there is no separate wire DTO to drift out
/// of sync (mirrors `UsageSummary` in `usage.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowView {
    #[serde(flatten)]
    pub flow: FlowRecord,
    /// `None` when the peer IP is unknown (Coarse fidelity) or the ASN
    /// table has no entry. The UI distinguishes both from "AS0".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asn: Option<AsnInfo>,
}

impl FlowView {
    pub fn from_flow(flow: FlowRecord, table: Option<&AsnTable>) -> Self {
        let asn = flow.peer_ip.and_then(|ip| table.and_then(|t| t.lookup(ip)));
        Self { flow, asn }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetflowResponse {
    pub flows: Vec<FlowView>,
    /// Per-host rollup, computed here rather than in the browser so the
    /// percentile and unknown-bytes logic has exactly ONE implementation
    /// (`rupu_netflow::ledger::host_rollup`).
    pub hosts: Vec<rupu_netflow::ledger::HostRollup>,
    /// Records lost to writer-channel overflow. Surfaced so the UI can say
    /// "N flows dropped" instead of quietly under-reporting.
    pub dropped: u64,
    /// Whether the ASN table was available for this request. `false` means
    /// the `asn` fields are absent because we could not look them up, NOT
    /// because the flows had no ASN.
    pub asn_loaded: bool,
}

/// Flows belonging to one run, as recorded in the ledger. Callers almost
/// always want [`merge_with_transcript`] applied on top of this — see the
/// module doc for why the ledger alone under-reports.
pub fn filter_by_run(flows: &[FlowRecord], run_id: &str) -> Vec<FlowRecord> {
    flows
        .iter()
        .filter(|f| f.ctx.run_id.as_deref() == Some(run_id))
        .cloned()
        .collect()
}

/// Load the ASN table if present. A missing table is not an error —
/// enrichment simply degrades.
pub(crate) fn load_asn_table() -> Option<AsnTable> {
    rupu_netflow::asn::asn_db_path().and_then(|p| AsnTable::load(&p).ok())
}

/// Merge the ledger's run-scoped flows with the `Event::NetFlow` lines
/// found in the run's own transcript files, deduped by [`FlowId`].
///
/// On an id collision the LEDGER's copy wins: `ledger::read_flows` folds
/// any `LedgerLine::Complete` into it, so its `bytes_in`/`duration_ms` are
/// finalized, whereas the transcript's copy is only the snapshot taken at
/// the moment the flow was first recorded (a streaming flow is written to
/// the transcript before it completes). Transcript-only flows — everything
/// with `ctx.run_id: None` that the ledger's run-id filter dropped — are
/// added as-is; there is no more-authoritative copy of them anywhere.
///
/// A transcript file that fails to open/parse is skipped, not fatal — the
/// same "missing data degrades, never errors" contract as the ledger read.
fn merge_with_transcript(
    ledger_scoped: Vec<FlowRecord>,
    transcript_paths: &[PathBuf],
) -> Vec<FlowRecord> {
    let mut by_id: HashMap<FlowId, FlowRecord> = HashMap::new();
    for f in ledger_scoped {
        by_id.insert(f.id, f);
    }

    for path in transcript_paths {
        let Ok(events) = JsonlReader::iter(path) else {
            continue;
        };
        for event in events.flatten() {
            if let TranscriptEvent::NetFlow { flow } = event {
                by_id.entry(flow.id).or_insert(*flow);
            }
        }
    }

    by_id.into_values().collect()
}

pub(crate) fn build_response(flows: Vec<FlowRecord>, dropped: u64) -> NetflowResponse {
    let table = load_asn_table();
    let hosts = rupu_netflow::ledger::host_rollup(&flows);
    NetflowResponse {
        flows: flows
            .into_iter()
            .map(|f| FlowView::from_flow(f, table.as_ref()))
            .collect(),
        hosts,
        dropped,
        asn_loaded: table.is_some(),
    }
}

/// Read the ledger + run transcript for a run whose artifacts live under
/// `workspace` (a project root, `<workspace>/.rupu/netflow/flows.jsonl` and
/// whatever `store` reports as this run's step transcript paths), scope to
/// `run_id`, merge, and build the response. A missing ledger is an empty
/// result, not an error.
fn collect_run_netflow(store: &RunStore, run_id: &str, workspace: &StdPath) -> NetflowResponse {
    let paths = NetflowPaths::new(workspace);
    let all = rupu_netflow::ledger::read_flows(&paths.flows).unwrap_or_default();
    let dropped = rupu_netflow::ledger::read_dropped_total(&paths.flows).unwrap_or(0);

    let scoped = filter_by_run(&all, run_id);
    let transcript_paths = crate::usage::run_transcript_paths(store, run_id);
    let merged = merge_with_transcript(scoped, &transcript_paths);

    build_response(merged, dropped)
}

/// Proxy `GET /api/runs/:id/netflow` to a resolved host. Mirrors
/// `graph.rs`'s `run_graph_from_host` — the remote CP does the same
/// ledger+transcript merge locally and we just relay its response.
async fn run_netflow_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> ApiResult<NetflowResponse> {
    let conn = resolve_host(s, host_id)?;
    let value = conn
        .proxy_get_json(&format!("/api/runs/{id}/netflow"))
        .await
        .map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
    serde_json::from_value(value).map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/runs/:id/netflow` — network flows attributed to one run, from
/// the workspace ledger merged with the run transcript (see module doc),
/// with read-time ASN enrichment and a server-computed per-host rollup.
///
/// Dispatches on [`resolve_run_location`] exactly like `run_graph`:
/// `Global`/`ProjectLocal` read locally, `Host` proxies, `Unpersisted` has
/// no workspace to read from so it returns an empty (not error) response,
/// `NotFound` → 404.
async fn get_run_netflow(
    State(s): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<NetflowResponse>> {
    match resolve_run_location(&s, &run_id).await {
        RunLocation::Global => {
            let run = s
                .run_store
                .load(&run_id)
                .map_err(|e| run_not_found_or_internal(&run_id, e))?;
            Ok(Json(collect_run_netflow(
                &s.run_store,
                &run_id,
                &run.workspace_path,
            )))
        }
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            Ok(Json(collect_run_netflow(&store, &run_id, &path)))
        }
        RunLocation::Host { host_id } => {
            run_netflow_from_host(&s, &host_id, &run_id).await.map(Json)
        }
        // No artifacts anywhere: the run never persisted a workspace to
        // read a ledger or transcript from. Empty, not an error — mirrors
        // `run_graph`'s `Unpersisted` branch.
        RunLocation::Unpersisted { .. } => Ok(Json(build_response(Vec::new(), 0))),
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {run_id} not found"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_netflow::{Fidelity, FlowCtx, Origin, Outcome};

    fn flow(id: FlowId, run: Option<&str>, host: &str) -> FlowRecord {
        FlowRecord {
            id,
            ts: chrono::Utc::now(),
            ctx: FlowCtx {
                run_id: run.map(str::to_string),
                step_id: None,
                agent: None,
                workspace_id: None,
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: host.into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
            resolved_ips: vec![],
            http_version: None,
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: Some(10),
            bytes_in: Some(20),
            body_complete: true,
            ttfb_ms: None,
            duration_ms: Some(30),
        }
    }

    fn write_transcript(path: &std::path::Path, flows: &[FlowRecord]) {
        let mut w = rupu_transcript::JsonlWriter::create(path).unwrap();
        for f in flows {
            w.write(&TranscriptEvent::NetFlow {
                flow: Box::new(f.clone()),
            })
            .unwrap();
        }
        w.flush().unwrap();
    }

    #[test]
    fn build_response_wires_a_server_computed_host_rollup() {
        let f = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let resp = build_response(vec![f], 0);
        assert_eq!(resp.hosts.len(), 1);
        assert_eq!(resp.hosts[0].host, "api.anthropic.com");
        assert_eq!(resp.hosts[0].calls, 1);
    }

    #[test]
    fn merge_adds_transcript_only_flows_the_ledger_run_filter_dropped() {
        // Simulates the SCM/auth/update gap: an Scm flow is `run_id: None`
        // so it never survives `filter_by_run` against the ledger, but it
        // DID land in the run's transcript (the process saw it while the
        // run was active).
        let tmp = tempfile::TempDir::new().unwrap();
        let scm_flow = {
            let mut f = flow(FlowId::new(), None, "api.github.com");
            f.ctx.origin = Origin::Scm("github".into());
            f
        };
        let transcript = tmp.path().join("step.jsonl");
        write_transcript(&transcript, std::slice::from_ref(&scm_flow));

        let merged = merge_with_transcript(Vec::new(), &[transcript]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, scm_flow.id);
        assert_eq!(merged[0].host, "api.github.com");
    }

    #[test]
    fn merge_prefers_the_ledgers_finalized_copy_on_id_collision() {
        // Same FlowId in both: the ledger's copy already folded in a
        // `Complete` line (finalized bytes_in/duration_ms); the
        // transcript's copy is the pre-completion snapshot. The ledger
        // must win, not "whichever was inserted first" by accident.
        let tmp = tempfile::TempDir::new().unwrap();
        let id = FlowId::new();

        let mut stale = flow(id, Some("r"), "api.anthropic.com");
        stale.bytes_in = None;
        stale.body_complete = false;

        let mut finalized = flow(id, Some("r"), "api.anthropic.com");
        finalized.bytes_in = Some(9999);
        finalized.body_complete = true;

        let transcript = tmp.path().join("step.jsonl");
        write_transcript(&transcript, &[stale]);

        let merged = merge_with_transcript(vec![finalized], &[transcript]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].bytes_in, Some(9999));
        assert!(merged[0].body_complete);
    }

    #[test]
    fn merge_skips_an_unreadable_transcript_path_without_failing() {
        let ledger_flow = flow(FlowId::new(), Some("r"), "api.anthropic.com");
        let merged = merge_with_transcript(
            vec![ledger_flow.clone()],
            &[PathBuf::from("/nonexistent/dir/gone.jsonl")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, ledger_flow.id);
    }
}
