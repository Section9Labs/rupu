//! Read-side views over the append-only ledger.
//!
//! Nothing here resolves ASN — enrichment happens in the CP API at
//! render time (spec §6.2), so a dataset that arrives late improves
//! every historical record with no backfill.

use crate::record::{FlowId, FlowRecord, LedgerLine, Outcome};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read every flow (folding `Complete` lines into their flow) AND the
/// total dropped-count, in a single pass over the ledger file.
///
/// [`read_flows`] and [`read_dropped_total`] both delegate here — calling
/// both of them back-to-back for the common "I need flows and the dropped
/// count" case (every CP netflow endpoint) would open and line-scan the
/// same file twice for no reason. Callers that want both should call this
/// directly rather than the two single-purpose wrappers.
///
/// A missing file is an empty ledger (`(vec![], 0)`), not an error.
/// Malformed lines are skipped — a torn write at the tail must not lose
/// the whole history.
pub fn read_flows_and_dropped(path: &Path) -> std::io::Result<(Vec<FlowRecord>, u64)> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(e),
    };

    let mut flows: Vec<FlowRecord> = Vec::new();
    let mut index: HashMap<FlowId, usize> = HashMap::new();
    let mut dropped = 0u64;

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<LedgerLine>(&line) else {
            continue;
        };
        match parsed {
            LedgerLine::Flow(f) => {
                index.insert(f.id, flows.len());
                flows.push(*f);
            }
            LedgerLine::Complete {
                id,
                bytes_in,
                duration_ms,
            } => {
                if let Some(&i) = index.get(&id) {
                    flows[i].bytes_in = Some(bytes_in);
                    flows[i].duration_ms = Some(duration_ms);
                    flows[i].body_complete = true;
                }
            }
            LedgerLine::Dropped { count, .. } => {
                dropped += count;
            }
        }
    }

    Ok((flows, dropped))
}

/// Read every flow, folding `Complete` lines into their flow.
///
/// A missing file is an empty ledger, not an error. Malformed lines are
/// skipped — a torn write at the tail must not lose the whole history.
///
/// Prefer [`read_flows_and_dropped`] if the caller also needs the dropped
/// count — that reads the file once instead of twice.
pub fn read_flows(path: &Path) -> std::io::Result<Vec<FlowRecord>> {
    Ok(read_flows_and_dropped(path)?.0)
}

/// Total records lost to writer-channel overflow.
///
/// Prefer [`read_flows_and_dropped`] if the caller also needs the flows —
/// that reads the file once instead of twice.
pub fn read_dropped_total(path: &Path) -> std::io::Result<u64> {
    Ok(read_flows_and_dropped(path)?.1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRollup {
    pub host: String,
    pub port: u16,
    pub calls: u64,
    /// `None` when ANY contributing flow had an unknown byte count — a
    /// `Coarse` record cannot be summed into a total that claims to be
    /// complete. Summing `unwrap_or(0)` would silently turn "we could
    /// not see it" into "it was zero".
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub errors: u64,
    /// `None` when no contributing flow had a known `duration_ms` — e.g.
    /// every flow for this host is a streamed request still in flight, or
    /// this host only ever produced transport failures before `Instant`
    /// timing was captured. A CP reading `p50_ms: 0` cannot tell
    /// "sub-millisecond" from "we timed nothing"; `None` says so
    /// honestly. Mirrors the `bytes_in` / `bytes_out` honesty contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<u64>,
}

fn is_error(f: &FlowRecord) -> bool {
    !matches!(f.outcome, Outcome::Ok)
}

/// Nearest-rank percentile — the standard definition: the smallest value
/// at or below which at least `pct` of the samples fall, i.e. index
/// `ceil(pct * n) - 1`. `pct` in 0.0..=1.0. `sorted` must be ascending.
///
/// `None` on an empty slice — "no timings observed" is not the same fact
/// as "the observed timings were all zero". A bare `0` fallback here would
/// have been indistinguishable from a genuine sub-millisecond p50.
///
/// Do NOT "fix" the non-empty case to `ceil(pct * (n - 1))` to make a test
/// pass. That is a different, nonstandard definition; if a test disagrees
/// with nearest-rank, the test's expected value is what is wrong.
pub(crate) fn percentile(sorted: &[u64], pct: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (pct * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    Some(sorted[idx])
}

/// `bytes_in` and `bytes_out` are tracked INDEPENDENTLY. A single shared
/// "known" flag would be wrong: an in-flight streamed flow has a known
/// `bytes_out` and an unknown `bytes_in`, and one shared flag would blank
/// the host's `bytes_out` total too — discarding data we actually have.
struct RollupAcc {
    calls: u64,
    /// `None` once any contributor's own `bytes_in` is unknown.
    bytes_in: Option<u64>,
    /// `None` once any contributor's own `bytes_out` is unknown.
    bytes_out: Option<u64>,
    errors: u64,
    ms: Vec<u64>,
}

impl Default for RollupAcc {
    fn default() -> Self {
        Self {
            calls: 0,
            bytes_in: Some(0),
            bytes_out: Some(0),
            errors: 0,
            ms: Vec::new(),
        }
    }
}

/// Add one flow's contribution, collapsing to `None` on its own gap only.
fn accumulate(total: Option<u64>, sample: Option<u64>) -> Option<u64> {
    match (total, sample) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None,
    }
}

pub fn host_rollup(flows: &[FlowRecord]) -> Vec<HostRollup> {
    host_rollup_iter(flows)
}

/// [`host_rollup`] over an iterator of REFERENCES — for callers whose
/// flows live inside a wrapper type (`FlowView`/`ExplorerFlow` on the CP
/// read side), so they never deep-clone the whole set into a `Vec` just
/// to satisfy the slice signature.
pub fn host_rollup_iter<'a>(flows: impl IntoIterator<Item = &'a FlowRecord>) -> Vec<HostRollup> {
    let mut acc: HashMap<(String, u16), RollupAcc> = HashMap::new();

    for f in flows {
        let entry = acc.entry((f.host.clone(), f.port)).or_default();
        entry.calls += 1;
        // Per-field: one unobservable contributor makes THAT total
        // unknowable, and only that one.
        entry.bytes_in = accumulate(entry.bytes_in, f.bytes_in);
        entry.bytes_out = accumulate(entry.bytes_out, f.bytes_out);
        if is_error(f) {
            entry.errors += 1;
        }
        if let Some(ms) = f.duration_ms {
            entry.ms.push(ms);
        }
    }

    acc.into_iter()
        .map(|((host, port), mut a)| {
            a.ms.sort_unstable();
            HostRollup {
                host,
                port,
                calls: a.calls,
                bytes_in: a.bytes_in,
                bytes_out: a.bytes_out,
                errors: a.errors,
                p50_ms: percentile(&a.ms, 0.50),
                p95_ms: percentile(&a.ms, 0.95),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeSide {
    Source,
    Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub side: NodeSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub bytes: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// An inclusive time window over `FlowRecord::ts`.
///
/// `ts` is the ONLY timestamp a `FlowRecord` carries, and it is stamped
/// after the response headers land (or after a transport failure settles)
/// — see `http/middleware.rs`, which calls `chrono::Utc::now()` right
/// after `next.run(...)` returns, not before the request is sent. A later
/// `LedgerLine::Complete` (streamed-body drain finishing) updates
/// `bytes_in`/`duration_ms` but never `ts`. So a long-running streamed
/// request that started before this window and finished draining inside
/// it is judged by when its headers arrived, not by when it started or
/// by when its body finally completed — the only instant this subsystem
/// actually captures.
///
/// Both bounds are INCLUSIVE: a flow at exactly `from` or exactly `to` is
/// in range. Absent bounds mean unbounded on that side, so
/// `TimeRange::unbounded()` selects everything and existing callers are
/// unaffected. `from > to` is not rejected — it is simply an empty
/// window, since no timestamp can satisfy both `>= from` and `<= to`
/// (see `an_inverted_range_contains_nothing`); callers building a range
/// from untrusted input get an empty result, not a panic or a
/// silently-reordered range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeRange {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
}

impl TimeRange {
    pub fn unbounded() -> Self {
        Self::default()
    }

    pub fn contains(&self, ts: chrono::DateTime<chrono::Utc>) -> bool {
        self.from.is_none_or(|f| ts >= f) && self.to.is_none_or(|t| ts <= t)
    }
}

/// Read a ledger, keeping only flows whose `ts` falls inside `range`
/// (see [`TimeRange`] for the inclusive/inclusive, `from > to` = empty,
/// and "which timestamp" semantics).
///
/// The returned `Dropped` total is NOT filtered by `range` — it is the
/// same whole-file total `read_flows_and_dropped` returns. A record lost
/// to writer-channel overflow has no timestamp to test against a window,
/// and suppressing (or partially counting) it because of a time filter
/// would silently under-report loss on a filtered view — exactly the
/// defect this subsystem exists to prevent. A caller must not read
/// `dropped: 0` on a narrow window as "nothing was lost in this window";
/// it means "nothing was lost in this FILE", full stop.
pub fn read_flows_in_range(
    path: &Path,
    range: &TimeRange,
) -> std::io::Result<(Vec<FlowRecord>, u64)> {
    let (flows, dropped) = read_flows_and_dropped(path)?;
    Ok((
        flows.into_iter().filter(|f| range.contains(f.ts)).collect(),
        dropped,
    ))
}

/// Bipartite topology: sources on one side, `host:port` endpoints on the
/// other. The source id for each flow is supplied by the CALLER, not
/// derived from `f.ctx.run_id` — no production `FlowCtx` ever populates
/// that field (see `crate::ctx::FlowCtx`'s doc comment: attribution is by
/// ledger FILE, not by this field), so deriving the source from it
/// collapsed every graph at every scope into one node labelled `system`.
/// The read side (`rupu-cp/src/api/netflow.rs`'s `resolve_ledger_paths`/
/// `read_all_run_ledgers_in_dir`) already knows which run/ledger-file each
/// flow came from, so it passes that id in directly: the run's own id at
/// run scope (one source node per run, even though its flows may be
/// spread across several of that run's own per-step ledger files — see
/// `run_and_unit_ids`), or the ledger file's own name at project/global
/// scope (one source node per run whose ledger contributed to that
/// union).
pub fn graph_view(flows: &[(String, FlowRecord)]) -> GraphView {
    let mut nodes: HashMap<String, GraphNode> = HashMap::new();
    let mut edges: HashMap<(String, String), GraphEdge> = HashMap::new();

    for (source_id, f) in flows {
        let source_id = source_id.clone();
        let endpoint_id = format!("{}:{}", f.host, f.port);

        nodes.entry(source_id.clone()).or_insert_with(|| GraphNode {
            id: source_id.clone(),
            label: source_id.clone(),
            side: NodeSide::Source,
        });
        nodes
            .entry(endpoint_id.clone())
            .or_insert_with(|| GraphNode {
                id: endpoint_id.clone(),
                label: f.host.clone(),
                side: NodeSide::Endpoint,
            });

        let edge = edges
            .entry((source_id.clone(), endpoint_id.clone()))
            .or_insert_with(|| GraphEdge {
                from: source_id.clone(),
                to: endpoint_id.clone(),
                calls: 0,
                bytes: 0,
                errors: 0,
            });
        edge.calls += 1;
        // Edge weight is a VISUAL scale for stroke thickness, not a
        // reported total — unlike `HostRollup::bytes_in`, which must
        // stay `None` when unknown. A Coarse flow contributes 0 here,
        // so its edge simply renders thin. Never surface this number
        // as a byte count in a table.
        edge.bytes += f.bytes_in.unwrap_or(0) + f.bytes_out.unwrap_or(0);
        if is_error(f) {
            edge.errors += 1;
        }
    }

    GraphView {
        nodes: nodes.into_values().collect(),
        edges: edges.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::{FlowCtx, Origin};
    use crate::record::{Fidelity, Outcome};

    fn flow(id: u64, host: &str, run: Option<&str>, ms: u64, ok: bool) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(id, id as u128),
            ts: chrono::Utc::now(),
            ctx: FlowCtx {
                run_id: run.map(str::to_string),
                step_id: Some("s1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws".into()),
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
            status: Some(if ok { 200 } else { 500 }),
            outcome: if ok { Outcome::Ok } else { Outcome::HttpError },
            error: None,
            bytes_out: Some(100),
            bytes_in: Some(200),
            body_complete: true,
            ttfb_ms: Some(10),
            duration_ms: Some(ms),
        }
    }

    fn write_lines(path: &std::path::Path, lines: &[LedgerLine]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = lines
            .iter()
            .map(|l| format!("{}\n", serde_json::to_string(l).unwrap()))
            .collect();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn read_flows_folds_complete_into_its_flow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");

        let mut streamed = flow(1, "api.anthropic.com", Some("r1"), 0, true);
        streamed.bytes_in = None;
        streamed.body_complete = false;
        let id = streamed.id;

        write_lines(
            &path,
            &[
                LedgerLine::Flow(Box::new(streamed)),
                LedgerLine::Complete {
                    id,
                    bytes_in: 8192,
                    duration_ms: 4200,
                },
            ],
        );

        let flows = read_flows(&path).unwrap();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].bytes_in, Some(8192));
        assert_eq!(flows[0].duration_ms, Some(4200));
        assert!(flows[0].body_complete);
    }

    #[test]
    fn read_flows_skips_malformed_lines_without_failing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");
        std::fs::create_dir_all(tmp.path()).unwrap();
        let good = serde_json::to_string(&LedgerLine::Flow(Box::new(flow(
            1,
            "example.test",
            Some("r1"),
            5,
            true,
        ))))
        .unwrap();
        std::fs::write(&path, format!("{{not json\n{good}\n\n")).unwrap();

        let flows = read_flows(&path).unwrap();
        assert_eq!(flows.len(), 1);
    }

    #[test]
    fn read_flows_on_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let flows = read_flows(&tmp.path().join("absent.jsonl")).unwrap();
        assert!(flows.is_empty());
    }

    #[test]
    fn host_rollup_groups_and_computes_percentiles() {
        let flows = vec![
            flow(1, "api.anthropic.com", Some("r1"), 10, true),
            flow(2, "api.anthropic.com", Some("r1"), 20, true),
            flow(3, "api.anthropic.com", Some("r1"), 100, false),
            flow(4, "api.github.com", Some("r1"), 50, true),
        ];
        let mut rollup = host_rollup(&flows);
        rollup.sort_by(|a, b| a.host.cmp(&b.host));

        assert_eq!(rollup.len(), 2);
        assert_eq!(rollup[0].host, "api.anthropic.com");
        assert_eq!(rollup[0].calls, 3);
        assert_eq!(rollup[0].errors, 1);
        assert_eq!(rollup[0].bytes_in, Some(600));
        assert_eq!(rollup[0].bytes_out, Some(300));
        assert_eq!(rollup[0].p50_ms, Some(20));
        assert_eq!(rollup[0].p95_ms, Some(100));
        assert_eq!(rollup[1].host, "api.github.com");
        assert_eq!(rollup[1].calls, 1);
    }

    #[test]
    fn a_host_serving_only_gets_keeps_a_real_bytes_out_total() {
        // Fix 3 (2026-08-03 review): before the fix, the middleware
        // recorded `bytes_out: None` for every bodyless GET (the update
        // checker, every `/v1/models` probe). Composed with
        // `host_rollup`'s collapse-on-unknown rule, ANY host that ever
        // served a GET would permanently report `bytes_out: None`. Now
        // that a bodyless request records a known `Some(0)`, a host with
        // only GET flows must keep a real (summed, non-None) total.
        let mut get_one = flow(1, "api.github.com", Some("r1"), 10, true);
        get_one.bytes_out = Some(0);
        let mut get_two = flow(2, "api.github.com", Some("r1"), 20, true);
        get_two.bytes_out = Some(0);

        let rollup = host_rollup(&[get_one, get_two]);

        assert_eq!(
            rollup[0].bytes_out,
            Some(0),
            "an all-GET host's bytes_out total must stay a real known \
             value, not collapse to None"
        );
    }

    #[test]
    fn a_host_with_no_observed_durations_reports_none_not_zero() {
        // Fix 4 (2026-08-03 review): `p50_ms`/`p95_ms` must distinguish
        // "no flow contributed a duration" from "the duration was zero" —
        // the same honesty contract `bytes_in`/`bytes_out` already have.
        // A transport failure recorded before any timing was captured is
        // exactly this shape: `duration_ms: None`.
        let mut untimed = flow(1, "api.github.com", Some("r1"), 0, true);
        untimed.duration_ms = None;

        let rollup = host_rollup(&[untimed]);

        assert_eq!(rollup[0].calls, 1);
        assert_eq!(
            rollup[0].p50_ms, None,
            "no contributing flow had a known duration"
        );
        assert_eq!(rollup[0].p95_ms, None);
    }

    #[test]
    fn one_unknown_byte_count_makes_the_host_total_unknown() {
        // A Coarse record cannot be summed into a total that claims to
        // be complete. `None` says "we could not see it"; 600 would be
        // a false claim and 0 would be worse.
        let mut coarse = flow(1, "api.github.com", Some("r1"), 10, true);
        coarse.fidelity = Fidelity::Coarse;
        coarse.bytes_in = None;
        coarse.bytes_out = None;

        let flows = vec![flow(2, "api.github.com", Some("r1"), 20, true), coarse];
        let rollup = host_rollup(&flows);

        assert_eq!(rollup[0].calls, 2, "calls are still exact");
        assert_eq!(rollup[0].bytes_in, None);
        assert_eq!(rollup[0].bytes_out, None);
        // Durations are [10, 20]; nearest-rank p50 = ceil(0.5*2) = rank 1
        // = the LOWER of the two. Timings stay exact even though bytes
        // became unknowable.
        assert_eq!(rollup[0].p50_ms, Some(10), "timings are still exact");
    }

    #[test]
    fn an_unknown_bytes_in_does_not_blank_a_known_bytes_out() {
        // The ordinary in-flight streaming shape: response body still
        // draining, so `bytes_in` is unknown while `bytes_out` (the
        // request we sent) is perfectly well known.
        let mut streaming = flow(1, "api.anthropic.com", Some("r1"), 10, true);
        streaming.bytes_in = None;
        streaming.body_complete = false;

        let flows = vec![
            flow(2, "api.anthropic.com", Some("r1"), 20, true),
            streaming,
        ];
        let rollup = host_rollup(&flows);

        assert_eq!(
            rollup[0].bytes_in, None,
            "one unknown in makes the in-total unknown"
        );
        assert_eq!(
            rollup[0].bytes_out,
            Some(200),
            "every bytes_out was known, so the out-total must survive"
        );
    }

    #[test]
    fn graph_view_is_bipartite_source_to_endpoint() {
        let flows = vec![
            (
                "r1".to_string(),
                flow(1, "api.anthropic.com", Some("r1"), 10, true),
            ),
            (
                "r1".to_string(),
                flow(2, "api.anthropic.com", Some("r1"), 20, false),
            ),
            (
                "r2".to_string(),
                flow(3, "api.github.com", Some("r2"), 30, true),
            ),
        ];
        let g = graph_view(&flows);

        let sources: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.side == NodeSide::Source)
            .collect();
        let endpoints: Vec<_> = g
            .nodes
            .iter()
            .filter(|n| n.side == NodeSide::Endpoint)
            .collect();
        assert_eq!(sources.len(), 2, "one per run");
        assert_eq!(endpoints.len(), 2, "one per host:port");

        let e = g
            .edges
            .iter()
            .find(|e| e.to == "api.anthropic.com:443")
            .unwrap();
        assert_eq!(e.calls, 2);
        assert_eq!(e.errors, 1);
        assert_eq!(e.bytes, 600);
    }

    #[test]
    fn source_id_is_whatever_the_caller_supplies_not_ctx_run_id() {
        // `graph_view` no longer derives the source from `f.ctx.run_id`
        // (no production `FlowCtx` ever populates it) -- the caller
        // passes the owning run id explicitly. This flow's own
        // `ctx.run_id` is `None` (unattributed by construction) yet the
        // node must still take the SUPPLIED id, proving the field is
        // genuinely ignored, not just usually absent.
        let flows = vec![(
            "run-x".to_string(),
            flow(1, "api.github.com", None, 10, true),
        )];
        let g = graph_view(&flows);
        let source = g.nodes.iter().find(|n| n.side == NodeSide::Source).unwrap();
        assert_eq!(source.id, "run-x");
    }

    #[test]
    fn read_flows_and_dropped_matches_the_two_single_purpose_readers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");
        write_lines(
            &path,
            &[
                LedgerLine::Flow(Box::new(flow(1, "api.anthropic.com", Some("r1"), 10, true))),
                LedgerLine::Dropped {
                    count: 5,
                    ts: chrono::Utc::now(),
                },
            ],
        );

        let (flows, dropped) = read_flows_and_dropped(&path).unwrap();
        assert_eq!(flows.len(), read_flows(&path).unwrap().len());
        assert_eq!(dropped, read_dropped_total(&path).unwrap());
        assert_eq!(dropped, 5);
    }

    #[test]
    fn read_flows_and_dropped_on_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (flows, dropped) = read_flows_and_dropped(&tmp.path().join("absent.jsonl")).unwrap();
        assert!(flows.is_empty());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn read_dropped_total_sums_dropped_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("flows.jsonl");
        write_lines(
            &path,
            &[
                LedgerLine::Dropped {
                    count: 3,
                    ts: chrono::Utc::now(),
                },
                LedgerLine::Dropped {
                    count: 4,
                    ts: chrono::Utc::now(),
                },
            ],
        );
        assert_eq!(read_dropped_total(&path).unwrap(), 7);
    }

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn flow_at(id: u64, ts: chrono::DateTime<chrono::Utc>) -> FlowRecord {
        let mut f = flow(id, "example.test", Some("r1"), 5, true);
        f.ts = ts;
        f
    }

    #[test]
    fn an_unbounded_range_contains_everything() {
        let r = TimeRange::unbounded();
        assert!(r.contains(at(0)));
        assert!(r.contains(at(1_900_000_000)));
    }

    #[test]
    fn bounds_are_inclusive_at_both_ends() {
        let r = TimeRange {
            from: Some(at(100)),
            to: Some(at(200)),
        };
        assert!(r.contains(at(100)), "from is inclusive");
        assert!(r.contains(at(200)), "to is inclusive");
        assert!(!r.contains(at(99)));
        assert!(!r.contains(at(201)));
    }

    #[test]
    fn a_half_open_range_bounds_only_the_side_it_names() {
        let from_only = TimeRange {
            from: Some(at(100)),
            to: None,
        };
        assert!(!from_only.contains(at(99)));
        assert!(from_only.contains(at(1_900_000_000)));

        let to_only = TimeRange {
            from: None,
            to: Some(at(100)),
        };
        assert!(to_only.contains(at(0)));
        assert!(!to_only.contains(at(101)));
    }

    #[test]
    fn an_inverted_range_contains_nothing() {
        // `from > to` is not an error to construct — it is simply an
        // empty window: no timestamp can be both `>= from` and `<= to`
        // when `from` is after `to`. Callers that build a range from
        // untrusted input (Task 2's CLI, Task 3's API) get an empty
        // result rather than a panic or a silently-reordered range.
        let inverted = TimeRange {
            from: Some(at(200)),
            to: Some(at(100)),
        };
        assert!(!inverted.contains(at(100)));
        assert!(!inverted.contains(at(150)));
        assert!(!inverted.contains(at(200)));
    }

    #[test]
    fn read_flows_in_range_filters_rows_and_keeps_the_dropped_count() {
        // The dropped count describes the whole file, not the window: a
        // record lost to overflow has no timestamp to filter on, and
        // hiding it because of a time filter would be exactly the silent
        // under-reporting this subsystem exists to prevent.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("run-a.jsonl");
        write_lines(
            &path,
            &[
                LedgerLine::Flow(Box::new(flow_at(1, at(100)))),
                LedgerLine::Flow(Box::new(flow_at(2, at(150)))),
                LedgerLine::Flow(Box::new(flow_at(3, at(250)))),
                LedgerLine::Dropped {
                    count: 4,
                    ts: at(150),
                },
            ],
        );

        let (flows, dropped) = read_flows_in_range(
            &path,
            &TimeRange {
                from: Some(at(120)),
                to: Some(at(200)),
            },
        )
        .unwrap();

        assert_eq!(flows.len(), 1, "only the t=150 flow is in range");
        assert_eq!(dropped, 4, "loss is reported regardless of the window");
    }

    #[test]
    fn read_flows_in_range_on_a_missing_file_is_empty_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (flows, dropped) =
            read_flows_in_range(&tmp.path().join("absent.jsonl"), &TimeRange::unbounded()).unwrap();
        assert!(flows.is_empty());
        assert_eq!(dropped, 0);
    }
}
