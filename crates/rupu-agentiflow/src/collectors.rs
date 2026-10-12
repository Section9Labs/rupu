//! Ambient-context collectors that fold the lead's coordination substrate into
//! each of its turns (spec §8): the inbox (`rupu-fleet` mailbox), the
//! standing board directives, and the lead's roster index of its pool
//! ([`RosterCollector`]). They are `rupu_agent::TurnCollector`s, so the
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
use rupu_tools::CatalogPort;

/// Priority of an inbox message: above ordinary observations, below a standing
/// directive.
const MAILBOX_PRIORITY: u8 = 200;
/// Priority of a standing board directive.
const DIRECTIVE_PRIORITY: u8 = 230;

/// Drains this participant's inbox, and reads the shared broadcast log past its
/// own cursor, into `Once` message injections.
///
/// `Once` because both reads consume: `Mailbox::drain` removes what it returns,
/// and `Mailbox::read_broadcast` advances this participant's cursor past what
/// it returns, so each message is delivered exactly once. The pipeline keeps
/// `Once` injections in the lead's in-memory conversation (they return via
/// `RunExit.messages` / history), so the lead carries them across rounds; they
/// are not written as transcript events.
///
/// Broadcast is NOT drained from a shared inbox: the first reader would take it
/// from every other reader. Each participant has its own cursor into one
/// append-only log, so a broadcast reaches all of them. A participant's first
/// read starts at the log's current end (no backlog), so the collector should
/// run once as the participant is registered if it must see every later
/// broadcast.
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
        let message = |source: String, m: rupu_fleet::FleetMessage| Injection {
            source,
            kind: InjectionKind::Message,
            cadence: Cadence::Once,
            priority: MAILBOX_PRIORITY,
            content: format!("from {} at {}: {}", m.from, m.ts, m.body),
        };
        let mut out = Vec::new();
        match self.mailbox.drain(&self.participant) {
            Ok(messages) => {
                let source = format!("mailbox:{}", self.participant);
                out.extend(messages.into_iter().map(|m| message(source.clone(), m)));
            }
            Err(e) => {
                tracing::warn!(inbox = %self.participant, error = %e, "mailbox collector: drain failed");
            }
        }
        match self.mailbox.read_broadcast(&self.participant) {
            Ok(messages) => {
                let source = format!("broadcast:{}", self.participant);
                out.extend(messages.into_iter().map(|m| message(source.clone(), m)));
            }
            Err(e) => {
                tracing::warn!(participant = %self.participant, error = %e, "mailbox collector: broadcast read failed");
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
                content: directive_text(d),
            })
            .collect()
    }
}

/// The text a standing directive is injected as. A directive with an id carries
/// it in brackets: the injection is the one place the lead reliably re-sees a
/// standing directive (the `board.directive` result does not survive compaction
/// or a resumed run), and the id is what `board.retract` takes. A legacy
/// directive with no id cannot be retracted, so it shows none.
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

/// The collectors of the participant `bus` addresses: its inbox and the
/// board's standing directives, over the same stores its `board.*` /
/// `msg.send` tools write.
pub fn bus_collectors(bus: &rupu_fleet::Bus) -> Vec<Arc<dyn TurnCollector>> {
    lead_collectors(
        Arc::new(bus.mailbox.clone()),
        Arc::new(bus.board.clone()),
        bus.participant.clone(),
    )
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

// ---- RosterCollector --------------------------------------------------------

/// Priority of the roster index: a standing status line, below an inbox
/// message (200) and a board directive (230).
const ROSTER_PRIORITY: u8 = 180;
/// Longest description kept per roster line; the full text is one `agents.get`
/// / `workflows.get` away.
const MAX_DESCRIPTION_CHARS: usize = 120;

/// Collapse `s` to one trimmed line of at most [`MAX_DESCRIPTION_CHARS`]
/// characters (cut on a char boundary, ending in `...` when cut).
fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_DESCRIPTION_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(MAX_DESCRIPTION_CHARS).collect();
    cut.push_str("...");
    cut
}

/// `- name -- description` (or `- name` when there is none).
fn roster_line(name: &str, description: Option<&str>) -> String {
    match description.map(one_line).filter(|d| !d.is_empty()) {
        Some(d) => format!("- {name} \u{2014} {d}"),
        None => format!("- {name}"),
    }
}

/// The lead's ambient roster awareness: a compact index of the agents and
/// workflows in its POOL (not the whole catalog), each resolved against the
/// run's [`CatalogPort`] as `name -- description`, re-asserted on every turn.
///
/// `EveryTurn` because the index is not consumed by reading it: it is transient
/// context, never accumulated into the transcript. Pool agents are addressed by
/// `name`; pool workflows by `id` (the file stem), matching `workflows.list`.
///
/// `content` is the raw index only. The collector pipeline's `wrap_injection`
/// already frames every injection as attributed data, so no framing is added
/// here (descriptions come from catalog files and must not read as
/// instructions twice over).
///
/// Failure handling is best-effort, like the other collectors: when the agents
/// cannot be loaded (one unparseable `.md` fails the agent listing) the pool
/// agents are listed by name with no descriptions; a pool entry the catalog does
/// not hold is marked `(not found)`. Nothing here panics or returns an error
/// into the turn. A pool with no agents and no workflows emits nothing.
pub struct RosterCollector {
    pool_agents: Vec<String>,
    pool_workflows: Vec<String>,
    catalog: Arc<dyn CatalogPort>,
}

impl RosterCollector {
    pub fn new(
        pool_agents: Vec<String>,
        pool_workflows: Vec<String>,
        catalog: Arc<dyn CatalogPort>,
    ) -> Self {
        Self {
            pool_agents,
            pool_workflows,
            catalog,
        }
    }

    fn agent_lines(&self) -> Vec<String> {
        if self.pool_agents.is_empty() {
            return Vec::new();
        }
        let catalog = match self.catalog.agents() {
            Ok(agents) => Some(agents),
            Err(e) => {
                tracing::warn!(error = %e, "roster collector: listing pool agents without descriptions");
                None
            }
        };
        let mut lines = vec!["agents:".to_string()];
        for name in &self.pool_agents {
            lines.push(match &catalog {
                Some(agents) => match agents.iter().find(|a| &a.name == name) {
                    Some(a) => roster_line(name, a.description.as_deref()),
                    None => format!("- {name} (not found)"),
                },
                None => roster_line(name, None),
            });
        }
        lines
    }

    fn workflow_lines(&self) -> Vec<String> {
        if self.pool_workflows.is_empty() {
            return Vec::new();
        }
        let catalog = self.catalog.workflows();
        let mut lines = vec!["workflows:".to_string()];
        for id in &self.pool_workflows {
            lines.push(match catalog.iter().find(|w| &w.id == id) {
                Some(w) if w.parse_error.is_some() => format!("- {id} (parse error)"),
                Some(w) => roster_line(id, w.description.as_deref()),
                None => format!("- {id} (not found)"),
            });
        }
        lines
    }
}

impl TurnCollector for RosterCollector {
    fn name(&self) -> &str {
        "roster"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let mut lines = self.agent_lines();
        lines.extend(self.workflow_lines());
        if lines.is_empty() {
            return Vec::new();
        }
        vec![Injection {
            source: "roster".to_string(),
            kind: InjectionKind::Status,
            cadence: Cadence::EveryTurn,
            priority: ROSTER_PRIORITY,
            content: lines.join("\n"),
        }]
    }
}

/// The lead's roster collector as a trait object, ready to push onto its
/// collector list.
pub fn roster_collector(
    pool_agents: Vec<String>,
    pool_workflows: Vec<String>,
    catalog: Arc<dyn CatalogPort>,
) -> Arc<dyn TurnCollector> {
    Arc::new(RosterCollector::new(pool_agents, pool_workflows, catalog))
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
            id: String::new(),
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
    fn mailbox_collector_delivers_broadcast_via_its_cursor_not_a_shared_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let mb = Arc::new(Mailbox::new(dir.path()));
        let lead = MailboxCollector::new(mb.clone(), "lead");
        let other = MailboxCollector::new(mb.clone(), "worker-2");
        // First collect registers each reader at the log tip.
        assert!(lead.collect(&ctx("lead")).is_empty());
        assert!(other.collect(&ctx("worker-2")).is_empty());

        mb.send("lead", &msg("w1", "direct"), 64).unwrap();
        mb.broadcast_send(&msg("w2", "to everyone"), 64).unwrap();
        mb.send("someone-else", &msg("w3", "not mine"), 64).unwrap();

        let injs = lead.collect(&ctx("lead"));
        assert_eq!(injs.len(), 2);
        assert!(injs.iter().all(|i| i.cadence == Cadence::Once));
        assert!(injs.iter().all(|i| i.kind == InjectionKind::Message));
        assert!(injs.iter().all(|i| i.priority == 200));
        let direct = injs.iter().find(|i| i.content.contains("direct")).unwrap();
        assert_eq!(direct.source, "mailbox:lead");
        let bcast = injs
            .iter()
            .find(|i| i.content.contains("to everyone"))
            .unwrap();
        assert_eq!(bcast.source, "broadcast:lead");
        assert!(bcast.content.contains("w2"), "{}", bcast.content);
        assert!(!injs.iter().any(|i| i.content.contains("not mine")));
        // consumed for the lead only...
        assert!(lead.collect(&ctx("lead")).is_empty());
        // ...the lead reading it did not take it from the other reader.
        let theirs = other.collect(&ctx("worker-2"));
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].source, "broadcast:worker-2");
        assert!(theirs[0].content.contains("to everyone"));
        assert!(other.collect(&ctx("worker-2")).is_empty());
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
                id: String::new(),
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
    fn directive_collector_stops_injecting_a_retracted_directive() {
        let dir = tempfile::tempdir().unwrap();
        let board = Arc::new(Board::new(dir.path()));
        let stale = board
            .put_directive(&directive("focus on auth", None))
            .unwrap();
        board
            .put_directive(&directive("expand to staging", None))
            .unwrap();
        let c = DirectiveCollector::new(board.clone(), "lead", None);
        assert_eq!(c.collect(&ctx("lead")).len(), 2);

        board.retract_directive(&stale).unwrap();
        let after = c.collect(&ctx("lead"));
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(after[0].content.contains("expand to staging"));
    }

    #[test]
    fn directive_injection_shows_the_id_a_retraction_needs() {
        let dir = tempfile::tempdir().unwrap();
        let board = Arc::new(Board::new(dir.path()));
        let id = board
            .put_directive(&directive("focus on auth", None))
            .unwrap();
        // A legacy line (written before ids): live, but nothing to retract it by.
        std::fs::write(
            dir.path().join("board").join("directives.jsonl"),
            std::fs::read_to_string(dir.path().join("board").join("directives.jsonl")).unwrap()
                + "{\"author\":\"operator\",\"ts\":\"t\",\"body\":\"old rule\"}\n",
        )
        .unwrap();

        let c = DirectiveCollector::new(board, "lead", None);
        let got: Vec<String> = c
            .collect(&ctx("lead"))
            .into_iter()
            .map(|i| i.content)
            .collect();
        assert_eq!(
            got,
            [
                format!("standing directive [{id}] from operator: focus on auth"),
                "standing directive from operator: old rule".to_string(),
            ]
        );
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

#[cfg(test)]
mod roster_tests {
    use super::*;
    use rupu_agent::InjectionKind;
    use std::path::Path;

    const RECON: &str = "---\nname: recon\ndescription: Surveys the layout of a repository.\ntools: [read_file, grep]\n---\nYou survey a repository.\n";
    const WRITER: &str = "---\nname: writer\n---\nYou write text.\n";
    const ONE_STEP: &str = "name: Plain Flow\ndescription: A single placeholder step.\ninputs:\n  topic:\n    type: string\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n";

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// The global root holding two agents (one described) + one workflow
    /// whose stem differs from its declared name.
    struct Fixture {
        _tmp: tempfile::TempDir,
        global: std::path::PathBuf,
    }

    fn fixture() -> (Fixture, Arc<dyn CatalogPort>) {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        write(&global, "agents/recon.md", RECON);
        write(&global, "agents/writer.md", WRITER);
        write(&global, "workflows/plain-flow.yaml", ONE_STEP);
        let catalog = Arc::new(rupu_orchestrator::FsCatalog::new(&global, None));
        (Fixture { _tmp: tmp, global }, catalog)
    }

    // ---- RosterCollector ----------------------------------------------------

    fn turn() -> TurnContext {
        TurnContext {
            run_id: "r".into(),
            codename: None,
            participant: "lead".into(),
            turn_index: 0,
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collector_emits_one_status_every_turn_injection_for_the_pool() {
        let (_fx, ctx) = fixture();
        let c = roster_collector(names(&["recon"]), names(&["plain-flow"]), ctx);
        assert_eq!(c.name(), "roster");
        let injs = c.collect(&turn());
        assert_eq!(injs.len(), 1);
        let i = &injs[0];
        assert_eq!(i.kind, InjectionKind::Status);
        assert_eq!(i.cadence, Cadence::EveryTurn);
        assert_eq!(i.priority, 180);
        assert_eq!(i.source, "roster");
        assert!(i.content.contains("recon"), "{}", i.content);
        assert!(
            i.content.contains("Surveys the layout of a repository."),
            "{}",
            i.content
        );
        // The workflow is listed by its id (the file stem) with its description.
        assert!(i.content.contains("plain-flow"), "{}", i.content);
        assert!(
            i.content.contains("A single placeholder step."),
            "{}",
            i.content
        );
        assert!(!i.content.contains("(not found)"), "{}", i.content);
    }

    #[test]
    fn collector_is_the_pool_not_the_whole_catalog() {
        let (_fx, ctx) = fixture();
        let injs = roster_collector(names(&["recon"]), vec![], ctx).collect(&turn());
        let content = &injs[0].content;
        assert!(content.contains("recon"));
        // `writer` exists in the catalog but is not in the pool; no workflows
        // were pooled, so none are listed.
        assert!(!content.contains("writer"), "{content}");
        assert!(!content.contains("plain-flow"), "{content}");
        assert!(!content.contains("workflows:"), "{content}");
    }

    #[test]
    fn collector_marks_an_unknown_pool_entry_not_found() {
        let (_fx, ctx) = fixture();
        let injs = roster_collector(
            names(&["recon", "ghost-agent"]),
            names(&["no-such-flow"]),
            ctx,
        )
        .collect(&turn());
        let content = &injs[0].content;
        assert!(content.contains("- ghost-agent (not found)"), "{content}");
        assert!(content.contains("- no-such-flow (not found)"), "{content}");
        assert!(content.contains("Surveys the layout"), "{content}");
    }

    #[test]
    fn collector_addresses_workflows_by_id_not_declared_name() {
        let (_fx, ctx) = fixture();
        // "Plain Flow" is the declared name; the pool address is the stem.
        let injs = roster_collector(vec![], names(&["Plain Flow"]), ctx).collect(&turn());
        assert!(
            injs[0].content.contains("- Plain Flow (not found)"),
            "{}",
            injs[0].content
        );
    }

    #[test]
    fn collector_degrades_to_bare_names_when_the_agents_cannot_load() {
        let (fx, ctx) = fixture();
        write(&fx.global, "agents/broken.md", "no frontmatter here\n");
        let injs =
            roster_collector(names(&["recon"]), names(&["plain-flow"]), ctx).collect(&turn());
        assert_eq!(injs.len(), 1);
        let content = &injs[0].content;
        // The agent is still named, without a description (and without the
        // misleading "(not found)").
        assert!(content.contains("- recon"), "{content}");
        assert!(!content.contains("Surveys the layout"), "{content}");
        assert!(!content.contains("- recon (not found)"), "{content}");
        // The workflow half is unaffected by the agent failure.
        assert!(content.contains("A single placeholder step."), "{content}");
    }

    #[test]
    fn collector_flags_an_unparseable_pool_workflow() {
        let (fx, ctx) = fixture();
        write(
            &fx.global,
            "workflows/broken.yaml",
            "name: [unclosed\nsteps: {",
        );
        let injs = roster_collector(vec![], names(&["broken"]), ctx).collect(&turn());
        assert!(
            injs[0].content.contains("- broken (parse error)"),
            "{}",
            injs[0].content
        );
    }

    #[test]
    fn collector_keeps_descriptions_to_one_short_line() {
        let (fx, ctx) = fixture();
        let long = "word ".repeat(100);
        write(
            &fx.global,
            "agents/wordy.md",
            &format!("---\nname: wordy\ndescription: |\n  first line\n  {long}\n---\nbody\n"),
        );
        let injs = roster_collector(names(&["wordy"]), vec![], ctx).collect(&turn());
        let content = &injs[0].content;
        let entry = content.lines().find(|l| l.contains("wordy")).unwrap();
        assert!(entry.contains("first line word"), "{entry}");
        assert!(entry.ends_with("..."), "{entry}");
        assert!(entry.chars().count() < 160, "{entry}");
    }

    #[test]
    fn collector_with_an_empty_pool_emits_nothing() {
        let (_fx, ctx) = fixture();
        assert!(roster_collector(vec![], vec![], ctx)
            .collect(&turn())
            .is_empty());
    }
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;

    /// `msg.send` to `"broadcast"` (the catalog tool, over a lead's bus)
    /// reaches every participant's collector exactly once.
    #[tokio::test]
    async fn msg_send_broadcast_reaches_every_collector_once() {
        let dir = tempfile::tempdir().unwrap();
        let mailbox = Arc::new(Mailbox::new(dir.path()));
        let ctx_for = |p: &str| TurnContext {
            run_id: "r".into(),
            codename: None,
            participant: p.into(),
            turn_index: 0,
        };
        let a = MailboxCollector::new(mailbox.clone(), "worker-a");
        let b = MailboxCollector::new(mailbox.clone(), "worker-b");
        // Register both readers before the broadcast.
        assert!(a.collect(&ctx_for("worker-a")).is_empty());
        assert!(b.collect(&ctx_for("worker-b")).is_empty());

        let mut tc = rupu_tools::ToolContext::default();
        tc.services.message_bus = Some(Arc::new(rupu_fleet::Bus::for_run_dir(dir.path(), "lead")));
        let out = rupu_tools::bodies::body("msg.send", &tc)
            .unwrap()
            .invoke(
                serde_json::json!({"to":"broadcast","body":"regroup at the gateway"}),
                &tc,
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{out:?}");
        assert_eq!(out.stdout, "sent to broadcast");

        for (c, p) in [(&a, "worker-a"), (&b, "worker-b")] {
            let injs = c.collect(&ctx_for(p));
            assert_eq!(injs.len(), 1, "{p}: {injs:?}");
            assert_eq!(injs[0].cadence, Cadence::Once);
            assert_eq!(injs[0].source, format!("broadcast:{p}"));
            assert!(injs[0].content.contains("regroup at the gateway"));
            assert!(injs[0].content.contains("lead"));
            assert!(c.collect(&ctx_for(p)).is_empty(), "{p}: delivered twice");
        }
        // It is not sitting in a shared inbox either.
        assert!(mailbox.drain("broadcast").unwrap().is_empty());
    }
}
