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

/// Read every flow, folding `Complete` lines into their flow.
///
/// A missing file is an empty ledger, not an error. Malformed lines are
/// skipped — a torn write at the tail must not lose the whole history.
pub fn read_flows(path: &Path) -> std::io::Result<Vec<FlowRecord>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut flows: Vec<FlowRecord> = Vec::new();
    let mut index: HashMap<FlowId, usize> = HashMap::new();

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
            LedgerLine::Dropped { .. } => {}
        }
    }

    Ok(flows)
}

/// Total records lost to writer-channel overflow.
pub fn read_dropped_total(path: &Path) -> std::io::Result<u64> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };

    let mut total = 0u64;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Ok(LedgerLine::Dropped { count, .. }) = serde_json::from_str::<LedgerLine>(&line) {
            total += count;
        }
    }
    Ok(total)
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
    pub p50_ms: u64,
    pub p95_ms: u64,
}

fn is_error(f: &FlowRecord) -> bool {
    !matches!(f.outcome, Outcome::Ok)
}

/// Nearest-rank percentile — the standard definition: the smallest value
/// at or below which at least `pct` of the samples fall, i.e. index
/// `ceil(pct * n) - 1`. `pct` in 0.0..=1.0. `sorted` must be ascending.
///
/// Do NOT "fix" this to `ceil(pct * (n - 1))` to make a test pass. That
/// is a different, nonstandard definition; if a test disagrees with
/// nearest-rank, the test's expected value is what is wrong.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
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

/// Bipartite topology: sources (runs, or `system` for unattributed
/// process-global egress) on one side, `host:port` endpoints on the other.
pub fn graph_view(flows: &[FlowRecord]) -> GraphView {
    let mut nodes: HashMap<String, GraphNode> = HashMap::new();
    let mut edges: HashMap<(String, String), GraphEdge> = HashMap::new();

    for f in flows {
        let source_id = f.ctx.run_id.clone().unwrap_or_else(|| "system".to_string());
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
        assert_eq!(rollup[0].p50_ms, 20);
        assert_eq!(rollup[0].p95_ms, 100);
        assert_eq!(rollup[1].host, "api.github.com");
        assert_eq!(rollup[1].calls, 1);
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
        assert_eq!(rollup[0].p50_ms, 10, "timings are still exact");
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
            flow(1, "api.anthropic.com", Some("r1"), 10, true),
            flow(2, "api.anthropic.com", Some("r1"), 20, false),
            flow(3, "api.github.com", Some("r2"), 30, true),
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
    fn unattributed_flows_group_under_a_system_source_node() {
        let flows = vec![flow(1, "api.github.com", None, 10, true)];
        let g = graph_view(&flows);
        let source = g.nodes.iter().find(|n| n.side == NodeSide::Source).unwrap();
        assert_eq!(source.id, "system");
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
}
