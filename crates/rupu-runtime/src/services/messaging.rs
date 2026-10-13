//! A run's message space (spec F1a): where each origin's space lives, when a
//! run joins it, and the collectors that deliver what is said in it before
//! every model call.
//!
//! | Root | Space |
//! |---|---|
//! | agentiflow | its run dir (the flow builds the lead's and units' buses) |
//! | workflow run | `<runs>/<workflow_run_id>/messages/` |
//! | session | `<sessions>/<id>/messages/` |
//! | standalone `rupu run` | `<runs>/<run_id>/messages/` |
//!
//! A child shares its parent's space: an in-process sub-agent is handed a
//! bus on it, a process `agent` child opens one from `--message-space`.
//! Nothing is created until something is written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rupu_agent::{Cadence, InjectedMessage, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_fleet::{address, Bus, BusCaps, Directive, FleetMessage, Member};

use crate::assembly::Origin;

/// Priority of a standing directive.
pub const DIRECTIVE_PRIORITY: u8 = 230;
/// Priority of a direct message.
pub const DIRECT_PRIORITY: u8 = 200;
/// Priority of a broadcast.
pub const BROADCAST_PRIORITY: u8 = 190;

/// The bus limits `[messaging]` sets.
pub fn caps(config: &rupu_config::Config) -> BusCaps {
    let mut caps = BusCaps::default();
    if let Some(mb) = config.messaging.log_max_mb {
        caps.log_max_bytes = mb.saturating_mul(1024 * 1024);
    }
    caps
}

/// The space a run of `origin` (as `run_id`) belongs to, when its origin has
/// one of its own to give it. An agentiflow's runs and a child's are handed
/// their bus by whoever starts them; a workflow run with no store (an
/// in-memory run) has none.
pub fn space_dir(global: &Path, origin: &Origin, run_id: &str) -> Option<PathBuf> {
    let runs = global.join("runs");
    match origin {
        Origin::Standalone if !run_id.is_empty() => Some(runs.join(run_id).join("messages")),
        Origin::WorkflowStep {
            workflow_run_id, ..
        } if !workflow_run_id.is_empty() => Some(runs.join(workflow_run_id).join("messages")),
        Origin::SessionTurn { session_dir, .. } => Some(session_dir.join("messages")),
        _ => None,
    }
}

/// Whether anyone has joined or written to the space at `space` yet: once
/// one has, every later run of the root joins it too.
pub fn is_active(space: &Path) -> bool {
    space.join("participants.jsonl").exists() || space.join("log.jsonl").exists()
}

/// How a run of `origin` was started, as its `joined` line says.
pub fn member_kind(origin: &Origin, launched: bool) -> &'static str {
    match origin {
        Origin::Standalone if launched => "agent_child",
        Origin::Standalone => "standalone",
        Origin::WorkflowStep { .. } => "workflow_step",
        Origin::SubAgent { .. } => "subagent",
        Origin::SessionTurn { .. } => "session_turn",
        Origin::FlowLead { .. } => "flow_lead",
        Origin::FlowUnit { .. } => "flow_unit",
    }
}

/// A top-level run's address in its own space: its codename below the crew
/// (`lynx#2`), else its run id.
pub fn address_of(codename: Option<&str>, run_id: &str) -> String {
    codename
        .and_then(address::of_codename)
        .unwrap_or_else(|| run_id.to_string())
}

/// The collectors that deliver `bus`'s space into its run's turns: its
/// inbox and the broadcasts it hasn't seen, and the directives that apply to
/// it.
pub fn collectors(bus: &Arc<Bus>) -> Vec<Arc<dyn TurnCollector>> {
    vec![
        Arc::new(MessageCollector {
            bus: Arc::clone(bus),
        }),
        Arc::new(DirectiveCollector {
            bus: Arc::clone(bus),
        }),
    ]
}

/// Drains this participant's inbox, and reads the broadcast log past its
/// own cursor, into one `Once` injection per message.
///
/// `Once` because both reads consume: each message is delivered exactly
/// once, kept in the conversation and recorded where it entered it. A
/// participant's broadcast cursor starts where it joined (no backlog).
/// A store error degrades to no injections (logged): a collector never fails
/// the turn.
pub struct MessageCollector {
    bus: Arc<Bus>,
}

impl MessageCollector {
    fn injection(&self, m: FleetMessage, broadcast: bool) -> Injection {
        let me = &self.bus.participant;
        let (source, kind, priority) = if broadcast {
            (format!("broadcast:{me}"), "broadcast", BROADCAST_PRIORITY)
        } else {
            (format!("mailbox:{me}"), "direct", DIRECT_PRIORITY)
        };
        let to = match m.to.as_deref() {
            Some(to) if to != me.as_str() && !broadcast => format!(" to {to}"),
            _ => String::new(),
        };
        Injection {
            source,
            kind: InjectionKind::Message,
            cadence: Cadence::Once,
            priority,
            content: format!("from {}{to} at {}: {}", m.from, m.ts, m.body),
            messages: m
                .id
                .clone()
                .map(|id| InjectedMessage {
                    id,
                    from: m.from.clone(),
                    from_codename: m.from_codename.clone(),
                    to: m.to.clone(),
                    kind: kind.into(),
                    body: m.body.clone(),
                    urgent: false,
                })
                .into_iter()
                .collect(),
        }
    }
}

impl TurnCollector for MessageCollector {
    fn name(&self) -> &str {
        "messages"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        // Nobody can reach a run that hasn't joined; reading would create
        // its mailbox.
        if !self.bus.is_joined() {
            return Vec::new();
        }
        let me = &self.bus.participant;
        let mut out = Vec::new();
        match self.bus.mailbox.drain(me) {
            Ok(messages) => out.extend(messages.into_iter().map(|m| self.injection(m, false))),
            Err(e) => tracing::warn!(inbox = %me, error = %e, "message collector: drain failed"),
        }
        match self.bus.mailbox.read_broadcast(me) {
            Ok(messages) => out.extend(messages.into_iter().map(|m| self.injection(m, true))),
            Err(e) => {
                tracing::warn!(participant = %me, error = %e, "message collector: broadcast read failed")
            }
        }
        out
    }

    fn seen(&self, ids: &[String], turn: u32) {
        if let Err(e) = self.bus.mark_seen(ids, &self.bus.participant, turn) {
            tracing::warn!(error = %e, "message collector: could not write receipts");
        }
    }
}

/// Reads the standing directives that apply to this participant into
/// `EveryTurn` directive injections: a directive stands until it is lifted,
/// so it is re-asserted on each turn (the transcript records it once).
pub struct DirectiveCollector {
    bus: Arc<Bus>,
}

impl DirectiveCollector {
    /// A directive applies when it is addressed to everyone, to this
    /// participant (its address or codename), to `role:<its agent>` (or the
    /// bare agent name), or to `step:<its step>`.
    fn applies(&self, d: &Directive, member: Option<&Member>) -> bool {
        let Some(to) = d.addressed_to.as_deref() else {
            return true;
        };
        if to == self.bus.participant {
            return true;
        }
        let Some(m) = member else {
            return false;
        };
        let agent = m.agent.as_str();
        if let Some(role) = to.strip_prefix("role:") {
            return role == agent;
        }
        if let Some(step) = to.strip_prefix("step:") {
            return m.step_id.as_deref() == Some(step);
        }
        to == agent
            || m.codename
                .as_deref()
                .is_some_and(|c| address::normalize(c) == address::normalize(to))
    }
}

/// The text a standing directive is injected as. A directive with an id
/// carries it in brackets: it is what `board.retract` takes, and the
/// injection is where the author reliably re-sees it.
fn directive_text(d: &Directive) -> String {
    if d.id.trim().is_empty() {
        format!("standing directive from {}: {}", d.author, d.body)
    } else {
        format!(
            "standing directive [{}] from {}: {}",
            d.id, d.author, d.body
        )
    }
}

impl TurnCollector for DirectiveCollector {
    fn name(&self) -> &str {
        "directives"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let directives = match self.bus.board.read_directives() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "directive collector: read failed");
                return Vec::new();
            }
        };
        let member = self.bus.member();
        directives
            .iter()
            .filter(|d| self.applies(d, member.as_ref()))
            .map(|d| Injection {
                source: "directive:board".to_string(),
                kind: InjectionKind::Directive,
                cadence: Cadence::EveryTurn,
                priority: DIRECTIVE_PRIORITY,
                content: directive_text(d),
                // A directive from before the log has no message to receipt.
                messages: d
                    .id
                    .starts_with("msg_")
                    .then(|| InjectedMessage {
                        id: d.id.clone(),
                        from: d.author.clone(),
                        from_codename: None,
                        to: d.addressed_to.clone(),
                        kind: "directive".into(),
                        body: d.body.clone(),
                        urgent: false,
                    })
                    .into_iter()
                    .collect(),
            })
            .collect()
    }

    fn seen(&self, ids: &[String], turn: u32) {
        if let Err(e) = self.bus.mark_seen(ids, &self.bus.participant, turn) {
            tracing::warn!(error = %e, "directive collector: could not write receipts");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_fleet::{Address, LogLine};

    fn ctx() -> TurnContext {
        TurnContext {
            run_id: "r".into(),
            codename: None,
            participant: "p".into(),
            turn_index: 0,
        }
    }

    fn joined(space: &Path, address: &str, agent: &str, step: Option<&str>) -> Arc<Bus> {
        let bus = Arc::new(Bus::open(space, address));
        bus.join(Member {
            codename: Some(format!("cobalt-harbor/{address}")),
            agent: agent.into(),
            run_id: format!("run_{agent}"),
            step_id: step.map(str::to_string),
            kind: "workflow_step".into(),
            ..Default::default()
        })
        .unwrap();
        bus
    }

    #[test]
    fn messages_are_delivered_once_and_receipted() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = joined(tmp.path(), "otter", "triage", None);
        let sender = joined(tmp.path(), "lynx", "recon", None);
        let c = MessageCollector {
            bus: Arc::clone(&reader),
        };
        let direct = sender
            .send(&Address::parse("role:triage").unwrap(), "look at 10.0.0.5")
            .unwrap();
        let bcast = sender.send(&Address::Broadcast, "regroup").unwrap();

        let got = c.collect(&ctx());
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|i| i.cadence == Cadence::Once));
        let d = got.iter().find(|i| i.priority == DIRECT_PRIORITY).unwrap();
        assert_eq!(d.source, "mailbox:otter");
        assert_eq!(
            d.content.split(" at ").next(),
            Some("from lynx to role:triage")
        );
        assert_eq!(d.messages[0].id, direct.id());
        assert_eq!(d.messages[0].kind, "direct");
        let b = got
            .iter()
            .find(|i| i.priority == BROADCAST_PRIORITY)
            .unwrap();
        assert_eq!(b.source, "broadcast:otter");
        assert_eq!(b.messages[0].id, bcast.id());
        assert!(c.collect(&ctx()).is_empty(), "delivered once");

        c.seen(&[direct.id().to_string()], 3);
        let receipts: Vec<LogLine> = reader
            .read_log(None, 100)
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| matches!(l, LogLine::Seen { .. }))
            .collect();
        assert_eq!(
            receipts,
            vec![LogLine::Seen {
                id: direct.id().to_string(),
                by: "otter".into(),
                turn: 3,
                at: match &receipts[0] {
                    LogLine::Seen { at, .. } => at.clone(),
                    _ => unreachable!(),
                },
            }]
        );
    }

    #[test]
    fn a_run_that_has_not_joined_reads_nothing_and_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let space = tmp.path().join("messages");
        let bus = Arc::new(Bus::open(&space, "heron"));
        bus.prepare(Member {
            agent: "auditor".into(),
            run_id: "r".into(),
            kind: "standalone".into(),
            ..Default::default()
        });
        for c in collectors(&bus) {
            assert!(c.collect(&ctx()).is_empty());
        }
        assert!(!space.exists());
    }

    #[test]
    fn directives_match_address_role_step_and_codename() {
        let tmp = tempfile::tempdir().unwrap();
        let lead = joined(tmp.path(), "lead", "coordinator", None);
        let me = joined(tmp.path(), "lynx#1", "recon", Some("scan"));
        for to in [
            None,
            Some("lynx#1"),
            Some("role:recon"),
            Some("recon"),
            Some("step:scan"),
            Some("cobalt-harbor/lynx#1"),
            Some("role:triage"),
            Some("step:fix"),
            Some("otter#1"),
        ] {
            let to = to.map(|t| Address::parse(t).unwrap());
            lead.directive(to.as_ref(), &format!("to {to:?}")).unwrap();
        }
        let c = DirectiveCollector {
            bus: Arc::clone(&me),
        };
        let got = c.collect(&ctx());
        let bodies: Vec<&str> = got.iter().map(|i| i.messages[0].body.as_str()).collect();
        assert_eq!(bodies.len(), 6, "{bodies:?}");
        assert!(bodies
            .iter()
            .all(|b| !b.contains("triage") && !b.contains("fix") && !b.contains("otter")));
        assert!(got
            .iter()
            .all(|i| i.cadence == Cadence::EveryTurn && i.priority == DIRECTIVE_PRIORITY));
        assert_eq!(c.collect(&ctx()).len(), 6, "a directive stands");

        let first = got[0].messages[0].id.clone();
        lead.retract(&first).unwrap();
        assert_eq!(c.collect(&ctx()).len(), 5, "until it is lifted");
        assert!(got[0]
            .content
            .starts_with(&format!("standing directive [{first}] from lead:")));
    }

    #[test]
    fn a_directive_from_before_the_log_has_no_message() {
        let tmp = tempfile::tempdir().unwrap();
        let me = joined(tmp.path(), "lead", "coordinator", None);
        std::fs::create_dir_all(tmp.path().join("board")).unwrap();
        std::fs::write(
            tmp.path().join("board").join("directives.jsonl"),
            "{\"author\":\"operator\",\"ts\":\"t\",\"body\":\"old rule\"}\n",
        )
        .unwrap();
        let got = DirectiveCollector { bus: me }.collect(&ctx());
        assert_eq!(got[0].content, "standing directive from operator: old rule");
        assert!(got[0].messages.is_empty());
    }

    #[test]
    fn spaces_by_origin() {
        let g = Path::new("/g");
        assert_eq!(
            space_dir(g, &Origin::Standalone, "run_1"),
            Some(PathBuf::from("/g/runs/run_1/messages"))
        );
        assert_eq!(
            space_dir(
                g,
                &Origin::SessionTurn {
                    session_id: "ses_1".into(),
                    session_dir: "/s/ses_1".into()
                },
                "turn"
            ),
            Some(PathBuf::from("/s/ses_1/messages"))
        );
        assert_eq!(
            space_dir(
                g,
                &Origin::FlowLead {
                    flow_id: "af".into(),
                    flow_dir: "/g/agentiflows/af".into()
                },
                "af"
            ),
            None,
            "a flow hands its runs their bus"
        );
        assert_eq!(
            address_of(Some("cobalt-harbor/lynx#2.3"), "run_1"),
            "lynx#2"
        );
        assert_eq!(address_of(None, "run_1"), "run_1");
    }
}
