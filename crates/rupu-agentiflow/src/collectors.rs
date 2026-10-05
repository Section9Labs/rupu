//! Ambient-context collectors that fold the lead's coordination substrate into
//! each of its turns (spec §8): the inbox (`rupu-fleet` mailbox) and the
//! standing board directives. They are `rupu_agent::TurnCollector`s, so the
//! agent loop runs them before every model call without spending a turn.
//!
//! Framing: the collector pipeline's `wrap_injection` already presents every
//! injection as attributed data, not an instruction. The `content` built here
//! is therefore the raw attributed body only; it adds no framing of its own.
//!
//! Failure handling: a store error degrades to "no injections" (logged), the
//! same best-effort stance as `rupu_agent::CommandCollector`. A collector never
//! propagates an error or panics into the turn.

use std::sync::Arc;

use rupu_agent::{Cadence, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_fleet::{Board, Directive, Mailbox};

/// The shared inbox every participant also reads (`msg.send` to `"broadcast"`).
const BROADCAST_INBOX: &str = "broadcast";

/// Priority of an inbox message: above ordinary observations, below a standing
/// directive.
const MAILBOX_PRIORITY: u8 = 200;
/// Priority of a standing board directive.
const DIRECTIVE_PRIORITY: u8 = 230;

/// Drains this participant's inbox (and the shared `"broadcast"` inbox) into
/// `Once` message injections.
///
/// `Once` because `Mailbox::drain` consumes what it returns: each message must
/// be delivered exactly once, and the pipeline persists `Once` injections into
/// the transcript so the lead keeps them across turns.
pub struct MailboxCollector {
    pub(crate) mailbox: Arc<Mailbox>,
    pub(crate) participant: String,
}

impl MailboxCollector {
    pub fn new(mailbox: Arc<Mailbox>, participant: impl Into<String>) -> Self {
        Self {
            mailbox,
            participant: participant.into(),
        }
    }
}

impl TurnCollector for MailboxCollector {
    fn name(&self) -> &str {
        "mailbox"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let source = format!("mailbox:{}", self.participant);
        let mut inboxes = vec![self.participant.as_str()];
        if self.participant != BROADCAST_INBOX {
            inboxes.push(BROADCAST_INBOX);
        }
        let mut out = Vec::new();
        for inbox in inboxes {
            match self.mailbox.drain(inbox) {
                Ok(messages) => out.extend(messages.into_iter().map(|m| Injection {
                    source: source.clone(),
                    kind: InjectionKind::Message,
                    cadence: Cadence::Once,
                    priority: MAILBOX_PRIORITY,
                    content: format!("from {} at {}: {}", m.from, m.ts, m.body),
                })),
                Err(e) => {
                    tracing::warn!(inbox, error = %e, "mailbox collector: drain failed");
                }
            }
        }
        out
    }
}

/// Reads the board's standing directives that apply to this participant into
/// `EveryTurn` directive injections.
///
/// `EveryTurn` because `Board::read_directives` does not consume: a directive
/// stands until it is lifted, so it is re-asserted on each turn (transient,
/// never accumulated into the transcript).
pub struct DirectiveCollector {
    pub(crate) board: Arc<Board>,
    pub(crate) participant: String,
    pub(crate) role: Option<String>,
}

impl DirectiveCollector {
    pub fn new(board: Arc<Board>, participant: impl Into<String>, role: Option<String>) -> Self {
        Self {
            board,
            participant: participant.into(),
            role,
        }
    }

    /// A directive applies when it is addressed to everyone (`None`), to this
    /// participant, or to this participant's role (when one is configured).
    fn applies(&self, d: &Directive) -> bool {
        match d.addressed_to.as_deref() {
            None => true,
            Some(to) => to == self.participant || self.role.as_deref() == Some(to),
        }
    }
}

impl TurnCollector for DirectiveCollector {
    fn name(&self) -> &str {
        "directive"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let directives = match self.board.read_directives() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "directive collector: read failed");
                return Vec::new();
            }
        };
        directives
            .iter()
            .filter(|d| self.applies(d))
            .map(|d| Injection {
                source: "directive:board".to_string(),
                kind: InjectionKind::Directive,
                cadence: Cadence::EveryTurn,
                priority: DIRECTIVE_PRIORITY,
                content: format!("standing directive from {}: {}", d.author, d.body),
            })
            .collect()
    }
}

/// The lead's ambient-context collectors: its inbox and the board's standing
/// directives. `participant` is the lead's agent name (its inbox id).
pub fn lead_collectors(
    mailbox: Arc<Mailbox>,
    board: Arc<Board>,
    participant: impl Into<String>,
) -> Vec<Arc<dyn TurnCollector>> {
    let participant = participant.into();
    vec![
        Arc::new(MailboxCollector::new(mailbox, participant.clone())),
        Arc::new(DirectiveCollector::new(board, participant, None)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_fleet::FleetMessage;

    fn ctx(p: &str) -> TurnContext {
        TurnContext {
            run_id: "r".into(),
            codename: None,
            participant: p.into(),
            turn_index: 0,
        }
    }

    fn msg(from: &str, body: &str) -> FleetMessage {
        FleetMessage {
            from: from.into(),
            ts: "t".into(),
            body: body.into(),
        }
    }

    fn directive(body: &str, to: Option<&str>) -> Directive {
        Directive {
            author: "operator".into(),
            ts: "t".into(),
            body: body.into(),
            addressed_to: to.map(str::to_string),
        }
    }

    #[test]
    fn mailbox_collector_drains_inbox_as_once_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Arc::new(Mailbox::new(dir.path()));
        mb.send(
            "lead",
            &FleetMessage {
                from: "worker-1".into(),
                ts: "t".into(),
                body: "found RCE on 1.1.2.2".into(),
            },
            64,
        )
        .unwrap();
        let c = MailboxCollector {
            mailbox: mb.clone(),
            participant: "lead".into(),
        };
        let injs = c.collect(&ctx("lead"));
        assert_eq!(injs.len(), 1);
        assert_eq!(injs[0].cadence, Cadence::Once);
        assert_eq!(injs[0].kind, InjectionKind::Message);
        assert_eq!(injs[0].priority, 200);
        assert_eq!(injs[0].source, "mailbox:lead");
        assert!(injs[0].content.contains("found RCE on 1.1.2.2"));
        assert!(injs[0].content.contains("worker-1"));
        // drained: a second collect returns nothing.
        assert!(c.collect(&ctx("lead")).is_empty());
    }

    #[test]
    fn mailbox_collector_also_drains_the_broadcast_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Arc::new(Mailbox::new(dir.path()));
        mb.send("lead", &msg("w1", "direct"), 64).unwrap();
        mb.send("broadcast", &msg("w2", "to everyone"), 64).unwrap();
        mb.send("someone-else", &msg("w3", "not mine"), 64).unwrap();
        let c = MailboxCollector::new(mb.clone(), "lead");
        let injs = c.collect(&ctx("lead"));
        assert_eq!(injs.len(), 2);
        assert!(injs.iter().all(|i| i.cadence == Cadence::Once));
        assert!(injs.iter().any(|i| i.content.contains("direct")));
        assert!(injs.iter().any(|i| i.content.contains("to everyone")));
        assert!(!injs.iter().any(|i| i.content.contains("not mine")));
        assert!(c.collect(&ctx("lead")).is_empty());
    }

    #[test]
    fn mailbox_collector_with_empty_inbox_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = MailboxCollector::new(Arc::new(Mailbox::new(dir.path())), "lead");
        assert!(c.collect(&ctx("lead")).is_empty());
    }

    #[test]
    fn directive_collector_reasserts_every_turn() {
        let dir = tempfile::tempdir().unwrap();
        let board = Arc::new(Board::new(dir.path()));
        board
            .put_directive(&Directive {
                author: "lead".into(),
                ts: "t".into(),
                body: "focus on the auth module".into(),
                addressed_to: None,
            })
            .unwrap();
        let c = DirectiveCollector {
            board: board.clone(),
            participant: "lead".into(),
            role: None,
        };
        assert_eq!(c.collect(&ctx("lead")).len(), 1);
        let again = c.collect(&ctx("lead")); // not consumed
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].cadence, Cadence::EveryTurn);
        assert_eq!(again[0].kind, InjectionKind::Directive);
        assert_eq!(again[0].priority, 230);
        assert_eq!(again[0].source, "directive:board");
        assert!(again[0].content.contains("focus on the auth module"));
    }

    #[test]
    fn directive_collector_filters_by_addressing() {
        let dir = tempfile::tempdir().unwrap();
        let board = Arc::new(Board::new(dir.path()));
        for d in [
            directive("for all", None),
            directive("for lead", Some("lead")),
            directive("for the coordinator role", Some("coordinator")),
            directive("for someone else", Some("worker-9")),
        ] {
            board.put_directive(&d).unwrap();
        }
        let bodies = |role: Option<&str>| -> Vec<String> {
            DirectiveCollector::new(board.clone(), "lead", role.map(str::to_string))
                .collect(&ctx("lead"))
                .into_iter()
                .map(|i| i.content)
                .collect()
        };
        let no_role = bodies(None);
        assert_eq!(no_role.len(), 2, "{no_role:?}");
        assert!(no_role.iter().any(|c| c.contains("for all")));
        assert!(no_role.iter().any(|c| c.contains("for lead")));
        let with_role = bodies(Some("coordinator"));
        assert_eq!(with_role.len(), 3, "{with_role:?}");
        assert!(with_role.iter().any(|c| c.contains("coordinator role")));
        assert!(!with_role.iter().any(|c| c.contains("someone else")));
    }

    #[test]
    fn lead_collectors_returns_both_collectors() {
        let dir = tempfile::tempdir().unwrap();
        let cs = lead_collectors(
            Arc::new(Mailbox::new(dir.path().join("mailboxes"))),
            Arc::new(Board::new(dir.path().join("board"))),
            "lead",
        );
        let names: Vec<&str> = cs.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["mailbox", "directive"]);
    }
}
