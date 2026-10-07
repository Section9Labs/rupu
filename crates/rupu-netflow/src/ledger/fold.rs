//! The resumable ledger fold — the ONE place ledger lines become flows.
//!
//! Every reader feeds lines through [`LedgerFold`]: the whole-file readers
//! in [`super::views`] and the CP's netflow index, which tails a growing
//! ledger in appended chunks. Sharing the fold is what guarantees a file
//! read in pieces folds to exactly what a file read in one pass does.

use super::views::CaptureEntry;
use crate::record::{FlowId, FlowRecord, LedgerLine, Outcome};
use std::collections::HashMap;

/// The fields a `complete` / `socket_complete` line rewrites on an earlier
/// flow. Applying a patch always marks the flow's body complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowPatch {
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl FlowPatch {
    pub fn apply(&self, f: &mut FlowRecord) {
        if let Some(b) = self.bytes_in {
            f.bytes_in = Some(b);
        }
        if let Some(b) = self.bytes_out {
            f.bytes_out = Some(b);
        }
        if let Some(o) = self.outcome {
            f.outcome = o;
        }
        if let Some(e) = &self.error {
            f.error = Some(e.clone());
        }
        f.duration_ms = Some(self.duration_ms);
        f.body_complete = true;
    }
}

/// What one ledger line contributes. `Patch::index` is the position of the
/// patched flow among this fold's `Flow` events, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum FoldEvent {
    Flow(Box<FlowRecord>),
    Patch { index: usize, patch: FlowPatch },
    Dropped(u64),
    Capture(CaptureEntry),
}

/// Fold state: which flow id sits at which position, so a later completion
/// line can find the flow it finishes. Blank and malformed lines, and
/// completions for an id this fold never saw, contribute nothing.
#[derive(Debug, Clone, Default)]
pub struct LedgerFold {
    index: HashMap<FlowId, usize>,
    len: usize,
}

impl LedgerFold {
    pub fn feed_line(&mut self, line: &str) -> Option<FoldEvent> {
        if line.trim().is_empty() {
            return None;
        }
        match serde_json::from_str::<LedgerLine>(line).ok()? {
            LedgerLine::Flow(f) => {
                self.index.insert(f.id, self.len);
                self.len += 1;
                Some(FoldEvent::Flow(f))
            }
            LedgerLine::Complete {
                id,
                bytes_in,
                duration_ms,
            } => Some(FoldEvent::Patch {
                index: *self.index.get(&id)?,
                patch: FlowPatch {
                    bytes_in: Some(bytes_in),
                    bytes_out: None,
                    outcome: None,
                    error: None,
                    duration_ms,
                },
            }),
            LedgerLine::SocketComplete(c) => Some(FoldEvent::Patch {
                index: *self.index.get(&c.id)?,
                patch: FlowPatch {
                    bytes_in: c.bytes_in,
                    bytes_out: c.bytes_out,
                    outcome: c.outcome,
                    error: c.error,
                    duration_ms: c.duration_ms,
                },
            }),
            LedgerLine::Dropped { count, .. } => Some(FoldEvent::Dropped(count)),
            LedgerLine::Capture {
                ts,
                state,
                tool_call_id,
                note,
            } => Some(FoldEvent::Capture(CaptureEntry {
                ts,
                state,
                tool_call_id,
                note,
            })),
        }
    }

    /// Flows seen so far.
    pub fn flow_count(&self) -> usize {
        self.len
    }

    /// Approximate heap held by the id map (for the CP index's budget).
    pub fn heap_bytes(&self) -> usize {
        self.index.capacity() * (std::mem::size_of::<FlowId>() + std::mem::size_of::<usize>() + 8)
    }
}

/// Split `buf` after its last `\n`: the complete-lines prefix and its byte
/// length. An unterminated tail is left for the next read.
pub fn split_complete_lines(buf: &[u8]) -> (&[u8], usize) {
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => (&buf[..=i], i + 1),
        None => (&buf[..0], 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::views::read_flows_and_dropped;
    use crate::record::{Fidelity, FlowRecord, LedgerLine, Outcome, SocketCompletion};
    use crate::{FlowCtx, Origin};

    fn flow(n: u64, secs: i64) -> FlowRecord {
        FlowRecord {
            id: crate::FlowId::from_parts(n, u128::from(n)),
            ts: chrono::DateTime::from_timestamp(secs, 0).unwrap(),
            ctx: FlowCtx::system(Origin::Provider("anthropic".into())),
            fidelity: Fidelity::Socket,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: None,
            resolved_ips: vec![],
            process: None,
            local_addr: None,
            direction: None,
            http_version: None,
            status: None,
            outcome: Outcome::Ok,
            error: None,
            bytes_out: None,
            bytes_in: None,
            body_complete: false,
            ttfb_ms: None,
            duration_ms: None,
        }
    }

    /// A ledger exercising every line kind, including completions that
    /// rewrite an earlier flow's outcome.
    fn ledger_text() -> String {
        let lines = vec![
            LedgerLine::Flow(Box::new(flow(1, 100))),
            LedgerLine::Flow(Box::new(flow(2, 200))),
            LedgerLine::Complete {
                id: crate::FlowId::from_parts(1, 1),
                bytes_in: 7,
                duration_ms: 9,
            },
            LedgerLine::Dropped {
                count: 3,
                ts: chrono::DateTime::from_timestamp(250, 0).unwrap(),
            },
            LedgerLine::SocketComplete(SocketCompletion {
                id: crate::FlowId::from_parts(2, 2),
                duration_ms: 11,
                bytes_in: Some(5),
                bytes_out: Some(6),
                outcome: Some(Outcome::TransportError),
                error: Some("reset".into()),
            }),
            LedgerLine::Capture {
                ts: chrono::DateTime::from_timestamp(300, 0).unwrap(),
                state: crate::record::CaptureState::Active {
                    backend: "ntstat".into(),
                },
                tool_call_id: None,
                note: None,
            },
        ];
        let mut s = String::new();
        for l in lines {
            s.push_str(&serde_json::to_string(&l).unwrap());
            s.push('\n');
        }
        s.push_str("{not json}\n\n");
        s
    }

    /// Fold `text` fed in pieces split at `cuts` (byte offsets), the way
    /// the CP index tails a growing file.
    fn fold_chunked(text: &str, cuts: &[usize]) -> (Vec<FlowRecord>, u64, usize) {
        let bytes = text.as_bytes();
        let mut fold = LedgerFold::default();
        let mut flows: Vec<FlowRecord> = Vec::new();
        let mut dropped = 0u64;
        let mut captures = 0usize;
        let mut offset = 0usize;
        let mut ends: Vec<usize> = cuts.to_vec();
        ends.push(bytes.len());
        for end in ends {
            let (complete, used) = split_complete_lines(&bytes[offset..end]);
            for line in std::str::from_utf8(complete).unwrap().lines() {
                match fold.feed_line(line) {
                    Some(FoldEvent::Flow(f)) => flows.push(*f),
                    Some(FoldEvent::Patch { index, patch }) => patch.apply(&mut flows[index]),
                    Some(FoldEvent::Dropped(n)) => dropped += n,
                    Some(FoldEvent::Capture(_)) => captures += 1,
                    None => {}
                }
            }
            offset += used;
        }
        (flows, dropped, captures)
    }

    #[test]
    fn chunked_fold_matches_the_whole_file_reader_at_every_split_point() {
        let text = ledger_text();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("run_x.jsonl");
        std::fs::write(&path, &text).unwrap();
        let (whole_flows, whole_dropped) = read_flows_and_dropped(&path).unwrap();
        for cut in 0..=text.len() {
            let (flows, dropped, captures) = fold_chunked(&text, &[cut]);
            assert_eq!(flows, whole_flows, "split at byte {cut}");
            assert_eq!(dropped, whole_dropped, "split at byte {cut}");
            assert_eq!(captures, 1, "split at byte {cut}");
        }
    }

    #[test]
    fn a_socket_completion_rewrites_the_earlier_flows_outcome() {
        let (flows, dropped, _) = fold_chunked(&ledger_text(), &[]);
        assert_eq!(dropped, 3);
        assert_eq!(flows[0].bytes_in, Some(7));
        assert!(flows[0].body_complete);
        assert_eq!(flows[1].outcome, Outcome::TransportError);
        assert_eq!(flows[1].error.as_deref(), Some("reset"));
        assert_eq!(flows[1].bytes_out, Some(6));
        assert_eq!(flows[1].duration_ms, Some(11));
    }

    #[test]
    fn split_complete_lines_holds_back_an_unterminated_tail() {
        assert_eq!(split_complete_lines(b"a\nb\nc"), (&b"a\nb\n"[..], 4));
        assert_eq!(split_complete_lines(b"abc"), (&b""[..], 0));
        assert_eq!(split_complete_lines(b""), (&b""[..], 0));
    }
}
