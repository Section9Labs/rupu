//! Autoflow runs as one table (spec
//! `docs/superpowers/specs/2026-10-14-rupu-cp-autoflow-runs-one-table-design.md`).
//!
//! The run list is the source of truth for autoflow runs (a run row whose
//! `autoflow` is non-null); the autoflow history contributes only the
//! attempts that never produced a recorded run ([`attempt_rows`]) and the
//! failures that name no workflow at all ([`problems`]).

use crate::api::autoflow_claims::ClaimIndex;
use crate::api::runs::RunAutoflow;
use chrono::{DateTime, Utc};
use rupu_orchestrator::RunStatus;
use rupu_runtime::{AutoflowCycleEventKind, AutoflowHistoryEventRecord};
use std::collections::HashMap;

/// What an attempt row says about the attempt itself (web `AutoflowAttempt`).
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct AttemptInfo {
    /// The failure text, when the event carried one.
    pub detail: Option<String>,
    pub cycle_id: String,
    pub worker: Option<String>,
    /// The claim's status as the event recorded it.
    pub claim_status: Option<String>,
}

/// A failed autoflow attempt in `RunListRow`'s shape, so the web renders it
/// with the Workflows columns: zero usage, `autoflow` set, and `attempt`.
#[derive(serde::Serialize, Clone, Debug)]
pub struct AttemptRow {
    /// The event's run id when it had one, else the history event id.
    pub id: String,
    pub workflow_name: String,
    pub status: RunStatus,
    /// The event time.
    pub started_at: DateTime<Utc>,
    /// Same as `started_at`: an attempt has no duration.
    pub finished_at: Option<DateTime<Utc>>,
    /// `event` when the event names the wake event it answered, else `cron`.
    pub trigger: &'static str,
    pub usage: crate::usage::UsageSummary,
    pub turns: u64,
    pub duration_ms: Option<u64>,
    pub codename: String,
    pub codename_derived: bool,
    /// Always `Some`: an attempt is an autoflow's.
    pub autoflow: Option<RunAutoflow>,
    pub attempt: AttemptInfo,
    /// `Some(false)` when `id` is a run id with no run record (the run page
    /// renders such a run from history); absent when `id` is an event id.
    /// Never `true`: an attempt whose run was recorded is the run row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_recorded: Option<bool>,
}

/// The fields of one history event an attempt row is built from — a local
/// [`AutoflowHistoryEventRecord`] or a remote host's event row JSON.
struct AttemptSource {
    event_id: String,
    cycle_id: String,
    at: DateTime<Utc>,
    /// `Some(id)` only when the row may use the run id as its own: the
    /// coordinator could check that no run record exists for it.
    unrecorded_run_id: Option<String>,
    workflow: String,
    status: RunStatus,
    autoflow: RunAutoflow,
    worker: Option<String>,
    claim_status: Option<String>,
    detail: Option<String>,
    event_trigger: bool,
}

impl AttemptSource {
    fn into_row(self) -> AttemptRow {
        let run_recorded = self.unrecorded_run_id.as_ref().map(|_| false);
        let id = self.unrecorded_run_id.unwrap_or(self.event_id);
        let (codename, codename_derived) = crate::codename::named(None, &id, None);
        AttemptRow {
            id,
            workflow_name: self.workflow,
            status: self.status,
            started_at: self.at,
            finished_at: Some(self.at),
            trigger: if self.event_trigger { "event" } else { "cron" },
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
            codename,
            codename_derived,
            autoflow: Some(self.autoflow),
            attempt: AttemptInfo {
                detail: self.detail,
                cycle_id: self.cycle_id,
                worker: self.worker,
                claim_status: self.claim_status,
            },
            run_recorded,
        }
    }
}

/// `at` as a UTC instant, or `None` when it isn't RFC 3339.
fn parse_at(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Newest first by `(started_at, id)`.
fn newest_first(rows: &mut [AttemptRow]) {
    rows.sort_by(|a, b| (b.started_at, &b.id).cmp(&(a.started_at, &a.id)));
}

/// The failed attempts among the local history `events`, newest first:
///
/// - a `cycle_failed` event naming a `workflow` → status `failed`;
/// - with `check_launched`, a `run_launched` event whose `run_id` has no
///   run record (`!run_exists(id)`) → status from the claim status the way
///   the run page maps it ([`crate::api::run_resolve::map_claim_status_str`]).
///
/// An event whose `run_id` names a recorded run is never an attempt (the
/// run row is listed). An event whose `at` isn't a timestamp can't be placed
/// and is left out. Claim fields come from `claims`.
pub fn attempt_rows(
    events: &[AutoflowHistoryEventRecord],
    run_exists: &dyn Fn(&str) -> bool,
    check_launched: bool,
    claims: &ClaimIndex,
) -> Vec<AttemptRow> {
    let mut rows: Vec<AttemptRow> = events
        .iter()
        .filter_map(|rec| {
            let e = &rec.event;
            if e.run_id.as_deref().is_some_and(run_exists) {
                return None;
            }
            let status = match e.kind {
                AutoflowCycleEventKind::CycleFailed if e.workflow.is_some() => RunStatus::Failed,
                AutoflowCycleEventKind::RunLaunched if check_launched && e.run_id.is_some() => {
                    crate::api::run_resolve::map_claim_status_str(e.status.as_deref().unwrap_or(""))
                }
                _ => return None,
            };
            let at = parse_at(&rec.at)?;
            let id = e.run_id.clone().unwrap_or_else(|| rec.event_id.clone());
            let mut autoflow = RunAutoflow {
                issue_ref: e.issue_ref.clone(),
                ..Default::default()
            };
            claims.fill(&id, &mut autoflow);
            if autoflow.issue_display_ref.is_none() {
                autoflow.issue_display_ref = e.issue_display_ref.clone();
            }
            Some(
                AttemptSource {
                    event_id: rec.event_id.clone(),
                    cycle_id: rec.cycle_id.clone(),
                    at,
                    unrecorded_run_id: e.run_id.clone(),
                    workflow: e.workflow.clone().unwrap_or_default(),
                    status,
                    autoflow,
                    worker: rec.worker_name.clone().or_else(|| rec.worker_id.clone()),
                    claim_status: e.status.clone(),
                    detail: e.detail.clone(),
                    event_trigger: e.wake_event_id.is_some(),
                }
                .into_row(),
            )
        })
        .collect();
    newest_first(&mut rows);
    rows
}

/// The failed attempts among a remote host's `/api/runs/autoflows/events`
/// rows: only `cycle_failed` events naming a workflow — the coordinator
/// can't check whether a remote `run_launched` run was recorded — each under
/// its event id (a remote run id can't be vouched for either).
pub fn remote_attempt_rows(rows: &[serde_json::Value]) -> Vec<AttemptRow> {
    let str_of = |row: &serde_json::Value, key: &str| {
        row.get(key).and_then(|v| v.as_str()).map(str::to_owned)
    };
    let mut out: Vec<AttemptRow> = rows
        .iter()
        .filter_map(|row| {
            if str_of(row, "kind").as_deref() != Some("cycle_failed") {
                return None;
            }
            let workflow = str_of(row, "workflow")?;
            let at = parse_at(&str_of(row, "at")?)?;
            Some(
                AttemptSource {
                    event_id: str_of(row, "event_id")?,
                    cycle_id: str_of(row, "cycle_id").unwrap_or_default(),
                    at,
                    unrecorded_run_id: None,
                    workflow,
                    status: RunStatus::Failed,
                    autoflow: RunAutoflow {
                        issue_display_ref: str_of(row, "issue_display_ref"),
                        ..Default::default()
                    },
                    worker: str_of(row, "worker_name"),
                    claim_status: str_of(row, "status"),
                    detail: str_of(row, "detail"),
                    event_trigger: false,
                }
                .into_row(),
            )
        })
        .collect();
    newest_first(&mut out);
    out
}

/// One line of the problems strip: `cycle_failed` events naming no
/// workflow, grouped by host and subject.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct Problem {
    pub host_id: String,
    /// `"repo"` (the event named a repo) or `"worker"`.
    pub subject_kind: &'static str,
    pub subject: String,
    /// The newest event's detail.
    pub detail: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub count: usize,
}

/// The fields [`problems`] needs, from a local record or a remote event row.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemEvent {
    pub at: String,
    pub repo_ref: Option<String>,
    pub worker: Option<String>,
    pub detail: Option<String>,
}

/// The subject a failure that names no workflow is about.
const UNNAMED_WORKER: &str = "autoflow worker";

/// `events` (failures that name no workflow) at or after `cutoff`, grouped
/// by `repo_ref`, else the worker (`UNNAMED_WORKER` when unknown), newest
/// `last_seen` first. An event whose `at` isn't a timestamp is left out.
pub fn problems(events: &[ProblemEvent], host_id: &str, cutoff: DateTime<Utc>) -> Vec<Problem> {
    struct Group {
        problem: Problem,
        first: DateTime<Utc>,
        last: DateTime<Utc>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<(&'static str, String), usize> = HashMap::new();
    for e in events {
        let Some(at) = parse_at(&e.at) else { continue };
        if at < cutoff {
            continue;
        }
        let key = match (&e.repo_ref, &e.worker) {
            (Some(repo), _) => ("repo", repo.clone()),
            (None, Some(w)) => ("worker", w.clone()),
            (None, None) => ("worker", UNNAMED_WORKER.to_string()),
        };
        let i = *index.entry(key.clone()).or_insert_with(|| {
            groups.push(Group {
                problem: Problem {
                    host_id: host_id.to_string(),
                    subject_kind: key.0,
                    subject: key.1,
                    detail: e.detail.clone(),
                    first_seen: e.at.clone(),
                    last_seen: e.at.clone(),
                    count: 0,
                },
                first: at,
                last: at,
            });
            groups.len() - 1
        });
        let g = &mut groups[i];
        g.problem.count += 1;
        if at < g.first {
            g.first = at;
            g.problem.first_seen = e.at.clone();
        }
        if at >= g.last {
            g.last = at;
            g.problem.last_seen = e.at.clone();
            g.problem.detail = e.detail.clone();
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.last));
    groups.into_iter().map(|g| g.problem).collect()
}

use crate::api::runs::{
    autoflow_of, blocking, host_list_error, in_lifecycle, read_filtered_runs, resolve_host,
    run_probe, FilteredRuns, RunListRow, FAN_OUT_LIMIT,
};
use crate::customers::{Attribution, CustomerFilter, PriceBook};
use crate::error::{ApiError, ApiResult};
use crate::host::connector::{HostConnector, HostConnectorError, RunKind};
use crate::query::{self, ListError, ListQuery, ListResponse, RowFilter, Unfilterable};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use futures_util::future::join_all;
use rupu_orchestrator::{runs::RunStore, RunRecord};
use rupu_runtime::AutoflowHistoryStore;
use std::path::Path;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/autoflows/runs", get(list_autoflow_run_rows))
        .route("/api/runs/autoflows/problems", get(list_autoflow_problems))
}

/// `GET /api/runs/autoflows/runs`'s parameters: `/api/runs/workflows`'s
/// (`q` on [`crate::query::autoflow_runs::FIELDS`]).
#[derive(serde::Deserialize)]
pub(crate) struct AutoflowRunsQuery {
    // Flat, not a flattened `PageQuery` — see `WorkflowRunsQuery`.
    offset: Option<usize>,
    limit: Option<usize>,
    /// `active` | `completed` | `failed`.
    lifecycle: Option<String>,
    /// One host; absent → every registered host.
    #[serde(default)]
    host: Option<String>,
    /// RFC 3339 bounds on `started_at`; `host=local` only, as on
    /// `/api/runs/workflows` (`q=since:… until:…` filters every host).
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    /// `<slug>` | `none` — see `list_runs`.
    #[serde(default)]
    customer: Option<String>,
    /// `?q=` — the list query ([`crate::query::autoflow_runs::FIELDS`]).
    #[serde(default)]
    q: Option<String>,
    /// `?facets=1` — answer `{rows, facets, facets_partial}`.
    #[serde(default)]
    facets: Option<String>,
}

impl AutoflowRunsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }

    /// What the request lists, derived once.
    fn listing(&self) -> Result<Listing, ListError> {
        let filter = RowFilter::new(
            CustomerFilter::parse(self.customer.as_deref())?,
            ListQuery::parse(self.q.as_deref(), query::autoflow_runs::FIELDS)?,
        )
        .only(query::autoflow_runs::is_autoflow_row);
        let archived = filter
            .query
            .as_ref()
            .is_some_and(|q| q.asks_for("is", "archived"));
        Ok(Listing {
            lifecycle: self.lifecycle.clone(),
            range: crate::pagination::DateRangeQuery {
                since: self.since.clone(),
                until: self.until.clone(),
            },
            filter,
            facets: query::wants_facets(self.facets.as_deref()),
            archived,
        })
    }
}

/// What one request lists: the rows every host's contribution is kept by.
#[derive(Clone)]
struct Listing {
    pub lifecycle: Option<String>,
    pub range: crate::pagination::DateRangeQuery,
    /// The customer scope, the query and the autoflow predicate.
    pub filter: RowFilter,
    pub facets: bool,
    /// `is:archived`: the local archive, no attempts, no remote host.
    pub archived: bool,
}

impl Listing {
    /// Whether attempt rows are listed at all: never from the archive (an
    /// attempt can't be archived), never when the filter reads a customer
    /// (an attempt records none) and only under All or Failed·Rejected (an
    /// attempt failed). What the query keeps of them is the query's call.
    fn attempts(&self) -> bool {
        !self.archived
            && !self.filter.reads_customer()
            && matches!(self.lifecycle.as_deref(), None | Some("failed"))
    }

    fn facet_counts(&self, scoped: &[serde_json::Value]) -> Option<query::FacetCounts> {
        self.facets
            .then(|| query::facet_counts(scoped, query::autoflow_runs::FIELDS, false))
    }

    /// Whether the filter keeps `row`, collecting it into `scoped` (what
    /// facets count) when it is in scope; `Err` names the field the row
    /// can't report.
    fn admit(
        &self,
        row: &serde_json::Value,
        scoped: &mut Vec<serde_json::Value>,
    ) -> Result<bool, rupu_query::Unreportable> {
        if !self.filter.in_scope(row)? {
            return Ok(false);
        }
        if self.facets {
            scoped.push(row.clone());
        }
        self.filter.matches_query(row)
    }
}

/// The history events of `since..until` under `global`; an unreadable or
/// corrupt history is logged and contributes nothing (the runs still list).
fn history_events(
    global: &Path,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Vec<AutoflowHistoryEventRecord> {
    let store = AutoflowHistoryStore::new(global.join("autoflows").join("history"));
    match store.list_events_between(since, until) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(
                root = %store.root.display(),
                error = %e,
                "autoflow history unreadable; listing no attempts or problems from it"
            );
            Vec::new()
        }
    }
}

/// A local run the list kept, attributed and unpriced: pricing folds its
/// usage ledger, so only the runs on the returned page are priced.
struct Unpriced {
    record: RunRecord,
    who: Option<Attribution>,
}

/// One host's contribution: its rows (run rows and attempt rows, tagged,
/// newest first; the local host's run rows are unpriced probes), the
/// in-scope rows facets count, and the hosts named as unable to report a
/// field the filter reads.
#[derive(Default)]
struct HostRows {
    rows: Vec<serde_json::Value>,
    scoped: Vec<serde_json::Value>,
    unfilterable: Vec<Unfilterable>,
}

/// The local host's rows plus what prices the run rows that land on the
/// page ([`LocalRows::price`]).
struct LocalRows {
    rows: HostRows,
    store: Arc<RunStore>,
    claims: ClaimIndex,
    unpriced: std::collections::HashMap<String, Unpriced>,
    archived: bool,
}

impl LocalRows {
    /// Replace each local run probe among `rows` (one page) with its priced
    /// row; attempt rows and other hosts' rows stay. Blocking IO.
    fn price(mut self, rows: &mut [serde_json::Value], prices: &mut dyn PriceBook) {
        for v in rows.iter_mut() {
            if v["host_id"] != "local" {
                continue;
            }
            let Some(run) = v["id"].as_str().and_then(|id| self.unpriced.remove(id)) else {
                continue;
            };
            let mut row = RunListRow::priced(&run.record, run.who, &self.store, prices);
            self.claims.apply(&mut row);
            *v = tag_local(row, self.archived);
        }
    }
}

/// A local row on the wire: `host_id: "local"` (and `archived: true` from
/// the archive).
fn tag_local(row: RunListRow, archived: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(row).expect("a run row serializes");
    v["host_id"] = serde_json::json!("local");
    if archived {
        v["archived"] = serde_json::json!(true);
    }
    v
}

/// The local host's autoflow runs and attempts as the filter keeps them,
/// newest first and UNPRICED: every run record of `store` is read, kept by
/// lifecycle and range, attributed (fail-closed when the filter reads a
/// customer), read by the registry as its probe — the row without usage,
/// with its autoflow and claim fields — and kept by the filter; the attempt
/// rows (when [`Listing::attempts`]) are kept by the same filter. Under a
/// filter, a mirrored worker run whose customer or autoflow can't be known
/// here is left out and its worker host named. Blocking IO.
fn select_local(store: Arc<RunStore>, global: &Path, l: &Listing) -> ApiResult<LocalRows> {
    let mut lookup =
        crate::customers::CustomerLookup::new(rupu_workspace::CustomerStore::new(global));
    let claims = ClaimIndex::load(global);
    let mut rows = HostRows::default();
    let mut unpriced = std::collections::HashMap::new();
    let fail_closed = l.filter.reads_customer();
    for r in store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
    {
        if !in_lifecycle(r.status, l.lifecycle.as_deref()) || !l.range.contains(r.started_at) {
            continue;
        }
        let who = lookup.attribute_run(&r, fail_closed)?;
        let mut probe = run_probe(&r, &who, "local", l.archived);
        if let Some(af) = autoflow_of(&r, &store) {
            probe["autoflow"] = serde_json::to_value(af).expect("an autoflow serializes");
        }
        claims.apply_json(&mut probe);
        match l.admit(&probe, &mut rows.scoped) {
            Ok(true) => {
                unpriced.insert(r.id.clone(), Unpriced { record: r, who });
                rows.rows.push(probe);
            }
            Ok(false) => {}
            Err(e) => {
                if let Some(w) = r.worker_id.as_deref() {
                    query::note_unfilterable(&mut rows.unfilterable, w, e.field);
                }
            }
        }
    }
    if l.attempts() {
        let events = history_events(global, l.range.since(), l.range.until());
        for a in attempt_rows(&events, &|id| store.exists(id), true, &claims) {
            let mut v = serde_json::to_value(a).expect("an attempt row serializes");
            v["host_id"] = serde_json::json!("local");
            if l.admit(&v, &mut rows.scoped) == Ok(true) {
                rows.rows.push(v);
            }
        }
    }
    sort_newest_first(&mut rows.rows);
    Ok(LocalRows {
        rows,
        store,
        claims,
        unpriced,
        archived: l.archived,
    })
}

/// [`select_local`] on the blocking pool, over the active store or — from
/// the archive — a store at its root (an archived run's ledger lives there).
async fn local_rows(s: &AppState, l: &Listing) -> ApiResult<LocalRows> {
    let store = if l.archived {
        Arc::new(RunStore::new(s.run_store.archive_root()))
    } else {
        Arc::clone(&s.run_store)
    };
    let global = s.global_dir.clone();
    let listing = l.clone();
    blocking(move || select_local(store, &global, &listing)).await
}

/// Price the local run rows of `page` ([`LocalRows::price`]) on the
/// blocking pool, per customer.
async fn price_page(
    s: &AppState,
    local: LocalRows,
    mut page: Vec<serde_json::Value>,
) -> ApiResult<Vec<serde_json::Value>> {
    let customer_pricing = Arc::clone(&s.customer_pricing);
    blocking(move || {
        let mut prices = crate::customers::PricingMemo::new(&customer_pricing);
        local.price(&mut page, &mut prices);
        Ok(page)
    })
    .await
}

/// One remote host's autoflow runs and attempts, newest first, enough for a
/// window of `want` rows (the caller cuts the page). Runs: the host's run
/// list (`RunKind::All`), page by page through [`read_filtered_runs`] until
/// `want` rows match the filter — the autoflow predicate, the customer
/// scope and the query; a host any of whose rows can't report a field the
/// filter reads (no `autoflow` key: an older rupu) is named in
/// `unfilterable` and contributes nothing. Attempts: the host's event rows'
/// `cycle_failed` events naming a workflow, kept by the same filter; a host
/// without that listing contributes runs only.
async fn remote_rows(
    conn: &dyn HostConnector,
    host_id: &str,
    l: &Listing,
    want: usize,
) -> ApiResult<HostRows> {
    let mut out = HostRows::default();
    match read_filtered_runs(
        conn,
        host_id,
        RunKind::All,
        l.lifecycle.clone(),
        &l.filter,
        FAN_OUT_LIMIT,
        want,
        l.facets,
    )
    .await
    .map_err(host_list_error)?
    {
        FilteredRuns::CannotReport(field) => {
            out.unfilterable.push((host_id.to_string(), field));
            return Ok(out);
        }
        FilteredRuns::Rows {
            matches, scoped, ..
        } => {
            out.rows = matches;
            out.scoped = scoped.unwrap_or_default();
        }
    }
    if l.attempts() {
        let events = match conn.list_autoflow_events().await {
            Ok(events) => events,
            Err(HostConnectorError::Unsupported(_)) => Vec::new(),
            Err(e) => {
                tracing::warn!(
                    host_id,
                    error = %e,
                    "autoflow runs: the host's event listing failed; listing its runs only"
                );
                Vec::new()
            }
        };
        for a in remote_attempt_rows(&events) {
            let mut v = serde_json::to_value(a).expect("an attempt row serializes");
            v["host_id"] = serde_json::json!(host_id);
            if l.admit(&v, &mut out.scoped) == Ok(true) {
                out.rows.push(v);
            }
        }
    }
    sort_newest_first(&mut out.rows);
    Ok(out)
}

/// Newest first by `started_at`: every row's is serde's `DateTime<Utc>`
/// (`Z` suffix), so the strings order like the instants.
fn sort_newest_first(rows: &mut [serde_json::Value]) {
    rows.sort_by(|a, b| {
        let ta = a["started_at"].as_str().unwrap_or("");
        let tb = b["started_at"].as_str().unwrap_or("");
        tb.cmp(ta)
            .then_with(|| b["id"].as_str().cmp(&a["id"].as_str()))
    });
}

/// `GET /api/runs/autoflows/runs[?host=][&lifecycle=][&since=&until=][&customer=][&q=][&facets=1]`
///
/// Autoflow-launched runs (rows whose `autoflow` is non-null) merged with
/// the failed attempts that never became a run, newest first, paged AFTER
/// the merge. One host with `?host=`, else every registered host (a remote
/// whose call fails is skipped, logged). `?customer=` and `?q=`
/// ([`crate::query::autoflow_runs::FIELDS`]) filter together, before the
/// page is cut and before any local run is priced (only the page's are).
/// Errors as `/api/runs/workflows`: 501 when the host can't serve the
/// listing or its rows can't report a field the filter reads (`autoflow`
/// on an older rupu), 502 when it gave no usable answer, 404 for an unknown
/// host id; on the fan-out such a host is skipped and named in
/// `X-Rupu-Hosts-Unfilterable` (`X-Rupu-Hosts-Without-Customer` for
/// `customer`). A `q` that doesn't parse is a 400 `{error, token, code,
/// start, end}`. Attempt rows are listed only under All and Failed·Rejected
/// and never while the filter reads a customer (an attempt records none);
/// the query keeps or drops them like any row. `is:archived` lists the
/// local archive's autoflow runs instead (no attempts; a remote host is a
/// 501, named `<host>:is` on the fan-out). `?facets=1` answers
/// `{rows, facets, facets_partial}`.
async fn list_autoflow_run_rows(
    State(s): State<AppState>,
    Query(q): Query<AutoflowRunsQuery>,
) -> Result<ListResponse, ListError> {
    let page = q.page();
    let l = q.listing()?;
    let remote = q.host.as_deref().filter(|h| *h != "local");
    if let (true, Some(host_id)) = (l.archived, remote) {
        resolve_host(&s, host_id)?;
        return Err(
            ApiError::not_available(format!("host {host_id} can't list archived runs")).into(),
        );
    }
    if q.host.as_deref() == Some("local") || l.archived {
        let mut local = local_rows(&s, &l).await?;
        let mut unfilterable = std::mem::take(&mut local.rows.unfilterable);
        if l.archived && q.host.is_none() {
            for h in s.hosts.list_hosts().into_iter().filter(|h| h.id != "local") {
                query::note_unfilterable(&mut unfilterable, &h.id, "is");
            }
        }
        let facets = l.facet_counts(&local.rows.scoped);
        let rows = crate::pagination::paginate(std::mem::take(&mut local.rows.rows), &page);
        return Ok(ListResponse {
            headers: query::unfilterable_headers(&unfilterable),
            rows: price_page(&s, local, rows).await?,
            facets,
        });
    }
    if let Some(host_id) = remote {
        let conn = resolve_host(&s, host_id)?;
        let want = page.offset().saturating_add(page.limit());
        let got = remote_rows(conn.as_ref(), host_id, &l, want).await?;
        if let Some((host, field)) = got.unfilterable.first() {
            return Err(query::unfilterable_error(host, field).into());
        }
        return Ok(ListResponse {
            headers: axum::http::HeaderMap::new(),
            facets: l.facet_counts(&got.scoped),
            rows: crate::pagination::paginate(got.rows, &page),
        });
    }
    // Fan-out: every host's whole filtered list, merged, paged, the page's
    // local runs priced.
    let futs: Vec<_> = s
        .hosts
        .list_hosts()
        .into_iter()
        .map(|h| {
            let host_id = h.id;
            let l = &l;
            let s = &s;
            async move {
                if host_id == "local" {
                    return local_rows(s, l).await.map(Err);
                }
                let listed = match s.hosts.resolve(&host_id) {
                    Ok(conn) => remote_rows(conn.as_ref(), &host_id, l, usize::MAX).await,
                    Err(e) => Err(host_list_error(e)),
                };
                Ok(Ok(listed.unwrap_or_else(|e| {
                    tracing::warn!(
                        host_id,
                        error = %e.1,
                        "autoflow runs fan-out: host failed; skipping it"
                    );
                    HostRows::default()
                })))
            }
        })
        .collect();
    let mut merged = HostRows::default();
    let mut local: Option<LocalRows> = None;
    for host in join_all(futs).await {
        let mut rows = match host? {
            Ok(remote) => remote,
            Err(mut l) => {
                let rows = std::mem::take(&mut l.rows);
                local = Some(l);
                rows
            }
        };
        merged.rows.append(&mut rows.rows);
        merged.scoped.append(&mut rows.scoped);
        for (h, f) in rows.unfilterable {
            query::note_unfilterable(&mut merged.unfilterable, &h, f);
        }
    }
    sort_newest_first(&mut merged.rows);
    let mut rows = crate::pagination::paginate(merged.rows, &page);
    if let Some(local) = local {
        rows = price_page(&s, local, rows).await?;
    }
    Ok(ListResponse {
        headers: query::unfilterable_headers(&merged.unfilterable),
        facets: l.facet_counts(&merged.scoped),
        rows,
    })
}

/// `GET /api/runs/autoflows/problems`'s parameters.
#[derive(serde::Deserialize)]
pub(crate) struct ProblemsQuery {
    /// One host; absent → every registered host.
    #[serde(default)]
    pub host: Option<String>,
}

/// How long a problem stays shown after it was last seen (spec: idle
/// cycles are not saved, so history can't tell a fix happened).
fn problem_cutoff() -> DateTime<Utc> {
    Utc::now() - chrono::Duration::hours(1)
}

/// The local host's problems: `cycle_failed` events naming no workflow
/// since `cutoff`.
fn local_problems_sync(global: &Path, cutoff: DateTime<Utc>) -> Vec<Problem> {
    let events: Vec<ProblemEvent> = history_events(global, Some(cutoff), None)
        .into_iter()
        .filter(|r| {
            r.event.kind == AutoflowCycleEventKind::CycleFailed && r.event.workflow.is_none()
        })
        .map(|r| ProblemEvent {
            at: r.at,
            repo_ref: r.event.repo_ref,
            worker: r.worker_name.or(r.worker_id),
            detail: r.event.detail,
        })
        .collect();
    problems(&events, "local", cutoff)
}

async fn local_problems(s: &AppState, cutoff: DateTime<Utc>) -> ApiResult<Vec<Problem>> {
    let global = s.global_dir.clone();
    blocking(move || Ok(local_problems_sync(&global, cutoff))).await
}

/// A remote host's problems from its event rows (`cycle_failed`, no
/// workflow); a host without an event listing has none to report.
async fn remote_problems(
    conn: &dyn HostConnector,
    host_id: &str,
    cutoff: DateTime<Utc>,
) -> ApiResult<Vec<Problem>> {
    let rows = match conn.list_autoflow_events().await {
        Ok(rows) => rows,
        Err(HostConnectorError::Unsupported(_)) => return Ok(Vec::new()),
        Err(e) => return Err(host_list_error(e)),
    };
    let str_of = |row: &serde_json::Value, key: &str| {
        row.get(key).and_then(|v| v.as_str()).map(str::to_owned)
    };
    let events: Vec<ProblemEvent> = rows
        .iter()
        .filter(|row| {
            row["kind"].as_str() == Some("cycle_failed") && row["workflow"].as_str().is_none()
        })
        .filter_map(|row| {
            Some(ProblemEvent {
                at: str_of(row, "at")?,
                repo_ref: str_of(row, "repo_ref"),
                worker: str_of(row, "worker_name"),
                detail: str_of(row, "detail"),
            })
        })
        .collect();
    Ok(problems(&events, host_id, cutoff))
}

/// `GET /api/runs/autoflows/problems[?host=]` — the autoflow failures that
/// name no workflow, seen within the past hour, grouped by host and subject
/// (the event's repo, else the cycle's worker), newest `last_seen` first.
/// One host with `?host=` (errors as the run list's), else every registered
/// host, a failing remote skipped.
async fn list_autoflow_problems(
    State(s): State<AppState>,
    Query(q): Query<ProblemsQuery>,
) -> ApiResult<Json<Vec<Problem>>> {
    let cutoff = problem_cutoff();
    let mut out = match q.host.as_deref() {
        Some("local") => local_problems(&s, cutoff).await?,
        Some(host_id) => {
            let conn = resolve_host(&s, host_id)?;
            remote_problems(conn.as_ref(), host_id, cutoff).await?
        }
        None => {
            let futs: Vec<_> = s
                .hosts
                .list_hosts()
                .into_iter()
                .map(|h| {
                    let host_id = h.id;
                    let s = &s;
                    async move {
                        if host_id == "local" {
                            return local_problems(s, cutoff).await;
                        }
                        let listed = match s.hosts.resolve(&host_id) {
                            Ok(conn) => remote_problems(conn.as_ref(), &host_id, cutoff).await,
                            Err(e) => Err(host_list_error(e)),
                        };
                        Ok(listed.unwrap_or_else(|e| {
                            tracing::warn!(
                                host_id,
                                error = %e.1,
                                "autoflow problems fan-out: host failed; skipping it"
                            );
                            Vec::new()
                        }))
                    }
                })
                .collect();
            join_all(futs)
                .await
                .into_iter()
                .collect::<ApiResult<Vec<Vec<Problem>>>>()?
                .into_iter()
                .flatten()
                .collect()
        }
    };
    out.sort_by_key(|p| std::cmp::Reverse(parse_at(&p.last_seen)));
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::autoflow_claims::ClaimIndex;
    use rupu_orchestrator::RunStatus;
    use rupu_runtime::{
        AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
        AutoflowHistoryEventRecord,
    };

    fn ev(
        kind: AutoflowCycleEventKind,
        at: &str,
        f: impl FnOnce(&mut AutoflowCycleEvent),
    ) -> AutoflowHistoryEventRecord {
        let at = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, at);
        let mut e = AutoflowCycleEvent {
            kind,
            ..Default::default()
        };
        f(&mut e);
        AutoflowHistoryEventRecord::from_cycle_event(&cycle, e, at)
    }

    #[test]
    fn cycle_failed_with_workflow_is_a_failed_attempt() {
        let events = [ev(
            AutoflowCycleEventKind::CycleFailed,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.issue_ref = Some("github:acme/api/issues/7".into());
                e.detail = Some("dispatch failed: boom".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, events[0].event_id);
        assert_eq!(rows[0].workflow_name, "fix-issue");
        assert_eq!(rows[0].status, RunStatus::Failed);
        assert_eq!(rows[0].trigger, "cron");
        assert_eq!(
            rows[0].attempt.detail.as_deref(),
            Some("dispatch failed: boom")
        );
        assert_eq!(rows[0].attempt.cycle_id, events[0].cycle_id);
        assert_eq!(
            rows[0].autoflow.as_ref().unwrap().issue_ref.as_deref(),
            Some("github:acme/api/issues/7")
        );
        assert_eq!(rows[0].run_recorded, None, "its id is a history event id");
        let v = serde_json::to_value(&rows[0]).unwrap();
        assert!(v.get("run_recorded").is_none());
        assert_eq!(v["started_at"], "2026-10-01T10:00:00Z");
        assert_eq!(v["usage"]["total_tokens"], 0);
    }

    #[test]
    fn cycle_failed_without_workflow_is_not_an_attempt() {
        let events = [ev(
            AutoflowCycleEventKind::CycleFailed,
            "2026-10-01T10:00:00Z",
            |e| e.repo_ref = Some("github:acme/api".into()),
        )];
        assert!(attempt_rows(&events, &|_| false, true, &ClaimIndex::default()).is_empty());
    }

    #[test]
    fn attempt_with_recorded_run_is_not_listed() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_exists".into());
                e.status = Some("blocked".into());
            },
        )];
        assert!(attempt_rows(
            &events,
            &|id| id == "run_exists",
            true,
            &ClaimIndex::default()
        )
        .is_empty());
    }

    #[test]
    fn launched_run_never_recorded_is_an_attempt_with_its_run_id() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.status = Some("blocked".into());
                e.wake_event_id = Some("evt_1".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows[0].id, "run_ghost");
        assert_eq!(rows[0].run_recorded, Some(false));
        assert_eq!(rows[0].status, RunStatus::Failed);
        assert_eq!(rows[0].trigger, "event");
        assert_eq!(rows[0].attempt.claim_status.as_deref(), Some("blocked"));
        // remote mode never checks launches
        assert!(attempt_rows(&events, &|_| false, false, &ClaimIndex::default()).is_empty());
    }

    #[test]
    fn launched_run_status_follows_the_claim_status_mapping() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.status = Some("await_human".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows[0].status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn attempts_are_newest_first_whatever_the_input_order() {
        let mk = |at: &str| {
            ev(AutoflowCycleEventKind::CycleFailed, at, |e| {
                e.workflow = Some("fix-issue".into())
            })
        };
        let events = [
            mk("2026-10-01T10:00:00Z"),
            mk("2026-10-01T12:00:00Z"),
            mk("2026-10-01T11:00:00Z"),
        ];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        let ats: Vec<String> = rows.iter().map(|r| r.started_at.to_rfc3339()).collect();
        assert_eq!(
            ats,
            [
                "2026-10-01T12:00:00+00:00",
                "2026-10-01T11:00:00+00:00",
                "2026-10-01T10:00:00+00:00"
            ]
        );
    }

    #[test]
    fn attempt_takes_its_claim_fields_from_the_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = rupu_workspace::AutoflowClaimStore {
            root: tmp.path().join("autoflows").join("claims"),
        };
        let mut claim = crate::api::autoflow_claims::tests::seed_record("github:acme/api/issues/7");
        claim.issue_display_ref = Some("acme/api#7".into());
        claim.issue_url = Some("https://example.test/7".into());
        claim.pr_url = Some("https://example.test/pr/9".into());
        claim.last_run_id = Some("run_ghost".into());
        store.save(&claim).unwrap();
        let idx = ClaimIndex::load(tmp.path());
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.issue_ref = Some("github:acme/api/issues/7".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &idx);
        assert_eq!(
            rows[0].autoflow,
            Some(crate::api::runs::RunAutoflow {
                issue_ref: Some("github:acme/api/issues/7".into()),
                issue_display_ref: Some("acme/api#7".into()),
                issue_url: Some("https://example.test/7".into()),
                pr_url: Some("https://example.test/pr/9".into()),
            })
        );
    }

    #[test]
    fn problems_group_count_and_age_out() {
        let pe = |at: &str, repo: Option<&str>, worker: Option<&str>, d: &str| ProblemEvent {
            at: at.into(),
            repo_ref: repo.map(Into::into),
            worker: worker.map(Into::into),
            detail: Some(d.into()),
        };
        let events = vec![
            pe("2026-10-01T09:00:00Z", Some("github:acme/api"), None, "old"),
            pe(
                "2026-10-01T10:10:00Z",
                Some("github:acme/api"),
                None,
                "config bad",
            ),
            pe(
                "2026-10-01T10:20:00Z",
                Some("github:acme/api"),
                None,
                "config still bad",
            ),
            pe("2026-10-01T10:30:00Z", None, Some("mini"), "rate limited"),
            pe("2026-10-01T10:05:00Z", None, None, "crashed"),
        ];
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-10-01T09:45:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let got = problems(&events, "local", cutoff);
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].subject_kind, got[0].subject.as_str(), got[0].count),
            ("worker", "mini", 1)
        );
        assert_eq!(
            (got[1].subject_kind, got[1].subject.as_str(), got[1].count),
            ("repo", "github:acme/api", 2)
        );
        assert_eq!(got[1].detail.as_deref(), Some("config still bad"));
        assert_eq!(got[1].first_seen, "2026-10-01T10:10:00Z");
        assert_eq!(got[1].last_seen, "2026-10-01T10:20:00Z");
        assert_eq!(got[1].host_id, "local");
        assert_eq!(
            (got[2].subject_kind, got[2].subject.as_str()),
            ("worker", "autoflow worker")
        );
    }

    // ── Endpoints ─────────────────────────────────────────────────────────

    use crate::state::AppState;
    use axum::extract::{Query, State};
    use rupu_runtime::AutoflowHistoryStore;

    fn state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// A completed run of `fix-issue` started at `started_at`; an autoflow
    /// run of `issue` when given, else a manual run (envelope, no autoflow).
    fn seed_run(s: &AppState, id: &str, started_at: &str, issue: Option<&str>) {
        let mut rec = crate::api::runs::tests::terminal_record(id);
        rec.workflow_name = "fix-issue".into();
        rec.started_at = at(started_at);
        rec.finished_at = Some(at(started_at));
        let mut env = crate::api::runs::tests::autoflow_envelope(id, issue);
        if issue.is_none() {
            env.autoflow = None;
        }
        // The envelope is written before the run record exists.
        s.run_store.write_run_envelope(id, &env).unwrap();
        s.run_store.create(rec, "name: x\n").unwrap();
    }

    fn history(s: &AppState) -> AutoflowHistoryStore {
        AutoflowHistoryStore::new(s.global_dir.join("autoflows").join("history"))
    }

    /// A `cycle_failed` history event at `when`, naming `workflow` (an
    /// attempt) or not (a problem).
    fn seed_failure(
        s: &AppState,
        when: &str,
        workflow: Option<&str>,
        repo: Option<&str>,
        detail: &str,
    ) -> AutoflowHistoryEventRecord {
        let at = at(when);
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, at);
        history(s)
            .append_cycle_event(
                &cycle,
                AutoflowCycleEvent {
                    kind: AutoflowCycleEventKind::CycleFailed,
                    workflow: workflow.map(Into::into),
                    repo_ref: repo.map(Into::into),
                    issue_ref: Some("github:acme/api/issues/7".into()),
                    detail: Some(detail.into()),
                    ..Default::default()
                },
                at,
            )
            .unwrap()
    }

    fn query(host: Option<&str>) -> AutoflowRunsQuery {
        AutoflowRunsQuery {
            offset: None,
            limit: None,
            lifecycle: None,
            host: host.map(Into::into),
            since: None,
            until: None,
            customer: None,
            q: None,
            facets: None,
        }
    }

    fn with_q(host: Option<&str>, q: &str) -> AutoflowRunsQuery {
        let mut query = query(host);
        query.q = Some(q.into());
        query
    }

    async fn call(s: &AppState, q: AutoflowRunsQuery) -> Result<ListResponse, ListError> {
        list_autoflow_run_rows(State(s.clone()), Query(q)).await
    }

    async fn call_list(s: &AppState, q: AutoflowRunsQuery) -> Vec<serde_json::Value> {
        call(s, q).await.expect("list ok").rows
    }

    fn api_error(e: ListError) -> ApiError {
        match e {
            ListError::Api(e) => e,
            ListError::Query(e) => panic!("a query error: {e:?}"),
        }
    }

    fn ids(rows: &[serde_json::Value]) -> Vec<String> {
        rows.iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn lists_autoflow_runs_not_manual_ones_with_attempts_newest_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_run(&s, "run_manual", "2026-10-01T11:00:00Z", None);
        let attempt = seed_failure(&s, "2026-10-01T12:00:00Z", Some("fix-issue"), None, "boom");
        let rows = call_list(&s, query(Some("local"))).await;
        assert_eq!(ids(&rows), [attempt.event_id.clone(), "run_af".to_string()]);
        assert_eq!(rows[0]["attempt"]["detail"], "boom");
        assert_eq!(rows[0]["status"], "failed");
        assert_eq!(rows[0]["host_id"], "local");
        assert!(rows[0].get("run_recorded").is_none());
        assert_eq!(rows[1]["autoflow"]["issue_ref"], "github:acme/api/issues/7");
        assert_eq!(rows[1]["host_id"], "local");
        assert!(rows[1].get("attempt").is_none());
        // The page's run is priced: its usage is the folded one, not a probe's.
        assert_eq!(rows[1]["usage"]["priced"], true);
        // The fan-out (no host) lists the same.
        let rows = call_list(&s, query(None)).await;
        assert_eq!(ids(&rows), [attempt.event_id, "run_af".to_string()]);
        assert_eq!(rows[1]["usage"]["priced"], true);
    }

    #[tokio::test]
    async fn merged_list_pages_after_merge() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_10",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/1"),
        );
        seed_run(
            &s,
            "run_12",
            "2026-10-01T12:00:00Z",
            Some("github:acme/api/issues/2"),
        );
        let a11 = seed_failure(&s, "2026-10-01T11:00:00Z", Some("fix-issue"), None, "a");
        let a13 = seed_failure(&s, "2026-10-01T13:00:00Z", Some("fix-issue"), None, "b");
        for host in [Some("local"), None] {
            let mut q = query(host);
            q.limit = Some(2);
            let page1 = call_list(&s, q).await;
            assert_eq!(ids(&page1), [a13.event_id.clone(), "run_12".to_string()]);
            let mut q = query(host);
            q.offset = Some(2);
            q.limit = Some(2);
            let page2 = call_list(&s, q).await;
            assert_eq!(ids(&page2), [a11.event_id.clone(), "run_10".to_string()]);
        }
        // since/until narrow runs and attempts alike.
        let mut q = query(Some("local"));
        q.since = Some("2026-10-01T10:30:00Z".into());
        q.until = Some("2026-10-01T12:30:00Z".into());
        assert_eq!(
            ids(&call_list(&s, q).await),
            ["run_12".to_string(), a11.event_id.clone()]
        );
        // So do `since:` / `until:` terms, on every host.
        for host in [Some("local"), None] {
            assert_eq!(
                ids(&call_list(
                    &s,
                    with_q(
                        host,
                        "since:2026-10-01T10:30:00Z until:2026-10-01T12:30:00Z"
                    )
                )
                .await),
                ["run_12".to_string(), a11.event_id.clone()]
            );
        }
    }

    #[tokio::test]
    async fn completed_filter_and_customer_scope_drop_attempts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        let attempt = seed_failure(&s, "2026-10-01T12:00:00Z", Some("fix-issue"), None, "boom");
        let mut q = query(Some("local"));
        q.lifecycle = Some("completed".into());
        assert_eq!(ids(&call_list(&s, q).await), ["run_af"]);
        let mut q = query(Some("local"));
        q.lifecycle = Some("failed".into());
        assert_eq!(ids(&call_list(&s, q).await), [attempt.event_id.as_str()]);
        // The query decides the same way.
        assert_eq!(
            ids(&call_list(&s, with_q(Some("local"), "status:completed")).await),
            ["run_af"]
        );
        assert_eq!(
            ids(&call_list(&s, with_q(None, "status:failed")).await),
            [attempt.event_id.as_str()]
        );
        // The run recorded no customer: `customer=none` keeps it, drops the attempt.
        let mut q = query(Some("local"));
        q.customer = Some("none".into());
        assert_eq!(ids(&call_list(&s, q).await), ["run_af"]);
        let mut q = query(None);
        q.customer = Some("none".into());
        assert_eq!(ids(&call_list(&s, q).await), ["run_af"]);
        // A `customer:` term reads the customer too: no attempt is listed.
        assert_eq!(
            ids(&call_list(&s, with_q(Some("local"), "customer:none")).await),
            ["run_af"]
        );
        let mut q = query(Some("local"));
        q.customer = Some("acme".into());
        assert!(call_list(&s, q).await.is_empty());
    }

    #[tokio::test]
    async fn workflow_term_narrows_runs_and_attempts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_fix",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        let mut other = crate::api::runs::tests::terminal_record("run_other");
        other.workflow_name = "triage".into();
        other.started_at = at("2026-10-01T10:30:00Z");
        s.run_store
            .write_run_envelope(
                "run_other",
                &crate::api::runs::tests::autoflow_envelope("run_other", None),
            )
            .unwrap();
        s.run_store.create(other, "name: x\n").unwrap();
        let fix = seed_failure(&s, "2026-10-01T11:00:00Z", Some("fix-issue"), None, "a");
        seed_failure(&s, "2026-10-01T12:00:00Z", Some("triage"), None, "b");
        for host in [Some("local"), None] {
            assert_eq!(
                ids(&call_list(&s, with_q(host, "workflow:fix-issue")).await),
                [fix.event_id.clone(), "run_fix".to_string()]
            );
        }
    }

    /// A claim for `issue` on `run_id`, with a display ref, an issue URL and
    /// (when given) a PR.
    fn seed_claim(s: &AppState, issue: &str, display: &str, run_id: &str, pr: Option<&str>) {
        let store = rupu_workspace::AutoflowClaimStore {
            root: s.global_dir.join("autoflows").join("claims"),
        };
        let mut claim = crate::api::autoflow_claims::tests::seed_record(issue);
        claim.issue_display_ref = Some(display.into());
        claim.issue_url = Some(format!("https://example.test/{display}"));
        claim.pr_url = pr.map(Into::into);
        claim.last_run_id = Some(run_id.into());
        store.save(&claim).unwrap();
    }

    #[tokio::test]
    async fn q_issue_matches_the_display_ref_and_the_ref() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_7",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_run(
            &s,
            "run_8",
            "2026-10-01T11:00:00Z",
            Some("github:acme/widgets/issues/8"),
        );
        seed_claim(&s, "github:acme/api/issues/7", "acme/api#7", "run_7", None);
        for host in [Some("local"), None] {
            // The display ref (from the claim) and the ref (from the envelope).
            assert_eq!(
                ids(&call_list(&s, with_q(host, "issue:api#7")).await),
                ["run_7"]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "issue:issues/7")).await),
                ["run_7"]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "issue:widgets")).await),
                ["run_8"]
            );
            // A bare word reaches the issue.
            assert_eq!(ids(&call_list(&s, with_q(host, "api#7")).await), ["run_7"]);
        }
        let rows = call_list(&s, with_q(Some("local"), "issue:api#7")).await;
        assert_eq!(rows[0]["autoflow"]["issue_display_ref"], "acme/api#7");
    }

    #[tokio::test]
    async fn q_pr_any_and_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_pr",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_run(
            &s,
            "run_nopr",
            "2026-10-01T11:00:00Z",
            Some("github:acme/api/issues/8"),
        );
        seed_claim(
            &s,
            "github:acme/api/issues/7",
            "acme/api#7",
            "run_pr",
            Some("https://example.test/pr/9"),
        );
        seed_claim(
            &s,
            "github:acme/api/issues/8",
            "acme/api#8",
            "run_nopr",
            None,
        );
        for host in [Some("local"), None] {
            assert_eq!(
                ids(&call_list(&s, with_q(host, "pr:any")).await),
                ["run_pr"]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "pr:none")).await),
                ["run_nopr"]
            );
        }
        let rows = call_list(&s, with_q(Some("local"), "pr:any")).await;
        assert_eq!(rows[0]["autoflow"]["pr_url"], "https://example.test/pr/9");
    }

    #[tokio::test]
    async fn q_kind_attempt_and_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        let boom = seed_failure(&s, "2026-10-01T12:00:00Z", Some("fix-issue"), None, "boom");
        let branch = seed_failure(
            &s,
            "2026-10-01T13:00:00Z",
            Some("fix-issue"),
            None,
            "dispatch failed: branch exists",
        );
        for host in [Some("local"), None] {
            assert_eq!(
                ids(&call_list(&s, with_q(host, "kind:attempt")).await),
                [branch.event_id.clone(), boom.event_id.clone()]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "kind:run")).await),
                ["run_af"]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "error:\"branch exists\"")).await),
                [branch.event_id.as_str()]
            );
            assert_eq!(
                ids(&call_list(&s, with_q(host, "boom")).await),
                [boom.event_id.as_str()]
            );
        }
    }

    #[tokio::test]
    async fn facets_count_runs_and_attempts_before_the_query() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_run(&s, "run_manual", "2026-10-01T11:00:00Z", None);
        seed_failure(&s, "2026-10-01T12:00:00Z", Some("fix-issue"), None, "boom");
        for host in [Some("local"), None] {
            let mut q = with_q(host, "kind:attempt");
            q.facets = Some("1".into());
            let resp = call(&s, q).await.unwrap();
            assert_eq!(resp.rows.len(), 1);
            let facets = serde_json::to_value(&resp.facets.expect("facets").facets).unwrap();
            // The manual run is outside the list: it counts nowhere.
            assert_eq!(facets["kind"], serde_json::json!({"attempt": 1, "run": 1}));
            assert_eq!(
                facets["status"],
                serde_json::json!({"completed": 1, "failed": 1})
            );
            assert_eq!(facets["pr"], serde_json::json!({"none": 2}));
            assert_eq!(facets["host"], serde_json::json!({"local": 2}));
        }
        // Without `facets=1` there are none.
        assert!(call(&s, query(Some("local")))
            .await
            .unwrap()
            .facets
            .is_none());
    }

    #[tokio::test]
    async fn is_archived_lists_the_archives_autoflow_runs_without_attempts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_run(
            &s,
            "run_live",
            "2026-10-01T11:00:00Z",
            Some("github:acme/api/issues/8"),
        );
        seed_run(&s, "run_manual", "2026-10-01T12:00:00Z", None);
        seed_failure(&s, "2026-10-01T13:00:00Z", Some("fix-issue"), None, "boom");
        s.run_store.archive("run_af").unwrap();
        s.run_store.archive("run_manual").unwrap();
        for host in [Some("local"), None] {
            let resp = call(&s, with_q(host, "is:archived")).await.unwrap();
            assert_eq!(ids(&resp.rows), ["run_af"]);
            assert_eq!(resp.rows[0]["archived"], true);
            assert_eq!(resp.rows[0]["host_id"], "local");
            assert_eq!(resp.rows[0]["usage"]["priced"], true);
        }
        let rows = call_list(&s, query(Some("local"))).await;
        assert_eq!(rows.len(), 2, "the live list: the attempt and run_live");
        assert_eq!(rows[1]["id"], "run_live");
    }

    #[tokio::test]
    async fn remote_without_autoflow_key_is_501_and_named_on_the_fan_out() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = crate::api::runs::tests::state_with_fake_host(
            &tmp,
            serde_json::json!({
                "runs": [{"id": "r1", "started_at": "2026-10-01T00:00:00Z",
                          "codename": "c", "codename_derived": false, "customer": null}],
            }),
        );
        let err = api_error(call(&s, query(Some("host_fake"))).await.unwrap_err());
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            err.1,
            "host host_fake can't report `autoflow` for every row"
        );
        let resp = call(&s, query(None)).await.unwrap();
        assert!(resp.rows.is_empty());
        assert_eq!(
            resp.headers
                .get(crate::query::HOSTS_UNFILTERABLE_HEADER)
                .unwrap(),
            "host_fake:autoflow"
        );
        // A remote host has no archive to list.
        let err = api_error(
            call(&s, with_q(Some("host_fake"), "is:archived"))
                .await
                .unwrap_err(),
        );
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
        let resp = call(&s, with_q(None, "is:archived")).await.unwrap();
        assert_eq!(
            resp.headers
                .get(crate::query::HOSTS_UNFILTERABLE_HEADER)
                .unwrap(),
            "host_fake:is"
        );
    }

    #[tokio::test]
    async fn a_bad_q_is_a_400() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        let err = call(&s, with_q(Some("local"), "sevrity:high"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ListError::Query(ref e) if serde_json::to_value(e.code).unwrap() == "unknown_key"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn remote_keeps_autoflow_rows_and_cycle_failed_attempts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let row = |id: &str, started: &str, af: serde_json::Value| {
            serde_json::json!({
                "id": id, "workflow_name": "fix-issue", "status": "completed",
                "started_at": started, "codename": "c", "codename_derived": false,
                "customer": null, "autoflow": af,
            })
        };
        let s = crate::api::runs::tests::state_with_fake_host(
            &tmp,
            serde_json::json!({
                "paged": true,
                "runs": [
                    row("r_manual", "2026-10-01T13:00:00Z", serde_json::Value::Null),
                    row("r_af", "2026-10-01T10:00:00Z", serde_json::json!({"issue_ref": "github:acme/api/issues/7"})),
                    row("r_pr", "2026-10-01T09:00:00Z", serde_json::json!({"issue_ref": "github:acme/api/issues/8", "pr_url": "https://example.test/pr/9"})),
                ],
                "autoflow_events": [
                    {"event_id": "afe_1", "cycle_id": "cyc_1", "at": "2026-10-01T12:00:00Z",
                     "kind": "cycle_failed", "workflow": "fix-issue", "issue_display_ref": "acme/api#8",
                     "run_id": null, "status": null, "worker_name": "prod", "detail": "boom"},
                    {"event_id": "afe_2", "cycle_id": "cyc_1", "at": "2026-10-01T12:30:00Z",
                     "kind": "cycle_failed", "workflow": null, "repo_ref": "github:acme/api",
                     "detail": "config bad"},
                    {"event_id": "afe_3", "cycle_id": "cyc_1", "at": "2026-10-01T11:00:00Z",
                     "kind": "run_launched", "workflow": "fix-issue", "run_id": "r_af"},
                ],
            }),
        );
        let rows = call_list(&s, query(Some("host_fake"))).await;
        assert_eq!(ids(&rows), ["afe_1", "r_af", "r_pr"]);
        assert_eq!(rows[0]["host_id"], "host_fake");
        assert_eq!(rows[0]["attempt"]["worker"], "prod");
        assert_eq!(rows[0]["autoflow"]["issue_display_ref"], "acme/api#8");
        assert_eq!(rows[1]["host_id"], "host_fake");
        // A customer scope keeps the runs (recorded none) and drops the attempt.
        let mut q = query(Some("host_fake"));
        q.customer = Some("none".into());
        assert_eq!(ids(&call_list(&s, q).await), ["r_af", "r_pr"]);
        // The window applies to the merge: page two is the run.
        let mut q = query(Some("host_fake"));
        q.offset = Some(1);
        q.limit = Some(1);
        assert_eq!(ids(&call_list(&s, q).await), ["r_af"]);
        // The query reads a remote row's autoflow and attempt like a local one's.
        assert_eq!(
            ids(&call_list(&s, with_q(Some("host_fake"), "pr:any")).await),
            ["r_pr"]
        );
        assert_eq!(
            ids(&call_list(&s, with_q(Some("host_fake"), "issue:api#8")).await),
            ["afe_1"],
            "the attempt's display ref"
        );
        assert_eq!(
            ids(&call_list(&s, with_q(Some("host_fake"), "issue:issues/8")).await),
            ["r_pr"],
            "the run's ref"
        );
        assert_eq!(
            ids(&call_list(&s, with_q(Some("host_fake"), "kind:attempt error:boom")).await),
            ["afe_1"]
        );
        // The fan-out (local has nothing) names the host in the `host` facet.
        let mut q = with_q(None, "kind:run");
        q.facets = Some("1".into());
        let resp = call(&s, q).await.unwrap();
        assert_eq!(ids(&resp.rows), ["r_af", "r_pr"]);
        let facets = serde_json::to_value(&resp.facets.unwrap().facets).unwrap();
        assert_eq!(facets["kind"], serde_json::json!({"attempt": 1, "run": 2}));
        assert_eq!(facets["host"], serde_json::json!({"host_fake": 3}));
    }

    #[tokio::test]
    async fn corrupt_history_lists_runs_without_attempts_and_no_problems() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        seed_run(
            &s,
            "run_af",
            "2026-10-01T10:00:00Z",
            Some("github:acme/api/issues/7"),
        );
        seed_failure(&s, "2026-10-01T12:00:00Z", Some("fix-issue"), None, "boom");
        let day = s.global_dir.join("autoflows/history/events/2026-10-01");
        std::fs::write(day.join("afe_garbage.json"), b"{not json").unwrap();
        assert_eq!(ids(&call_list(&s, query(Some("local"))).await), ["run_af"]);
        assert_eq!(ids(&call_list(&s, query(None)).await), ["run_af"]);
        // A problem seen just now would list — but garbage in its day's
        // directory makes the whole history read fail, and the strip stays empty.
        let recent = chrono::Utc::now() - chrono::Duration::minutes(5);
        seed_failure(
            &s,
            &recent.to_rfc3339(),
            None,
            Some("github:acme/api"),
            "config bad",
        );
        let today = s
            .global_dir
            .join("autoflows/history/events")
            .join(recent.format("%Y-%m-%d").to_string());
        std::fs::write(today.join("afe_garbage2.json"), b"{not json").unwrap();
        for host in [Some("local"), None] {
            let axum::Json(problems) = list_autoflow_problems(
                State(s.clone()),
                Query(ProblemsQuery {
                    host: host.map(Into::into),
                }),
            )
            .await
            .unwrap();
            assert!(problems.is_empty(), "{problems:?}");
        }
    }

    /// A [`crate::customers::PriceBook`] that counts the rows priced.
    struct CountingPrices<'a>(crate::customers::FlatPricing<'a>, usize);

    impl crate::customers::PriceBook for CountingPrices<'_> {
        fn pricing_for(&mut self, customer: Option<&str>) -> &rupu_config::PricingConfig {
            self.1 += 1;
            self.0.pricing_for(customer)
        }
    }

    #[test]
    fn only_the_returned_page_is_priced() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        for (id, started) in [
            ("run_a", "2026-10-01T10:00:00Z"),
            ("run_b", "2026-10-01T11:00:00Z"),
            ("run_c", "2026-10-01T12:00:00Z"),
        ] {
            seed_run(&s, id, started, Some("github:acme/api/issues/7"));
        }
        let mut prices = CountingPrices(crate::customers::FlatPricing(&s.pricing), 0);
        let listing = with_q(Some("local"), "workflow:fix").listing().unwrap();
        let mut local = select_local(Arc::clone(&s.run_store), &s.global_dir, &listing).unwrap();
        assert_eq!(prices.1, 0, "selecting prices nothing");
        let page = crate::pagination::PageQuery {
            offset: Some(1),
            limit: Some(1),
        };
        let mut rows = crate::pagination::paginate(std::mem::take(&mut local.rows.rows), &page);
        local.price(&mut rows, &mut prices);
        assert_eq!(ids(&rows), ["run_b"]);
        assert_eq!(rows[0]["usage"]["priced"], true);
        assert_eq!(prices.1, 1, "only the page's run is priced");
    }

    #[tokio::test]
    async fn problems_endpoint_groups_recent_failures_without_workflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state(&tmp);
        let now = chrono::Utc::now();
        let ago = |m: i64| (now - chrono::Duration::minutes(m)).to_rfc3339();
        seed_failure(&s, &ago(50), None, Some("github:acme/api"), "config bad");
        seed_failure(
            &s,
            &ago(20),
            None,
            Some("github:acme/api"),
            "config still bad",
        );
        seed_failure(&s, &ago(10), None, None, "crashed");
        seed_failure(&s, &ago(90), None, Some("github:acme/old"), "aged out");
        seed_failure(
            &s,
            &ago(5),
            Some("fix-issue"),
            None,
            "an attempt, not a problem",
        );
        for host in [Some("local"), None] {
            let axum::Json(got) = list_autoflow_problems(
                State(s.clone()),
                Query(ProblemsQuery {
                    host: host.map(Into::into),
                }),
            )
            .await
            .unwrap();
            let subjects: Vec<(&str, &str, usize)> = got
                .iter()
                .map(|p| (p.subject_kind, p.subject.as_str(), p.count))
                .collect();
            assert_eq!(
                subjects,
                [
                    ("worker", "autoflow worker", 1),
                    ("repo", "github:acme/api", 2)
                ]
            );
            assert_eq!(got[1].detail.as_deref(), Some("config still bad"));
            assert_eq!(got[1].host_id, "local");
        }
    }

    #[tokio::test]
    async fn remote_problems_come_from_its_event_rows() {
        let tmp = tempfile::TempDir::new().unwrap();
        let recent = (chrono::Utc::now() - chrono::Duration::minutes(3)).to_rfc3339();
        let s = crate::api::runs::tests::state_with_fake_host(
            &tmp,
            serde_json::json!({
                "autoflow_events": [
                    {"event_id": "afe_2", "cycle_id": "cyc_1", "at": recent,
                     "kind": "cycle_failed", "workflow": null, "repo_ref": "github:acme/api",
                     "worker_name": "prod", "detail": "config bad"},
                    {"event_id": "afe_1", "cycle_id": "cyc_1", "at": recent,
                     "kind": "cycle_failed", "workflow": "fix-issue", "detail": "an attempt"},
                ],
            }),
        );
        let axum::Json(got) = list_autoflow_problems(
            State(s),
            Query(ProblemsQuery {
                host: Some("host_fake".into()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].host_id, "host_fake");
        assert_eq!(
            (got[0].subject_kind, got[0].subject.as_str()),
            ("repo", "github:acme/api")
        );
    }
}
