//! The lead's roster index of its pool ([`RosterCollector`]), folded into
//! each of its turns (spec §8) as a `rupu_agent::TurnCollector`: the agent
//! loop runs it before every model call without spending a turn. The inbox
//! and the standing directives are delivered by the message-space collectors
//! every run in a space gets (`rupu_runtime::services::messaging`).
//!
//! Framing: the collector pipeline's `wrap_injection` already presents every
//! injection as attributed data, not an instruction. The `content` built here
//! is therefore the raw index only; it adds no framing of its own.
//!
//! Failure handling: a store error degrades to "no injections" (logged), the
//! same best-effort stance as `rupu_agent::CommandCollector`. A collector never
//! propagates an error or panics into the turn.

use std::sync::Arc;

use rupu_agent::{Cadence, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_tools::CatalogPort;

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
            messages: Vec::new(),
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
