//! Compact in-memory flow rows for the CP's netflow index.
//!
//! A ledger repeats the same few hosts, paths, methods and context strings
//! across thousands of flows, so each [`CompactRows`] keeps its own string
//! table and stores rows as symbols. The table is per file: dropping a
//! file's rows frees all of its memory. Conversion is exhaustive — the
//! destructuring below names every field with no `..`, so adding a field to
//! [`FlowRecord`], [`FlowCtx`] or [`FlowProcess`] fails to compile here
//! until the compact form carries it.

use super::fold::FlowPatch;
use super::views::TimeRange;
use crate::ctx::{FlowCtx, Origin};
use crate::record::{Direction, Fidelity, FlowId, FlowProcess, FlowRecord, Outcome};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

type Sym = u32;
const NONE: Sym = u32::MAX;

#[derive(Debug, Clone, Default)]
struct Interner {
    map: HashMap<Arc<str>, Sym>,
    strs: Vec<Arc<str>>,
    text_bytes: usize,
}

impl Interner {
    fn intern(&mut self, s: &str) -> Sym {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let a: Arc<str> = Arc::from(s);
        let i = self.strs.len() as Sym;
        self.strs.push(Arc::clone(&a));
        self.map.insert(a, i);
        self.text_bytes += s.len();
        i
    }

    fn opt(&mut self, s: Option<&str>) -> Sym {
        s.map_or(NONE, |s| self.intern(s))
    }

    fn get(&self, i: Sym) -> String {
        self.strs[i as usize].to_string()
    }

    fn get_opt(&self, i: Sym) -> Option<String> {
        (i != NONE).then(|| self.get(i))
    }

    fn heap_bytes(&self) -> usize {
        // Text once, plus per-entry Arc headers, the Vec slot and the map slot.
        self.text_bytes + self.strs.capacity() * 48
    }
}

#[derive(Debug, Clone, Copy)]
enum COrigin {
    Provider(Sym),
    Scm(Sym),
    Subprocess(Sym),
    Update,
    Cp,
    System,
}

#[derive(Debug, Clone)]
struct CompactFlow {
    id: FlowId,
    ts: DateTime<Utc>,
    run_id: Sym,
    step_id: Sym,
    agent: Sym,
    workspace_id: Sym,
    tool_call_id: Sym,
    origin: COrigin,
    fidelity: Fidelity,
    method: Sym,
    scheme: Sym,
    host: Sym,
    port: u16,
    path: Sym,
    peer_ip: Option<IpAddr>,
    resolved_ips: Box<[IpAddr]>,
    process: Option<(u32, Sym)>,
    local_addr: Sym,
    direction: Option<Direction>,
    http_version: Sym,
    status: Option<u16>,
    outcome: Outcome,
    error: Sym,
    bytes_out: Option<u64>,
    bytes_in: Option<u64>,
    body_complete: bool,
    ttfb_ms: Option<u64>,
    duration_ms: Option<u64>,
}

/// One ledger file's flows, in file order, with a per-file string table.
#[derive(Debug, Clone, Default)]
pub struct CompactRows {
    strings: Interner,
    rows: Vec<CompactFlow>,
    ip_bytes: usize,
}

impl CompactRows {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn push(&mut self, f: FlowRecord) {
        let row = self.compact(f);
        self.rows.push(row);
    }

    /// Apply a completion to row `index` through [`FlowPatch::apply`] itself,
    /// so the compact path can never drift from the whole-file reader.
    pub fn patch(&mut self, index: usize, p: &FlowPatch) {
        let mut f = self.record(index);
        p.apply(&mut f);
        self.ip_bytes -= self.rows[index].resolved_ips.len() * std::mem::size_of::<IpAddr>();
        self.rows[index] = self.compact(f);
    }

    pub fn record(&self, index: usize) -> FlowRecord {
        let r = &self.rows[index];
        let s = &self.strings;
        FlowRecord {
            id: r.id,
            ts: r.ts,
            ctx: FlowCtx {
                run_id: s.get_opt(r.run_id),
                step_id: s.get_opt(r.step_id),
                agent: s.get_opt(r.agent),
                workspace_id: s.get_opt(r.workspace_id),
                tool_call_id: s.get_opt(r.tool_call_id),
                origin: match r.origin {
                    COrigin::Provider(x) => Origin::Provider(s.get(x)),
                    COrigin::Scm(x) => Origin::Scm(s.get(x)),
                    COrigin::Subprocess(x) => Origin::Subprocess(s.get(x)),
                    COrigin::Update => Origin::Update,
                    COrigin::Cp => Origin::Cp,
                    COrigin::System => Origin::System,
                },
            },
            fidelity: r.fidelity,
            method: s.get(r.method),
            scheme: s.get(r.scheme),
            host: s.get(r.host),
            port: r.port,
            path: s.get(r.path),
            peer_ip: r.peer_ip,
            resolved_ips: r.resolved_ips.to_vec(),
            process: r.process.map(|(pid, name)| FlowProcess {
                pid,
                name: s.get(name),
            }),
            local_addr: s.get_opt(r.local_addr),
            direction: r.direction,
            http_version: s.get_opt(r.http_version),
            status: r.status,
            outcome: r.outcome,
            error: s.get_opt(r.error),
            bytes_out: r.bytes_out,
            bytes_in: r.bytes_in,
            body_complete: r.body_complete,
            ttfb_ms: r.ttfb_ms,
            duration_ms: r.duration_ms,
        }
    }

    /// Rows whose `ts` falls in `range` (same inclusive semantics as
    /// [`TimeRange::contains`]), materialized, in file order.
    pub fn records_in_range<'a>(
        &'a self,
        range: &'a TimeRange,
    ) -> impl Iterator<Item = FlowRecord> + 'a {
        (0..self.rows.len())
            .filter(move |&i| range.contains(self.rows[i].ts))
            .map(move |i| self.record(i))
    }

    /// Approximate heap held (rows + resolved-IP lists + string table).
    pub fn heap_bytes(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<CompactFlow>()
            + self.ip_bytes
            + self.strings.heap_bytes()
    }

    fn compact(&mut self, f: FlowRecord) -> CompactFlow {
        let FlowRecord {
            id,
            ts,
            ctx,
            fidelity,
            method,
            scheme,
            host,
            port,
            path,
            peer_ip,
            resolved_ips,
            process,
            local_addr,
            direction,
            http_version,
            status,
            outcome,
            error,
            bytes_out,
            bytes_in,
            body_complete,
            ttfb_ms,
            duration_ms,
        } = f;
        let FlowCtx {
            run_id,
            step_id,
            agent,
            workspace_id,
            tool_call_id,
            origin,
        } = ctx;
        let st = &mut self.strings;
        let origin = match origin {
            Origin::Provider(x) => COrigin::Provider(st.intern(&x)),
            Origin::Scm(x) => COrigin::Scm(st.intern(&x)),
            Origin::Subprocess(x) => COrigin::Subprocess(st.intern(&x)),
            Origin::Update => COrigin::Update,
            Origin::Cp => COrigin::Cp,
            Origin::System => COrigin::System,
        };
        self.ip_bytes += resolved_ips.len() * std::mem::size_of::<IpAddr>();
        CompactFlow {
            id,
            ts,
            run_id: st.opt(run_id.as_deref()),
            step_id: st.opt(step_id.as_deref()),
            agent: st.opt(agent.as_deref()),
            workspace_id: st.opt(workspace_id.as_deref()),
            tool_call_id: st.opt(tool_call_id.as_deref()),
            origin,
            fidelity,
            method: st.intern(&method),
            scheme: st.intern(&scheme),
            host: st.intern(&host),
            port,
            path: st.intern(&path),
            peer_ip,
            resolved_ips: resolved_ips.into_boxed_slice(),
            process: process.map(|FlowProcess { pid, name }| (pid, st.intern(&name))),
            local_addr: st.opt(local_addr.as_deref()),
            direction,
            http_version: st.opt(http_version.as_deref()),
            status,
            outcome,
            error: st.opt(error.as_deref()),
            bytes_out,
            bytes_in,
            body_complete,
            ttfb_ms,
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::views::TimeRange;
    use crate::record::{Direction, Fidelity, FlowProcess, Outcome};

    fn full(n: u64, origin: Origin) -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(n, u128::from(n)),
            ts: chrono::DateTime::from_timestamp(1_700_000_000 + n as i64, 5).unwrap(),
            ctx: FlowCtx {
                run_id: Some("run-1".into()),
                step_id: Some("step-1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws-1".into()),
                tool_call_id: Some("tc-1".into()),
                origin,
            },
            fidelity: Fidelity::Full,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages?x=1".into(),
            peer_ip: Some("160.79.104.10".parse().unwrap()),
            resolved_ips: vec!["160.79.104.10".parse().unwrap(), "::1".parse().unwrap()],
            process: Some(FlowProcess {
                pid: 42,
                name: "curl".into(),
            }),
            local_addr: Some("10.0.0.2:51515".into()),
            direction: Some(Direction::Outbound),
            http_version: Some("HTTP/2".into()),
            status: Some(529),
            outcome: Outcome::HttpError,
            error: Some("overloaded".into()),
            bytes_out: Some(100),
            bytes_in: Some(200),
            body_complete: true,
            ttfb_ms: Some(30),
            duration_ms: Some(40),
        }
    }

    fn origins() -> Vec<Origin> {
        vec![
            Origin::Provider("anthropic".into()),
            Origin::Scm("github".into()),
            Origin::Subprocess("bash".into()),
            Origin::Update,
            Origin::Cp,
            Origin::System,
        ]
    }

    #[test]
    fn every_field_round_trips_for_every_origin() {
        let mut rows = CompactRows::default();
        let input: Vec<FlowRecord> = origins()
            .into_iter()
            .enumerate()
            .map(|(i, o)| full(i as u64, o))
            .collect();
        for f in &input {
            rows.push(f.clone());
        }
        let back: Vec<FlowRecord> = (0..rows.len()).map(|i| rows.record(i)).collect();
        assert_eq!(back, input);
    }

    #[test]
    fn empty_optionals_round_trip_as_none() {
        let mut f = full(9, Origin::System);
        f.ctx.run_id = None;
        f.ctx.tool_call_id = None;
        f.peer_ip = None;
        f.resolved_ips = vec![];
        f.process = None;
        f.local_addr = None;
        f.http_version = None;
        f.error = None;
        let mut rows = CompactRows::default();
        rows.push(f.clone());
        assert_eq!(rows.record(0), f);
    }

    #[test]
    fn a_patch_matches_flow_patch_apply() {
        let mut f = full(1, Origin::System);
        f.outcome = Outcome::Ok;
        f.error = None;
        let patch = crate::ledger::fold::FlowPatch {
            bytes_in: Some(9),
            bytes_out: None,
            outcome: Some(Outcome::Timeout),
            error: Some("slow".into()),
            duration_ms: 77,
        };
        let mut rows = CompactRows::default();
        rows.push(f.clone());
        rows.patch(0, &patch);
        patch.apply(&mut f);
        assert_eq!(rows.record(0), f);
    }

    #[test]
    fn records_in_range_filters_inclusively_and_keeps_file_order() {
        let mut rows = CompactRows::default();
        for n in 0..5 {
            rows.push(full(n, Origin::System));
        }
        let lo = rows.record(1).ts;
        let hi = rows.record(3).ts;
        let range = TimeRange {
            from: Some(lo),
            to: Some(hi),
        };
        let ids: Vec<FlowId> = rows.records_in_range(&range).map(|f| f.id).collect();
        assert_eq!(
            ids,
            vec![
                FlowId::from_parts(1, 1),
                FlowId::from_parts(2, 2),
                FlowId::from_parts(3, 3)
            ]
        );
    }

    #[test]
    fn repeated_strings_are_stored_once() {
        let mut rows = CompactRows::default();
        rows.push(full(1, Origin::System));
        let one = rows.heap_bytes();
        for n in 2..1000 {
            rows.push(full(n, Origin::System));
        }
        let per_row = (rows.heap_bytes() - one) / 998;
        assert!(
            per_row < 400,
            "per-row bytes {per_row} — strings are not being interned"
        );
    }
}
