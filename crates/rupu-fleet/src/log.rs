//! The canonical message log of a space (spec F1a §4): `log.jsonl`,
//! append-only and never drained. Every post, direct message, broadcast,
//! directive and retraction is a `msg` line, written before the working
//! state it feeds; every delivery to a model is a `seen` line.

use crate::participants::Undeliverable;
use crate::types::PostKind;
use crate::FleetError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// A board post. `to: null` is a channel post, not a broadcast.
    Post,
    /// To a participant, a role, a step, the parent, the lead or the
    /// operator.
    Direct,
    /// To everyone, through the broadcast log.
    Broadcast,
    /// A standing directive.
    Directive,
    /// The retraction of a directive (`refs.retracts`).
    Retract,
    /// From the operator (F1b).
    Operator,
}

impl MessageKind {
    pub const ALL: [MessageKind; 6] = [
        MessageKind::Post,
        MessageKind::Direct,
        MessageKind::Broadcast,
        MessageKind::Directive,
        MessageKind::Retract,
        MessageKind::Operator,
    ];

    /// The kind [`MessageKind::as_str`] names.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Post => "post",
            MessageKind::Direct => "direct",
            MessageKind::Broadcast => "broadcast",
            MessageKind::Directive => "directive",
            MessageKind::Retract => "retract",
            MessageKind::Operator => "operator",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl Refs {
    fn is_empty(&self) -> bool {
        self.retracts.is_none() && self.reply_to.is_none()
    }
}

/// An intended recipient a message did not reach, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undelivered {
    pub to: String,
    pub reason: Undeliverable,
}

/// Where a message went, decided when it was sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub resolved: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undelivered: Vec<Undelivered>,
}

/// One message (`{"t":"msg", …}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogMessage {
    /// `msg_<ULID>`: the anchor transcripts and the UI refer to.
    pub id: String,
    pub ts: String,
    pub kind: MessageKind,
    /// The sender's address, set from the sender's bus, never from tool
    /// input.
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_codename: Option<String>,
    /// `None` only for an unaddressed board post (and a retraction).
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_kind: Option<PostKind>,
    pub body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub urgent: bool,
    #[serde(default, skip_serializing_if = "Refs::is_empty")]
    pub refs: Refs,
    pub delivery: Delivery,
    /// Where an operator message was sent from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<OperatorVia>,
    /// Who sent an operator message, when the sending surface knows (the
    /// CP's viewer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<String>,
}

/// The surface an operator message came through (spec F1b §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorVia {
    Cli,
    Cp,
}

/// One line of `log.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum LogLine {
    Msg(LogMessage),
    /// A message reached a model: `by` saw `id` on its turn `turn`.
    Seen {
        id: String,
        by: String,
        turn: u32,
        at: String,
    },
}

/// A page of the log, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    /// The id to pass as `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

/// What a write returns: the message's id and where it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReceipt {
    pub message: LogMessage,
}

impl SendReceipt {
    pub fn id(&self) -> &str {
        &self.message.id
    }

    pub fn delivery(&self) -> &Delivery {
        &self.message.delivery
    }
}

/// Every complete line of the log, oldest first. A missing log is empty; an
/// unreadable line is skipped and logged.
pub(crate) fn read_lines(path: &Path) -> Result<Vec<LogLine>, FleetError> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(FleetError::Io {
                action: format!("read {}", path.display()),
                source: e,
            })
        }
    };
    let complete = match text.rfind('\n') {
        Some(i) => &text[..=i],
        None => return Ok(Vec::new()),
    };
    Ok(complete
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .filter_map(|(n, l)| match serde_json::from_str(l) {
            Ok(line) => Some(line),
            Err(e) => {
                tracing::warn!(path = %path.display(), line = n + 1, error = %e, "skipping an unreadable message log line");
                None
            }
        })
        .collect())
}

/// A page of `lines` after the message `after` (from the start when `None`,
/// or when `after` isn't in the log).
///
/// The log is cut into pages before a `msg` line only: a page holds about
/// `limit` lines plus the receipts that follow its last message, and the next
/// page starts at the message after them. So every line is on exactly one
/// page and a page is never only receipts.
pub(crate) fn page(lines: Vec<LogLine>, after: Option<&str>, limit: usize) -> LogPage {
    let is_msg = |l: &LogLine| matches!(l, LogLine::Msg(_));
    let start = match after.and_then(|a| {
        lines
            .iter()
            .position(|l| matches!(l, LogLine::Msg(m) if m.id == a))
    }) {
        Some(i) => lines[i + 1..]
            .iter()
            .position(is_msg)
            .map_or(lines.len(), |j| i + 1 + j),
        None => 0,
    };
    let rest = &lines[start..];
    let mut end = rest.len().min(limit.max(1));
    while end < rest.len() && !is_msg(&rest[end]) {
        end += 1;
    }
    let page: Vec<LogLine> = rest[..end].to_vec();
    let next = (end < rest.len())
        .then(|| {
            page.iter().rev().find_map(|l| match l {
                LogLine::Msg(m) => Some(m.id.clone()),
                LogLine::Seen { .. } => None,
            })
        })
        .flatten();
    LogPage { lines: page, next }
}
