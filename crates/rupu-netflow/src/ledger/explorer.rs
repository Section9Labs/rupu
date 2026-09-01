//! Server-side aggregates for the Network explorer UI (rupu-cp's
//! `GET /api/netflow/explorer`).
//!
//! Per the repo convention established by [`super::HostRollup`] ("computed
//! server-side" so the percentile / unknown-bytes logic has exactly ONE
//! implementation), every aggregate the explorer surface renders is built
//! here, in Rust, once — the web client only fetches and formats.
//!
//! Nothing in this module resolves ASN or workflow names itself — the
//! caller (rupu-cp, which owns read-time enrichment per spec §6.2 and the
//! run stores) resolves both per flow and hands them in as [`ExplorerFlow`]
//! fields. A flow whose peer had no ASN entry groups under the explicit
//! `unknown` org ("Unknown network"); a flow whose ledger id matched no
//! run record groups under the explicit `unknown` workflow ("No
//! workflow") — never silently dropped, in either dimension.
//!
//! ## Honesty rules carried forward (see `views.rs` / the CP components)
//!
//! - Every visual weight is CALL-based. Byte counts never feed a weight:
//!   Coarse flows' bytes sum to 0 by construction, so a byte weight would
//!   render "unobservable" as "tiny".
//! - [`KpiView::bytes_in`]/`bytes_out` are observed-only sums, `None` when
//!   NO in-scope flow contributed an observed value (rendering a sum of
//!   zero observed values as `0 B` would claim we saw zero bytes), with
//!   [`KpiView::bytes_partial`] flagging that any flow's count was
//!   unobservable so the UI can mark the sum as partial.
//! - Percentiles are nearest-rank via [`super::views`]'s one
//!   implementation, `None` when no flow contributed a duration.

use super::views::{percentile, TimeRange};
use crate::asn::AsnInfo;
use crate::record::{Fidelity, FlowRecord, Outcome};
use crate::Origin;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The explicit bucket for flows whose dimension value could not be
/// resolved — shared by the workflow and org dimensions so a filter key
/// for "the unresolved group" is spelled one way everywhere.
pub const UNKNOWN_KEY: &str = "unknown";
/// Display label for the [`UNKNOWN_KEY`] org (no ASN entry / no peer IP).
pub const UNKNOWN_ORG_LABEL: &str = "Unknown network";
/// Display label for the [`UNKNOWN_KEY`] workflow (ledger id matched no
/// run record — e.g. a standalone agent run, which never enters the
/// orchestrator's `RunStore`).
pub const UNKNOWN_WORKFLOW_LABEL: &str = "No workflow";

/// One flow plus the read-time attribution the caller resolved for it.
#[derive(Debug, Clone)]
pub struct ExplorerFlow {
    /// The top-level run this flow folds into (ledger id mapped through
    /// step/sub-agent records back to its root run) — `None` when the
    /// ledger id matched no run record anywhere.
    pub run_id: Option<String>,
    /// `RunRecord::workflow_name` of that run; `None` when unresolvable.
    pub workflow: Option<String>,
    /// Read-time ASN enrichment; `None` when the peer IP is unknown
    /// (Coarse fidelity) or the table has no entry.
    pub asn: Option<AsnInfo>,
    pub flow: FlowRecord,
}

/// The workflow-dimension filter key for a resolved-or-not workflow name.
pub fn workflow_key_of(workflow: Option<&str>) -> &str {
    workflow.unwrap_or(UNKNOWN_KEY)
}

/// The org-dimension filter key for a resolved-or-not ASN: `as<number>`,
/// or [`UNKNOWN_KEY`].
pub fn org_key_of(asn: Option<&AsnInfo>) -> String {
    match asn {
        Some(a) => format!("as{}", a.asn),
        None => UNKNOWN_KEY.to_string(),
    }
}

/// The endpoint-dimension filter key: `host:port`, matched exactly — the
/// same string [`LaneAgg`]/org-card rows carry, so the client never
/// parses it back apart.
pub fn endpoint_key_of(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

impl ExplorerFlow {
    /// The workflow-dimension filter key for this flow.
    pub fn workflow_key(&self) -> &str {
        workflow_key_of(self.workflow.as_deref())
    }

    /// The origin-dimension filter key: `provider:anthropic`,
    /// `scm:github`, or the bare unit-variant name.
    pub fn origin_key(&self) -> String {
        origin_key(&self.flow.ctx.origin)
    }

    /// The org-dimension filter key: `as<number>`, or [`UNKNOWN_KEY`].
    pub fn org_key(&self) -> String {
        org_key_of(self.asn.as_ref())
    }

    /// The endpoint-dimension filter key — see [`endpoint_key_of`].
    pub fn endpoint_key(&self) -> String {
        endpoint_key_of(&self.flow.host, self.flow.port)
    }

    fn is_error(&self) -> bool {
        !matches!(self.flow.outcome, Outcome::Ok)
    }
}

/// Render an [`Origin`] as its stable filter key (also its display label —
/// the origin column shows these strings verbatim, mirroring the flow
/// table's `originLabel`).
pub fn origin_key(origin: &Origin) -> String {
    match origin {
        Origin::Provider(name) => format!("provider:{name}"),
        Origin::Scm(name) => format!("scm:{name}"),
        Origin::Update => "update".to_string(),
        Origin::Cp => "cp".to_string(),
        Origin::System => "system".to_string(),
    }
}

/// The active cross-filter sets, one per topology dimension plus the
/// endpoint set the timeline/org-cards toggle. Empty set = dimension
/// unfiltered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExplorerFilters {
    pub workflows: Vec<String>,
    pub origins: Vec<String>,
    pub orgs: Vec<String>,
    /// `host:port` strings, matched exactly against [`ExplorerFlow::endpoint_key`].
    pub hosts: Vec<String>,
}

/// The dimension a per-column aggregate deliberately does NOT filter by —
/// standard faceted-filter semantics: selecting a workflow must not zero
/// out the other workflows in the workflow column itself, or there would
/// be no way to see (or undo into) the alternatives. Everything else
/// (links, KPIs, lanes' cells, the table) applies ALL filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipDim {
    Workflow,
    Origin,
    Org,
    Host,
}

impl ExplorerFilters {
    pub fn is_empty(&self) -> bool {
        self.workflows.is_empty()
            && self.origins.is_empty()
            && self.orgs.is_empty()
            && self.hosts.is_empty()
    }

    /// Whether `f` passes every active filter, optionally skipping one
    /// dimension (see [`SkipDim`]).
    pub fn passes(&self, f: &ExplorerFlow, skip: Option<SkipDim>) -> bool {
        let dim = |skip_this: SkipDim, set: &[String], key: &str| {
            skip == Some(skip_this) || set.is_empty() || set.iter().any(|v| v == key)
        };
        dim(SkipDim::Workflow, &self.workflows, f.workflow_key())
            && dim(SkipDim::Origin, &self.origins, &f.origin_key())
            && dim(SkipDim::Org, &self.orgs, &f.org_key())
            && dim(SkipDim::Host, &self.hosts, &f.endpoint_key())
    }
}

/// One node in a topology column. `calls`/`errors` are computed under the
/// current window + cross-filters (minus the node's own dimension); a node
/// present with `calls: 0` is IN SCOPE but filtered/windowed out — the
/// client renders it dimmed, never removes it. Out-of-scope nodes are
/// absent from the response entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAgg {
    pub id: String,
    pub label: String,
    pub calls: u64,
    pub errors: u64,
}

/// One ribbon between adjacent topology columns, aggregated under the
/// window + ALL active filters. `calls` is the only sanctioned visual
/// weight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAgg {
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub errors: u64,
}

/// The three-column topology: Workflows → Origins → Networks (ASN orgs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SankeyView {
    pub workflows: Vec<NodeAgg>,
    pub origins: Vec<NodeAgg>,
    pub orgs: Vec<NodeAgg>,
    pub wf_origin: Vec<LinkAgg>,
    pub origin_org: Vec<LinkAgg>,
}

/// One dense time bucket: total calls and how many of them erred.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketAgg {
    pub calls: u64,
    pub errors: u64,
}

/// One endpoint swimlane, grouped by the client under its org header.
/// Aggregated under the window + every filter EXCEPT the host dimension
/// (so a selected endpoint's siblings stay visible for comparison /
/// deselection — same faceted semantics as the topology columns).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneAgg {
    pub host: String,
    pub port: u16,
    /// Org display label ([`UNKNOWN_ORG_LABEL`] for the unknown group).
    pub org: String,
    /// Org filter key (`as<number>` / [`UNKNOWN_KEY`]) — what a lane's
    /// org header groups by and what the org filter matches.
    pub org_id: String,
    /// `None` for the unknown-org group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asn: Option<u32>,
    /// The LEAST-observable fidelity among this lane's contributing flows
    /// (`coarse` < `http` < `full`): a lane is only as observable as its
    /// blindest contributor, so a mixed lane is tagged by its gap, not
    /// its best case.
    pub fidelity: Fidelity,
    pub calls: u64,
    pub errors: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<u64>,
    /// Dense — always exactly the view's bucket count, zeros included.
    pub buckets: Vec<BucketAgg>,
}

/// Per-endpoint swimlanes over the applied window (or the full retained
/// range when unbounded), plus the active-runs correlation strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineView {
    pub bucket_ms: u64,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub lanes: Vec<LaneAgg>,
    /// Dense, one entry per bucket: how many in-scope runs were active
    /// (their start..end span overlapped that bucket).
    pub runs: Vec<u64>,
    /// Distinct in-scope runs active anywhere in the window.
    pub runs_in_window: u64,
}

/// The activity strip's full-history histogram. Bounds are SERVER-chosen
/// (min/max recorded `ts` across the scope, ignoring any window or
/// filter — the strip is the context the window is dragged against) and
/// echoed back so drag-to-zoom can map pixels to time without guessing.
/// `from`/`to` are `null` (and every bucket zero) when the scope has no
/// flows at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistogramView {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub bucket_ms: u64,
    pub buckets: Vec<BucketAgg>,
}

/// The KPI strip, over the window + ALL active filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KpiView {
    pub flows: u64,
    /// Distinct `host:port` endpoints.
    pub endpoints: u64,
    /// Distinct ASN orgs (the unknown group counts as one when present).
    pub orgs: u64,
    pub errors: u64,
    /// Sum of OBSERVED byte counts only — `None` when no flow contributed
    /// an observed value (never `Some(0)` for "we saw nothing").
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    /// `true` when any counted flow had an unobservable byte count on
    /// either side — the sums above are then partial and the UI marks
    /// them (`† observed only`).
    pub bytes_partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<u64>,
}

/// A run's lifetime for the active-runs strip. `end: None` = still
/// running, i.e. active through the end of any window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSpan {
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
}

/// Fixed dense bucket count for both the activity strip and timeline
/// lanes.
pub const EXPLORER_BUCKETS: usize = 84;

fn bucket_index(
    ts: DateTime<Utc>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    n: usize,
) -> Option<usize> {
    if ts < from || ts > to || n == 0 {
        return None;
    }
    let span = (to - from).num_milliseconds().max(1) as f64;
    let offset = (ts - from).num_milliseconds() as f64;
    Some(((offset / span * n as f64) as usize).min(n - 1))
}

fn bucketize<'a>(
    flows: impl Iterator<Item = &'a ExplorerFlow>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    n: usize,
) -> Vec<BucketAgg> {
    let mut buckets = vec![BucketAgg::default(); n];
    for f in flows {
        if let Some(i) = bucket_index(f.flow.ts, from, to, n) {
            buckets[i].calls += 1;
            if f.is_error() {
                buckets[i].errors += 1;
            }
        }
    }
    buckets
}

fn bucket_ms(from: DateTime<Utc>, to: DateTime<Utc>, n: usize) -> u64 {
    ((to - from).num_milliseconds().max(0) as u64) / (n.max(1) as u64)
}

/// Sort key shared by every aggregate list: calls DESC, then id ASC — so
/// output ordering is deterministic and the busiest rows lead.
fn sort_aggs(list: &mut [NodeAgg]) {
    list.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.id.cmp(&b.id)));
}

struct Acc {
    calls: u64,
    errors: u64,
}

fn aggregate<'a>(
    flows: impl Iterator<Item = &'a ExplorerFlow>,
    key: impl Fn(&ExplorerFlow) -> String,
) -> HashMap<String, Acc> {
    let mut m: HashMap<String, Acc> = HashMap::new();
    for f in flows {
        let e = m.entry(key(f)).or_insert(Acc {
            calls: 0,
            errors: 0,
        });
        e.calls += 1;
        if f.is_error() {
            e.errors += 1;
        }
    }
    m
}

/// Build the three-column topology.
///
/// Node UNIVERSE per column comes from `scope_flows` unfiltered by window
/// or cross-filters — "out of scope is absent entirely; in scope but
/// filtered out renders dimmed" needs the full in-scope node list. Node
/// AGGREGATES apply `window` + `filters` minus the node's own dimension
/// ([`SkipDim`]); links apply `window` + all of `filters`.
pub fn sankey_view(
    scope_flows: &[ExplorerFlow],
    window: &TimeRange,
    filters: &ExplorerFilters,
) -> SankeyView {
    let in_win = |f: &&ExplorerFlow| window.contains(f.flow.ts);

    let column = |skip: SkipDim,
                  key: fn(&ExplorerFlow) -> String,
                  label: &dyn Fn(&ExplorerFlow) -> String|
     -> Vec<NodeAgg> {
        // Universe: every key present in scope, labeled from its first
        // occurrence (labels are 1:1 with keys for every dimension).
        let mut labels: BTreeMap<String, String> = BTreeMap::new();
        for f in scope_flows {
            labels.entry(key(f)).or_insert_with(|| label(f));
        }
        let agg = aggregate(
            scope_flows
                .iter()
                .filter(in_win)
                .filter(|f| filters.passes(f, Some(skip))),
            key,
        );
        let mut nodes: Vec<NodeAgg> = labels
            .into_iter()
            .map(|(id, label)| {
                let a = agg.get(&id);
                NodeAgg {
                    label,
                    calls: a.map_or(0, |a| a.calls),
                    errors: a.map_or(0, |a| a.errors),
                    id,
                }
            })
            .collect();
        sort_aggs(&mut nodes);
        nodes
    };

    let workflows = column(
        SkipDim::Workflow,
        |f| f.workflow_key().to_string(),
        &|f| match &f.workflow {
            Some(w) => w.clone(),
            None => UNKNOWN_WORKFLOW_LABEL.to_string(),
        },
    );
    let origins = column(SkipDim::Origin, |f| f.origin_key(), &|f| f.origin_key());
    let orgs = column(SkipDim::Org, |f| f.org_key(), &|f| match &f.asn {
        Some(a) => a.org.clone(),
        None => UNKNOWN_ORG_LABEL.to_string(),
    });

    let filtered: Vec<&ExplorerFlow> = scope_flows
        .iter()
        .filter(in_win)
        .filter(|f| filters.passes(f, None))
        .collect();
    let links = |key: fn(&ExplorerFlow) -> (String, String)| -> Vec<LinkAgg> {
        let mut m: HashMap<(String, String), Acc> = HashMap::new();
        for f in &filtered {
            let e = m.entry(key(f)).or_insert(Acc {
                calls: 0,
                errors: 0,
            });
            e.calls += 1;
            if f.is_error() {
                e.errors += 1;
            }
        }
        let mut out: Vec<LinkAgg> = m
            .into_iter()
            .map(|((from, to), a)| LinkAgg {
                from,
                to,
                calls: a.calls,
                errors: a.errors,
            })
            .collect();
        out.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then_with(|| a.from.cmp(&b.from))
                .then_with(|| a.to.cmp(&b.to))
        });
        out
    };

    SankeyView {
        workflows,
        origins,
        orgs,
        wf_origin: links(|f| (f.workflow_key().to_string(), f.origin_key())),
        origin_org: links(|f| (f.origin_key(), f.org_key())),
    }
}

/// Build the timeline: one lane per endpoint reached under `window` +
/// `filters`-minus-host, bucketized densely, plus the active-runs strip.
///
/// `from`/`to` here are the RESOLVED window: the caller substitutes the
/// scope's full retained range (the histogram bounds) for an unbounded
/// side before calling, so the lanes always have concrete bounds to
/// bucket against.
pub fn timeline_view(
    scope_flows: &[ExplorerFlow],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    filters: &ExplorerFilters,
    runs: &[RunSpan],
    n: usize,
) -> TimelineView {
    let visible: Vec<&ExplorerFlow> = scope_flows
        .iter()
        .filter(|f| f.flow.ts >= from && f.flow.ts <= to)
        .filter(|f| filters.passes(f, Some(SkipDim::Host)))
        .collect();

    struct LaneAcc<'a> {
        first: &'a ExplorerFlow,
        flows: Vec<&'a ExplorerFlow>,
    }
    let mut lanes_by_key: BTreeMap<String, LaneAcc> = BTreeMap::new();
    for f in &visible {
        lanes_by_key
            .entry(f.endpoint_key())
            .or_insert_with(|| LaneAcc {
                first: f,
                flows: Vec::new(),
            })
            .flows
            .push(f);
    }

    let mut lanes: Vec<LaneAgg> = lanes_by_key
        .into_values()
        .map(|acc| {
            let mut ms: Vec<u64> = acc
                .flows
                .iter()
                .filter_map(|f| f.flow.duration_ms)
                .collect();
            ms.sort_unstable();
            let errors = acc.flows.iter().filter(|f| f.is_error()).count() as u64;
            // Least-observable contributor tags the lane — see the field doc.
            let fidelity = acc
                .flows
                .iter()
                .map(|f| f.flow.fidelity)
                .min_by_key(|f| match f {
                    Fidelity::Coarse => 0u8,
                    Fidelity::Http => 1,
                    Fidelity::Full => 2,
                })
                .unwrap_or(Fidelity::Coarse);
            let org_id = org_key_of(acc.first.asn.as_ref());
            let (org, asn) = match &acc.first.asn {
                Some(a) => (a.org.clone(), Some(a.asn)),
                None => (UNKNOWN_ORG_LABEL.to_string(), None),
            };
            LaneAgg {
                host: acc.first.flow.host.clone(),
                port: acc.first.flow.port,
                org,
                org_id,
                asn,
                fidelity,
                calls: acc.flows.len() as u64,
                errors,
                p50_ms: percentile(&ms, 0.50),
                p95_ms: percentile(&ms, 0.95),
                buckets: bucketize(acc.flows.iter().copied(), from, to, n),
            }
        })
        .collect();
    // Group lanes by org (busiest org first, then busiest lane first
    // within it) so the client can render org headers with a plain
    // "did the org change since the previous lane" scan.
    let mut org_calls: HashMap<String, u64> = HashMap::new();
    for l in &lanes {
        *org_calls.entry(l.org_id.clone()).or_insert(0) += l.calls;
    }
    lanes.sort_by(|a, b| {
        let oa = org_calls.get(&a.org_id).copied().unwrap_or(0);
        let ob = org_calls.get(&b.org_id).copied().unwrap_or(0);
        ob.cmp(&oa)
            .then_with(|| a.org_id.cmp(&b.org_id))
            .then_with(|| b.calls.cmp(&a.calls))
            .then_with(|| a.host.cmp(&b.host))
    });

    let active: Vec<&RunSpan> = runs
        .iter()
        .filter(|r| r.start <= to && r.end.is_none_or(|e| e >= from))
        .collect();
    let span_ms = (to - from).num_milliseconds().max(1) as f64;
    let mut run_buckets = vec![0u64; n];
    for (i, bucket) in run_buckets.iter_mut().enumerate() {
        let b0 = from + chrono::Duration::milliseconds((span_ms * i as f64 / n as f64) as i64);
        let b1 =
            from + chrono::Duration::milliseconds((span_ms * (i + 1) as f64 / n as f64) as i64);
        *bucket = active
            .iter()
            .filter(|r| r.start <= b1 && r.end.is_none_or(|e| e >= b0))
            .count() as u64;
    }

    TimelineView {
        bucket_ms: bucket_ms(from, to, n),
        from,
        to,
        lanes,
        runs: run_buckets,
        runs_in_window: active.len() as u64,
    }
}

/// Build the activity-strip histogram over the scope's WHOLE retained
/// range — no window, no cross-filters (see [`HistogramView`]).
pub fn histogram_view(scope_flows: &[ExplorerFlow], n: usize) -> HistogramView {
    let mut min: Option<DateTime<Utc>> = None;
    let mut max: Option<DateTime<Utc>> = None;
    for f in scope_flows {
        let ts = f.flow.ts;
        min = Some(min.map_or(ts, |m| m.min(ts)));
        max = Some(max.map_or(ts, |m| m.max(ts)));
    }
    match (min, max) {
        (Some(from), Some(to)) => HistogramView {
            from: Some(from),
            to: Some(to),
            bucket_ms: bucket_ms(from, to, n),
            buckets: bucketize(scope_flows.iter(), from, to, n),
        },
        _ => HistogramView {
            from: None,
            to: None,
            bucket_ms: 0,
            buckets: vec![BucketAgg::default(); n],
        },
    }
}

/// Build the KPI strip over the fully filtered, windowed set (the caller
/// passes exactly that set — this function applies no filtering itself).
pub fn kpi_view(filtered: &[ExplorerFlow]) -> KpiView {
    let mut endpoints: HashSet<String> = HashSet::new();
    let mut orgs: HashSet<String> = HashSet::new();
    let mut errors = 0u64;
    let mut bytes_in: Option<u64> = None;
    let mut bytes_out: Option<u64> = None;
    let mut bytes_partial = false;
    let mut ms: Vec<u64> = Vec::new();

    for f in filtered {
        endpoints.insert(f.endpoint_key());
        orgs.insert(f.org_key());
        if f.is_error() {
            errors += 1;
        }
        match f.flow.bytes_in {
            // Observed-only sum: unlike `HostRollup`'s collapse-to-`None`,
            // one unobservable flow does not blank the KPI — it flips
            // `bytes_partial` so the UI marks the sum instead.
            Some(b) => bytes_in = Some(bytes_in.unwrap_or(0) + b),
            None => bytes_partial = true,
        }
        match f.flow.bytes_out {
            Some(b) => bytes_out = Some(bytes_out.unwrap_or(0) + b),
            None => bytes_partial = true,
        }
        if let Some(d) = f.flow.duration_ms {
            ms.push(d);
        }
    }
    ms.sort_unstable();

    KpiView {
        flows: filtered.len() as u64,
        endpoints: endpoints.len() as u64,
        orgs: orgs.len() as u64,
        errors,
        bytes_in,
        bytes_out,
        bytes_partial,
        p95_ms: percentile(&ms, 0.95),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::FlowCtx;
    use crate::record::FlowId;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn flow(
        id: u64,
        ts: i64,
        host: &str,
        origin: Origin,
        workflow: Option<&str>,
        asn: Option<(u32, &str)>,
        ok: bool,
    ) -> ExplorerFlow {
        ExplorerFlow {
            run_id: Some(format!("run-{id}")),
            workflow: workflow.map(str::to_string),
            asn: asn.map(|(asn, org)| AsnInfo {
                asn,
                org: org.to_string(),
            }),
            flow: FlowRecord {
                id: FlowId::from_parts(id, id as u128),
                ts: at(ts),
                ctx: FlowCtx {
                    run_id: None,
                    step_id: None,
                    agent: None,
                    workspace_id: None,
                    origin,
                },
                fidelity: Fidelity::Http,
                method: "POST".into(),
                scheme: "https".into(),
                host: host.into(),
                port: 443,
                path: "/v1".into(),
                peer_ip: None,
                resolved_ips: vec![],
                http_version: None,
                status: Some(if ok { 200 } else { 500 }),
                outcome: if ok { Outcome::Ok } else { Outcome::HttpError },
                error: None,
                bytes_out: Some(10),
                bytes_in: Some(20),
                body_complete: true,
                ttfb_ms: Some(5),
                duration_ms: Some(30),
            },
        }
    }

    fn anthropic(id: u64, ts: i64, ok: bool) -> ExplorerFlow {
        flow(
            id,
            ts,
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
            Some("review"),
            Some((13335, "Cloudflare")),
            ok,
        )
    }

    fn github(id: u64, ts: i64) -> ExplorerFlow {
        flow(
            id,
            ts,
            "api.github.com",
            Origin::Scm("github".into()),
            Some("triage"),
            Some((36459, "GitHub")),
            true,
        )
    }

    #[test]
    fn sankey_groups_unresolved_flows_under_explicit_unknowns_not_dropped() {
        let mut orphan = anthropic(1, 100, true);
        orphan.workflow = None;
        orphan.asn = None;
        let flows = vec![orphan, github(2, 100)];

        let s = sankey_view(&flows, &TimeRange::unbounded(), &ExplorerFilters::default());

        let wf_unknown = s.workflows.iter().find(|n| n.id == UNKNOWN_KEY).unwrap();
        assert_eq!(wf_unknown.label, UNKNOWN_WORKFLOW_LABEL);
        assert_eq!(wf_unknown.calls, 1);
        let org_unknown = s.orgs.iter().find(|n| n.id == UNKNOWN_KEY).unwrap();
        assert_eq!(org_unknown.label, UNKNOWN_ORG_LABEL);
        assert_eq!(org_unknown.calls, 1);
        // Both links survive too — nothing silently dropped.
        assert!(s
            .wf_origin
            .iter()
            .any(|l| l.from == UNKNOWN_KEY && l.to == "provider:anthropic"));
        assert!(s
            .origin_org
            .iter()
            .any(|l| l.from == "provider:anthropic" && l.to == UNKNOWN_KEY));
    }

    #[test]
    fn sankey_keeps_filtered_out_nodes_with_zero_calls_for_dimming() {
        // An org filter is active; the OTHER org's node must stay present
        // (calls 0 → client dims it) and the filtered dimension's own
        // column must keep real counts for every entry (facet semantics).
        let flows = vec![anthropic(1, 100, true), github(2, 100)];
        let filters = ExplorerFilters {
            orgs: vec!["as13335".into()],
            ..Default::default()
        };

        let s = sankey_view(&flows, &TimeRange::unbounded(), &filters);

        let gh_org = s.orgs.iter().find(|n| n.id == "as36459").unwrap();
        assert_eq!(
            gh_org.calls, 1,
            "own dimension is skipped when aggregating its own column"
        );
        let triage = s.workflows.iter().find(|n| n.id == "triage").unwrap();
        assert_eq!(
            triage.calls, 0,
            "cross-filtered-out node stays present with zero calls"
        );
        assert_eq!(s.workflows.len(), 2, "no node removed by a cross-filter");
        // Links apply ALL filters: only the anthropic path survives.
        assert_eq!(s.wf_origin.len(), 1);
        assert_eq!(s.wf_origin[0].from, "review");
    }

    #[test]
    fn sankey_window_narrows_aggregates_but_not_the_node_universe() {
        let flows = vec![anthropic(1, 100, true), anthropic(2, 500, false)];
        let window = TimeRange {
            from: Some(at(400)),
            to: Some(at(600)),
        };

        let s = sankey_view(&flows, &window, &ExplorerFilters::default());

        let wf = s.workflows.iter().find(|n| n.id == "review").unwrap();
        assert_eq!(wf.calls, 1, "only the in-window flow counts");
        assert_eq!(wf.errors, 1);
        assert_eq!(s.workflows.len(), 1, "same node, still present");
    }

    #[test]
    fn timeline_buckets_are_dense_and_lane_stats_are_per_endpoint() {
        let flows = vec![
            anthropic(1, 0, true),
            anthropic(2, 999, false),
            github(3, 500),
        ];

        let t = timeline_view(&flows, at(0), at(999), &ExplorerFilters::default(), &[], 10);

        assert_eq!(t.lanes.len(), 2);
        for lane in &t.lanes {
            assert_eq!(lane.buckets.len(), 10, "dense: zeros included");
        }
        let anth = t
            .lanes
            .iter()
            .find(|l| l.host == "api.anthropic.com")
            .unwrap();
        assert_eq!(anth.calls, 2);
        assert_eq!(anth.errors, 1);
        assert_eq!(anth.org_id, "as13335");
        assert_eq!(anth.buckets[0].calls, 1);
        assert_eq!(anth.buckets[9].calls, 1);
        assert_eq!(anth.buckets[9].errors, 1);
        assert_eq!(anth.p95_ms, Some(30));
    }

    #[test]
    fn timeline_lanes_group_contiguously_by_org_busiest_first() {
        let flows = vec![
            github(1, 100),
            anthropic(2, 100, true),
            anthropic(3, 200, true),
            // Second Cloudflare endpoint so the org spans two lanes.
            flow(
                4,
                150,
                "cdn.anthropic.com",
                Origin::Provider("anthropic".into()),
                Some("review"),
                Some((13335, "Cloudflare")),
                true,
            ),
        ];

        let t = timeline_view(&flows, at(0), at(300), &ExplorerFilters::default(), &[], 4);

        let org_order: Vec<&str> = t.lanes.iter().map(|l| l.org_id.as_str()).collect();
        assert_eq!(
            org_order,
            vec!["as13335", "as13335", "as36459"],
            "org groups contiguous, busiest org first"
        );
        assert_eq!(
            t.lanes[0].host, "api.anthropic.com",
            "busiest lane first in group"
        );
    }

    #[test]
    fn timeline_host_filter_skips_its_own_dimension_for_lane_visibility() {
        let flows = vec![anthropic(1, 100, true), github(2, 100)];
        let filters = ExplorerFilters {
            hosts: vec!["api.github.com:443".into()],
            ..Default::default()
        };

        let t = timeline_view(&flows, at(0), at(200), &filters, &[], 4);

        assert_eq!(
            t.lanes.len(),
            2,
            "an endpoint filter must not hide the other lanes (facet semantics)"
        );
    }

    #[test]
    fn timeline_coarse_contributor_tags_the_whole_lane_coarse() {
        let mut coarse = github(1, 100);
        coarse.flow.fidelity = Fidelity::Coarse;
        let flows = vec![coarse, github(2, 150)];

        let t = timeline_view(&flows, at(0), at(200), &ExplorerFilters::default(), &[], 4);

        assert_eq!(t.lanes[0].fidelity, Fidelity::Coarse);
    }

    #[test]
    fn timeline_active_runs_strip_counts_overlapping_spans() {
        let runs = vec![
            RunSpan {
                start: at(0),
                end: Some(at(400)),
            },
            RunSpan {
                start: at(600),
                end: None, // still running — active through the window end
            },
            RunSpan {
                start: at(2000),
                end: Some(at(3000)), // entirely outside
            },
        ];

        let t = timeline_view(&[], at(0), at(1000), &ExplorerFilters::default(), &runs, 4);

        assert_eq!(t.runs_in_window, 2);
        assert_eq!(t.runs.len(), 4);
        assert_eq!(t.runs[0], 1, "first run only");
        assert_eq!(t.runs[3], 1, "unfinished run active at the end");
    }

    #[test]
    fn histogram_covers_the_full_retained_range_and_echoes_bounds() {
        let flows = vec![anthropic(1, 100, true), anthropic(2, 940, false)];

        let h = histogram_view(&flows, 84);

        assert_eq!(h.from, Some(at(100)));
        assert_eq!(h.to, Some(at(940)));
        assert_eq!(h.buckets.len(), 84);
        assert_eq!(h.buckets.iter().map(|b| b.calls).sum::<u64>(), 2);
        assert_eq!(h.buckets[0].calls, 1);
        assert_eq!(h.buckets[83].calls, 1);
        assert_eq!(h.buckets[83].errors, 1);
    }

    #[test]
    fn histogram_with_no_flows_has_null_bounds_and_zero_buckets() {
        let h = histogram_view(&[], 84);
        assert_eq!(h.from, None);
        assert_eq!(h.to, None);
        assert_eq!(h.buckets.len(), 84);
        assert!(h.buckets.iter().all(|b| b.calls == 0 && b.errors == 0));
    }

    #[test]
    fn kpis_sum_observed_bytes_only_and_flag_partial() {
        let mut coarse = github(1, 100);
        coarse.flow.bytes_in = None;
        coarse.flow.bytes_out = None;
        let flows = vec![anthropic(2, 100, false), coarse];

        let k = kpi_view(&flows);

        assert_eq!(k.flows, 2);
        assert_eq!(k.endpoints, 2);
        assert_eq!(k.orgs, 2);
        assert_eq!(k.errors, 1);
        assert_eq!(k.bytes_in, Some(20), "observed contributor only");
        assert_eq!(k.bytes_out, Some(10));
        assert!(
            k.bytes_partial,
            "one unobservable contributor marks the sum"
        );
        assert_eq!(k.p95_ms, Some(30));
    }

    #[test]
    fn kpis_with_no_observed_bytes_report_none_not_zero() {
        let mut coarse = github(1, 100);
        coarse.flow.bytes_in = None;
        coarse.flow.bytes_out = None;

        let k = kpi_view(&[coarse]);

        assert_eq!(k.bytes_in, None, "a sum of nothing observed is not 0 B");
        assert_eq!(k.bytes_out, None);
        assert!(k.bytes_partial);
    }

    #[test]
    fn kpis_on_an_empty_set_are_all_zero_and_none() {
        let k = kpi_view(&[]);
        assert_eq!(k.flows, 0);
        assert_eq!(k.bytes_in, None);
        assert!(!k.bytes_partial);
        assert_eq!(k.p95_ms, None);
    }

    #[test]
    fn filters_pass_matches_each_dimension_by_its_key() {
        let f = anthropic(1, 100, true);
        let hit = ExplorerFilters {
            workflows: vec!["review".into()],
            origins: vec!["provider:anthropic".into()],
            orgs: vec!["as13335".into()],
            hosts: vec!["api.anthropic.com:443".into()],
        };
        assert!(hit.passes(&f, None));

        let miss = ExplorerFilters {
            workflows: vec!["triage".into()],
            ..Default::default()
        };
        assert!(!miss.passes(&f, None));
        assert!(miss.passes(&f, Some(SkipDim::Workflow)));
    }
}
