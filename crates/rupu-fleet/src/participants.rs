//! The participant registry of a message space (spec F1a §3.1):
//! `participants.jsonl`, one append-only event per transition, folded on
//! read into the [`Roster`] that addresses resolve against.

use crate::address::{normalize, Address, LEAD, OPERATOR};
use crate::error::FleetError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Who a run is in its space: written into its `joined` line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// The run's full codename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// The agent it runs: what `role:<agent>` matches.
    pub agent: String,
    pub run_id: String,
    /// The workflow step it runs: what `step:<id>` matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// Its unit of a fanned-out step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// How it was started (`workflow_step`, `flow_lead`, `flow_unit`,
    /// `subagent`, `standalone`, `session_turn`).
    pub kind: String,
    /// The run that launched it: what its `parent` resolves to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// The process the run lives in: whether a standalone run is still
    /// running, when it has no record yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// One line of `participants.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ParticipantEvent {
    Joined {
        address: String,
        #[serde(flatten)]
        member: Member,
        at: String,
    },
    Left {
        address: String,
        run_id: String,
        status: String,
        at: String,
    },
}

/// A participant as the registry last recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub address: String,
    pub member: Member,
    pub live: bool,
}

/// Why a message reached none (or not all) of its intended recipients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Undeliverable {
    UnknownParticipant,
    NoLiveParticipant,
    NoParent,
    NoLead,
    ParticipantLeft,
    InboxFull,
    BroadcastFull,
}

/// What an address resolved to at send time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// The address recorded as the message's `to`: concrete for `parent` and
    /// `lead`, as written otherwise.
    pub to: String,
    pub recipients: Vec<String>,
    pub undelivered: Option<Undeliverable>,
}

/// The sender an address is resolved for.
#[derive(Debug, Clone, Copy)]
pub struct Sender<'a> {
    pub address: &'a str,
    pub parent_run_id: Option<&'a str>,
}

/// The registry folded: the last state of every address, in join order.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    by_address: BTreeMap<String, Participant>,
    order: Vec<String>,
    /// Every run that ever joined, and the address it joined at: a run's
    /// `parent` is found by run id, even after a later run (a session's next
    /// turn) took the address over.
    run_address: BTreeMap<String, String>,
}

impl Roster {
    pub fn read(path: &Path) -> Result<Self, FleetError> {
        let text = match std::fs::read(path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(FleetError::Io {
                    action: format!("read {}", path.display()),
                    source: e,
                })
            }
        };
        let mut roster = Roster::default();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ParticipantEvent>(line) {
                Ok(ev) => roster.apply(ev),
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    line = n + 1,
                    error = %e,
                    "skipping an unreadable participants line"
                ),
            }
        }
        Ok(roster)
    }

    fn apply(&mut self, ev: ParticipantEvent) {
        match ev {
            ParticipantEvent::Joined {
                address, member, ..
            } => {
                if !self.by_address.contains_key(&address) {
                    self.order.push(address.clone());
                }
                self.run_address
                    .insert(member.run_id.clone(), address.clone());
                self.by_address.insert(
                    address.clone(),
                    Participant {
                        address,
                        member,
                        live: true,
                    },
                );
            }
            // A `left` of an earlier run of the address (a retried step's
            // first attempt) never ends the run that joined after it.
            ParticipantEvent::Left {
                address, run_id, ..
            } => {
                if let Some(p) = self.by_address.get_mut(&address) {
                    if p.member.run_id == run_id {
                        p.live = false;
                    }
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_address.is_empty()
    }

    pub fn get(&self, address: &str) -> Option<&Participant> {
        self.by_address.get(address)
    }

    /// Every participant, in the order they first joined.
    pub fn participants(&self) -> impl Iterator<Item = &Participant> {
        self.order.iter().filter_map(|a| self.by_address.get(a))
    }

    fn live_except<'a>(&'a self, sender: &'a str) -> impl Iterator<Item = &'a Participant> {
        self.participants()
            .filter(move |p| p.live && p.address != sender)
    }

    /// The participant `name` names: its address, or its full codename.
    fn named(&self, name: &str) -> Option<&Participant> {
        self.by_address.get(name).or_else(|| {
            self.participants().find(|p| {
                p.member
                    .codename
                    .as_deref()
                    .is_some_and(|c| normalize(c) == name)
            })
        })
    }

    /// Resolve `to` for `sender` (spec F1a §3.2). `legacy_inbox` says
    /// whether a mailbox exists under a name: participants of a space
    /// written before the registry existed aren't in the roster, and are
    /// found by their inbox directories (`recon#1` → `mailboxes/recon-1/`).
    /// Every joined participant is in the roster, so the fallback only ever
    /// finds those.
    pub fn resolve(
        &self,
        to: &Address,
        sender: Sender<'_>,
        legacy_inbox: &dyn Fn(&str) -> bool,
    ) -> Resolution {
        let one = |to: String, recipient: Option<String>, reason: Undeliverable| match recipient {
            Some(r) => Resolution {
                to,
                recipients: vec![r],
                undelivered: None,
            },
            None => Resolution {
                to,
                recipients: Vec::new(),
                undelivered: Some(reason),
            },
        };
        let many = |to: String, recipients: Vec<String>| {
            let undelivered = recipients
                .is_empty()
                .then_some(Undeliverable::NoLiveParticipant);
            Resolution {
                to,
                recipients,
                undelivered,
            }
        };
        let live_or_left = |p: &Participant| {
            if p.live {
                (Some(p.address.clone()), Undeliverable::ParticipantLeft)
            } else {
                (None, Undeliverable::ParticipantLeft)
            }
        };
        match to {
            Address::Operator => one(
                OPERATOR.into(),
                Some(OPERATOR.into()),
                Undeliverable::UnknownParticipant,
            ),
            Address::Broadcast => many(
                to.to_string(),
                self.live_except(sender.address)
                    .map(|p| p.address.clone())
                    .collect(),
            ),
            Address::Role(role) => many(
                to.to_string(),
                self.live_except(sender.address)
                    .filter(|p| &p.member.agent == role)
                    .map(|p| p.address.clone())
                    .collect(),
            ),
            Address::Step(step) => many(
                to.to_string(),
                self.live_except(sender.address)
                    .filter(|p| p.member.step_id.as_deref() == Some(step.as_str()))
                    .map(|p| p.address.clone())
                    .collect(),
            ),
            Address::Parent => {
                let parent = sender
                    .parent_run_id
                    .and_then(|run| self.run_address.get(run))
                    .and_then(|address| self.by_address.get(address));
                match parent {
                    None => one(to.to_string(), None, Undeliverable::NoParent),
                    Some(p) => {
                        let (r, reason) = live_or_left(p);
                        one(p.address.clone(), r, reason)
                    }
                }
            }
            Address::Lead => match self.by_address.get(LEAD) {
                Some(p) => {
                    let (r, reason) = live_or_left(p);
                    one(LEAD.into(), r, reason)
                }
                None if legacy_inbox(LEAD) => {
                    one(LEAD.into(), Some(LEAD.into()), Undeliverable::NoLead)
                }
                None => one(LEAD.into(), None, Undeliverable::NoLead),
            },
            Address::Named(name) => {
                if let Some(p) = self.named(name) {
                    let (r, reason) = live_or_left(p);
                    return one(p.address.clone(), r, reason);
                }
                // A bare word that isn't an address is a role.
                let role: Vec<String> = self
                    .live_except(sender.address)
                    .filter(|p| &p.member.agent == name)
                    .map(|p| p.address.clone())
                    .collect();
                if !role.is_empty() {
                    return many(name.clone(), role);
                }
                // A participant of a space written before the registry is
                // found by its inbox directory (`recon#1` → `recon-1`).
                let legacy = legacy_inbox(name);
                one(
                    name.clone(),
                    legacy.then(|| name.clone()),
                    Undeliverable::UnknownParticipant,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(address: &str, agent: &str, run: &str, step: Option<&str>) -> ParticipantEvent {
        ParticipantEvent::Joined {
            address: address.into(),
            member: Member {
                codename: Some(format!("c/{address}")),
                agent: agent.into(),
                run_id: run.into(),
                step_id: step.map(str::to_string),
                unit: None,
                kind: "workflow_step".into(),
                parent_run_id: None,
                pid: None,
            },
            at: "t".into(),
        }
    }

    fn left(address: &str, run: &str) -> ParticipantEvent {
        ParticipantEvent::Left {
            address: address.into(),
            run_id: run.into(),
            status: "done".into(),
            at: "t".into(),
        }
    }

    fn roster(events: Vec<ParticipantEvent>) -> Roster {
        let mut r = Roster::default();
        for e in events {
            r.apply(e);
        }
        r
    }

    #[test]
    fn a_stale_left_never_ends_a_later_join() {
        let r = roster(vec![
            joined("lynx", "recon", "run_a", None),
            joined("lynx", "recon", "run_b", None),
            left("lynx", "run_a"),
        ]);
        assert!(r.get("lynx").unwrap().live);
        let r = roster(vec![
            joined("lynx", "recon", "run_a", None),
            left("lynx", "run_a"),
        ]);
        assert!(!r.get("lynx").unwrap().live);
    }

    #[test]
    fn the_joined_line_keeps_its_shape() {
        let line = serde_json::to_value(joined("lynx#1", "recon", "run_1", Some("scan"))).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "t": "joined", "address": "lynx#1", "codename": "c/lynx#1", "agent": "recon",
                "run_id": "run_1", "step_id": "scan", "kind": "workflow_step", "at": "t"
            })
        );
    }
}
