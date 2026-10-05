//! The canonical flow record and the ledger line envelope.

use crate::ctx::FlowCtx;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

pub use ulid::Ulid as FlowId;

/// How much of this record is actually known, by capture backend.
///
/// Rendered as a badge in every CP view. The subsystem never claims
/// coverage it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// Host, outcome and timing only — recorded at a connector boundary
    /// whose HTTP stack we do not own (`octocrab`).
    Coarse,
    /// Connection-level: process, remote IP:port, bytes and timing
    /// observed from the OS socket table (spec 2026-10-04). No URL,
    /// method or HTTP status. More observable than `Coarse`, less than
    /// `Http`.
    Socket,
    /// Exact request/response metadata from the instrumented client.
    Http,
    /// Frame-level capture from the microVM backend. Not emitted yet.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    HttpError,
    TransportError,
    Timeout,
}

/// The process that owned a captured socket (socket flows only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowProcess {
    pub pid: u32,
    pub name: String,
}

/// Connection direction for socket flows. `None` on a record means
/// unknown — e.g. Linux v1 does not populate it (spec 2026-10-04 18 V1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outbound,
    Inbound,
}

/// One outbound request.
///
/// Deliberate omissions (spec §5.1): no query string, no headers, no TLS
/// version, no ASN. Query strings routinely carry tokens; ASN is resolved
/// at read time so a late-arriving dataset improves history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRecord {
    pub id: FlowId,
    pub ts: DateTime<Utc>,
    pub ctx: FlowCtx,
    pub fidelity: Fidelity,

    pub method: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Query-stripped. Never contains `?`.
    pub path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_ip: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_ips: Vec<IpAddr>,
    /// The process that owned the socket (socket flows only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<FlowProcess>,
    /// `"ip:port"` of the local end, for correlating with other logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
    /// DECLARED, not confirmed-drained, when no explicit `complete()`
    /// follows: the middleware records at response-header time and takes
    /// this from `Content-Length`. If the connection drops mid-body the
    /// record still shows the declared count. A value finalized by a
    /// `LedgerLine::Complete` (i.e. `body_complete == true` via the fold)
    /// IS an observed count. Do not present the two as equally exact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    /// `false` while a streamed body is still draining. Flipped by a
    /// later `LedgerLine::Complete` folded in at read time.
    pub body_complete: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Finalizes a socket flow at close: byte counts, final outcome and an
/// optional note. Any `Some` field overwrites the flow when folded;
/// `None` leaves it as written. `duration_ms` always applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketCompletion {
    pub id: FlowId,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the subprocess-capture backend could do for a run. Written as a
/// `LedgerLine::Capture` so the CP can surface "capture unavailable:
/// <reason>" honestly (spec 2026-10-04 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CaptureState {
    Active { backend: String },
    Unavailable { reason: String },
}

/// One line of the append-only ledger.
///
/// `Flow` is boxed because it dwarfs the other variants — clippy's
/// `large_enum_variant` denies otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LedgerLine {
    Flow(Box<FlowRecord>),
    /// Finalizes a streamed body written earlier at header time.
    Complete {
        id: FlowId,
        bytes_in: u64,
        duration_ms: u64,
    },
    /// Records visible loss when the writer channel overflowed.
    Dropped {
        count: u64,
        ts: DateTime<Utc>,
    },
    /// Finalizes a socket flow at close. Separate from `Complete` (the
    /// HTTP streamed-body finalizer) so the HTTP path is untouched. Any
    /// `Some` field overwrites the flow; `None` leaves it as written.
    SocketComplete(SocketCompletion),
    /// Capture availability for a run, plus any visible-loss note.
    Capture {
        ts: DateTime<Utc>,
        state: CaptureState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Origin;
    use chrono::TimeZone;

    fn sample() -> FlowRecord {
        FlowRecord {
            id: FlowId::from_parts(1, 2),
            ts: chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            ctx: FlowCtx {
                run_id: Some("run-1".into()),
                step_id: Some("step-1".into()),
                agent: Some("reviewer".into()),
                workspace_id: Some("ws-1".into()),
                tool_call_id: None,
                origin: Origin::Provider("anthropic".into()),
            },
            fidelity: Fidelity::Http,
            method: "POST".into(),
            scheme: "https".into(),
            host: "api.anthropic.com".into(),
            port: 443,
            path: "/v1/messages".into(),
            peer_ip: Some("160.79.104.10".parse().unwrap()),
            resolved_ips: vec!["160.79.104.10".parse().unwrap()],
            http_version: Some("HTTP/1.1".into()),
            status: Some(200),
            outcome: Outcome::Ok,
            error: None,
            bytes_out: Some(1234),
            bytes_in: None,
            body_complete: false,
            process: None,
            local_addr: None,
            direction: None,
            ttfb_ms: Some(42),
            duration_ms: None,
        }
    }

    #[test]
    fn flow_record_round_trips_through_json() {
        let r = sample();
        let json = serde_json::to_string(&r).unwrap();
        let back: FlowRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn origin_is_externally_tagged_with_kind_and_name() {
        let json = serde_json::to_value(Origin::Scm("github".into())).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "scm", "name": "github"}));

        let json = serde_json::to_value(Origin::Update).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "update"}));
    }

    #[test]
    fn ledger_line_round_trips_all_variants() {
        let lines = vec![
            LedgerLine::Flow(Box::new(sample())),
            LedgerLine::Complete {
                id: FlowId::from_parts(1, 2),
                bytes_in: 9999,
                duration_ms: 1500,
            },
            LedgerLine::Dropped {
                count: 7,
                ts: chrono::Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
            },
        ];
        for line in lines {
            let json = serde_json::to_string(&line).unwrap();
            let back: LedgerLine = serde_json::from_str(&json).unwrap();
            assert_eq!(line, back);
        }
    }
}
