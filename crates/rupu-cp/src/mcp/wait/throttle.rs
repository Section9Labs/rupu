//! Spacing across calls for a host whose every request is a connection of
//! its own (SSH, [`HostConnector::connects_per_request`]). Within one call
//! such a host is polled every [`SSH_POLL`]; this keeps that spacing between
//! calls too, so an agent looping `wait` (a `timeout_ms: 0` peek, or a call
//! that fires on every new event of the run's stream) doesn't start an `ssh`
//! process per call.
//!
//! One [`Throttle`] is shared by every call on the CP. It remembers, per host
//! and gate run or message space, when it was last asked and the answer to
//! that ask. An ask within [`SSH_POLL`] of the last one doesn't go to the
//! host: it takes that answer when the answer can say nothing false, else it
//! waits until the spacing allows and asks then ([`Claim::Wait`]; the caller
//! does the waiting, bounded by its call).
//!
//! - A gate answer is reused only when it says no gate is parked. A parked
//!   gate may have been decided since, so it is asked again, never replayed.
//! - A message page is reused only from the call's cursor on: the whole page
//!   when the cursor is the one it was read from, its tail when the cursor is
//!   one of its messages. A cursor it doesn't cover is asked again.
//! - A failed ask, or one still out, leaves no answer to reuse.
//!
//! So a reused answer can only be up to [`SSH_POLL`] late, as a poll within
//! a call is; it never fires a condition a real ask wouldn't.
//!
//! [`HostConnector::connects_per_request`]: crate::host::connector::HostConnector::connects_per_request

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use rupu_runtime::services::operator::{MessagesView, ViewQuery};
use serde_json::Value;
use tokio::time::Instant;

use super::remote::{gates_of, SSH_POLL};

/// How long an entry is kept after its ask: past the spacing it throttles
/// nothing, so this only bounds the map.
const KEEP: Duration = Duration::from_secs(60);

/// What an ask is about.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    /// A run's gates (`get_run`).
    Gate(String),
    /// A message space (`list_messages`), by its root.
    Msgs(String),
}

/// The answer to an entry's last ask.
enum Last {
    Gate(Value),
    Msgs(ViewQuery, MessagesView),
}

struct Entry {
    asked: Instant,
    /// The answer to the ask at `asked`; `None` while it is out, or when it
    /// failed.
    last: Option<Last>,
}

/// What an ask may do now.
pub(super) enum Claim<T> {
    /// Ask the host. The ask is recorded as made at `asked`; its answer is
    /// recorded against that instant.
    Ask { asked: Instant },
    /// Asked within the spacing: this answer stands in. The next ask may go
    /// out at `due`.
    Reuse { answer: T, due: Instant },
    /// Asked at `asked`, with nothing to stand in: ask once `due` comes.
    Wait { asked: Instant, due: Instant },
}

pub(crate) struct Throttle {
    /// [`SSH_POLL`]; shorter in tests.
    spacing: Duration,
    entries: Mutex<HashMap<(String, Key), Entry>>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::with_spacing(SSH_POLL)
    }
}

impl Throttle {
    pub(super) fn with_spacing(spacing: Duration) -> Self {
        Self {
            spacing,
            entries: Mutex::default(),
        }
    }

    /// How far apart two asks of one key go out.
    pub(super) fn spacing(&self) -> Duration {
        self.spacing
    }

    /// Whether `run`'s gates on `host` may be asked now.
    pub(super) fn claim_gate(&self, host: &str, run: &str) -> Claim<Value> {
        self.claim(host, Key::Gate(run.to_string()), |last| match last {
            Last::Gate(v) if gates_of(v).is_empty() => Some(v.clone()),
            _ => None,
        })
    }

    /// Whether `root`'s space on `host` may be read with `q` now.
    pub(super) fn claim_msgs(&self, host: &str, root: &str, q: &ViewQuery) -> Claim<MessagesView> {
        self.claim(host, Key::Msgs(root.to_string()), |last| match last {
            Last::Msgs(asked, view) => page_from(asked, view, q),
            Last::Gate(_) => None,
        })
    }

    /// The answer to the gate ask recorded at `asked`.
    pub(super) fn gate_answered(&self, host: &str, run: &str, asked: Instant, v: &Value) {
        self.answered(
            host,
            Key::Gate(run.to_string()),
            asked,
            Last::Gate(v.clone()),
        );
    }

    /// The page `q` read, by the ask recorded at `asked`.
    pub(super) fn msgs_answered(
        &self,
        host: &str,
        root: &str,
        asked: Instant,
        q: ViewQuery,
        view: &MessagesView,
    ) {
        self.answered(
            host,
            Key::Msgs(root.to_string()),
            asked,
            Last::Msgs(q, view.clone()),
        );
    }

    fn claim<T>(&self, host: &str, key: Key, reuse: impl FnOnce(&Last) -> Option<T>) -> Claim<T> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| now.saturating_duration_since(e.asked) < KEEP);
        let k = (host.to_string(), key);
        if let Some(e) = entries.get(&k) {
            let due = e.asked + self.spacing;
            if now < due {
                return match e.last.as_ref().and_then(reuse) {
                    Some(answer) => Claim::Reuse { answer, due },
                    None => Claim::Wait {
                        asked: e.asked,
                        due,
                    },
                };
            }
        }
        entries.insert(
            k,
            Entry {
                asked: now,
                last: None,
            },
        );
        Claim::Ask { asked: now }
    }

    /// Record `last` as the answer to the ask made at `asked`, unless a newer
    /// ask has taken the entry since (a slow answer never overwrites it).
    fn answered(&self, host: &str, key: Key, asked: Instant, last: Last) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get_mut(&(host.to_string(), key)) {
            if e.asked == asked {
                e.last = Some(last);
            }
        }
    }
}

/// The part of `view` (read with `asked`) that answers `q`: all of it when
/// `q` starts where it did, the messages after `q`'s cursor when the cursor
/// is one of them, else nothing (the page doesn't say what follows it).
fn page_from(asked: &ViewQuery, view: &MessagesView, q: &ViewQuery) -> Option<MessagesView> {
    if asked.kinds != q.kinds || asked.limit != q.limit {
        return None;
    }
    if asked.after == q.after {
        return Some(view.clone());
    }
    let after = q.after.as_ref()?;
    let at = view.messages.iter().position(|m| m.message.id == *after)?;
    let mut tail = view.clone();
    tail.messages.drain(..=at);
    Some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(after: Option<&str>) -> ViewQuery {
        ViewQuery {
            after: after.map(str::to_string),
            limit: 200,
            kinds: None,
        }
    }

    #[tokio::test]
    async fn a_second_ask_within_the_spacing_reuses_a_gate_free_answer_only() {
        let t = Throttle::default();
        let Claim::Ask { asked } = t.claim_gate("h", "run_a") else {
            panic!("first ask")
        };
        // Still out: nothing to reuse.
        assert!(matches!(t.claim_gate("h", "run_a"), Claim::Wait { .. }));
        let idle = json!({"run": {"status": "running", "awaiting": []}});
        t.gate_answered("h", "run_a", asked, &idle);
        assert!(
            matches!(t.claim_gate("h", "run_a"), Claim::Reuse { answer, .. } if answer == idle)
        );
        // Another host or run is its own entry.
        assert!(matches!(t.claim_gate("h2", "run_a"), Claim::Ask { .. }));
        // A parked gate is never replayed.
        let parked = json!({"run": {"awaiting": [{"step_id": "g"}]}});
        let Claim::Ask { asked } = t.claim_gate("h", "run_b") else {
            panic!("first ask")
        };
        t.gate_answered("h", "run_b", asked, &parked);
        assert!(matches!(t.claim_gate("h", "run_b"), Claim::Wait { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn at_the_spacing_the_host_is_asked_again_and_a_slow_answer_is_dropped() {
        let t = Throttle::default();
        let Claim::Ask { asked: old } = t.claim_gate("h", "run_a") else {
            panic!("first ask")
        };
        tokio::time::advance(SSH_POLL).await;
        let Claim::Ask { asked: new } = t.claim_gate("h", "run_a") else {
            panic!("due again")
        };
        // The first ask's answer lands after the second went out: it is not
        // the entry's answer.
        t.gate_answered("h", "run_a", old, &json!({"run": {"awaiting": []}}));
        assert!(matches!(t.claim_gate("h", "run_a"), Claim::Wait { .. }));
        t.gate_answered("h", "run_a", new, &json!({"run": {"awaiting": []}}));
        assert!(matches!(t.claim_gate("h", "run_a"), Claim::Reuse { .. }));
    }

    #[test]
    fn a_page_answers_only_from_a_cursor_it_covers() {
        let view: MessagesView = serde_json::from_value(json!({
            "participants": [], "legacy": false, "space": "present", "next": null,
            "messages": [
                {"id": "msg_1", "ts": "2026-10-20T00:00:00Z", "kind": "broadcast",
                 "from": "a", "body": "one", "delivery": {"resolved": [], "undelivered": []}, "seen": []},
                {"id": "msg_2", "ts": "2026-10-20T00:00:01Z", "kind": "broadcast",
                 "from": "a", "body": "two", "delivery": {"resolved": [], "undelivered": []}, "seen": []}
            ]
        }))
        .unwrap();
        let ids = |v: &MessagesView| -> Vec<String> {
            v.messages.iter().map(|m| m.message.id.clone()).collect()
        };
        let from = q(Some("msg_0"));
        assert_eq!(
            ids(&page_from(&from, &view, &from).unwrap()),
            ["msg_1", "msg_2"]
        );
        assert_eq!(
            ids(&page_from(&from, &view, &q(Some("msg_1"))).unwrap()),
            ["msg_2"]
        );
        assert!(ids(&page_from(&from, &view, &q(Some("msg_2"))).unwrap()).is_empty());
        // A cursor the page doesn't hold, or other kinds: not answerable.
        assert!(page_from(&from, &view, &q(Some("msg_9"))).is_none());
        assert!(page_from(&from, &view, &q(None)).is_none());
        let kinds = ViewQuery {
            kinds: Some(vec![rupu_fleet::MessageKind::Operator]),
            ..from.clone()
        };
        assert!(page_from(&from, &view, &kinds).is_none());
    }
}
