//! Roster read tools for the agentiflow lead: enumerate, inspect and search the
//! agents and workflows it can draw on, as read-only [`rupu_tools::Tool`] impls.
//!
//! Every tool returns COMPACT JSON (names, descriptions, shapes -- never the raw
//! `.md` / `.yaml`) so the lead's context stays small.
//!
//! Error discipline matches `tools.rs`: a failure on a well-formed call is
//! `Ok(ToolOutput { error: Some(..) })` so the model sees it and can react;
//! `Err(ToolError::InvalidInput)` is reserved for arguments that cannot be
//! parsed. In particular `rupu_agent::load_agents` aborts on ONE unparseable
//! `.md`, so each tool catches that `Err` and reports it instead of propagating
//! (and `catalog.search` still searches the workflows when the agents fail to
//! load). `rupu_orchestrator::list_workflow_summaries` is already tolerant: a
//! bad workflow is listed with its `parse_error`.
//!
//! [`RosterCollector`] is the ambient counterpart: a compact index of the
//! pool's agents and workflows folded into every lead turn, so the lead knows
//! what it can dispatch without spending a turn on `agents.list`.

use async_trait::async_trait;
use rupu_agent::{AgentSpec, Cadence, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_orchestrator::{list_workflow_summaries, WorkflowSummary};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Where the roster tools look. Mirrors the `(global, project)` convention of
/// `rupu_agent::load_agents` and `rupu_orchestrator::list_workflow_summaries`:
/// both append `agents/` / `workflows/` to each root themselves.
pub struct RosterCtx {
    /// The global rupu root (`~/.rupu`).
    pub global: PathBuf,
    /// The project's `.rupu` DIRECTORY (not the project root), when there is one.
    pub project: Option<PathBuf>,
}

/// The five read-only roster tools, each holding a clone of `ctx`.
pub fn roster_tools(ctx: Arc<RosterCtx>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(AgentsList(ctx.clone())),
        Arc::new(AgentsGet(ctx.clone())),
        Arc::new(WorkflowsList(ctx.clone())),
        Arc::new(WorkflowsGet(ctx.clone())),
        Arc::new(CatalogSearch(ctx)),
    ]
}

// ---- helpers ----------------------------------------------------------------

/// A successful result carrying compact JSON.
pub(crate) fn done(v: Value) -> ToolOutput {
    ToolOutput {
        stdout: v.to_string(),
        error: None,
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A failure the model should see (not a run-aborting `Err`).
pub(crate) fn failed(msg: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg.into()),
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A required, non-blank string argument.
pub(crate) fn req_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match input.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(ToolError::InvalidInput(format!(
            "{key} (non-empty string) required"
        ))),
    }
}

/// Load every agent, or the message naming why that was not possible.
fn load(ctx: &RosterCtx) -> Result<Vec<AgentSpec>, String> {
    rupu_agent::load_agents(&ctx.global, ctx.project.as_deref())
        .map_err(|e| format!("could not load agents: {e}"))
}

fn workflows(ctx: &RosterCtx) -> Vec<WorkflowSummary> {
    list_workflow_summaries(&ctx.global, ctx.project.as_deref())
}

/// The compact row `agents.list` returns for one agent.
fn agent_row(a: &AgentSpec) -> Value {
    json!({
        "name": a.name,
        "description": a.description,
        "tools": a.tools,
    })
}

/// The compact row `workflows.list` returns for one workflow.
fn workflow_row(w: &WorkflowSummary) -> Value {
    json!({
        "id": w.id,
        "name": w.name,
        "description": w.description,
        "scope": w.scope,
        "step_count": w.step_count,
        "parse_error": w.parse_error,
    })
}

// ---- agents.list ------------------------------------------------------------

/// `agents.list` -> `[{name, description, tools}]` over global + project agents.
struct AgentsList(Arc<RosterCtx>);

#[async_trait]
impl Tool for AgentsList {
    fn name(&self) -> &'static str {
        "agents.list"
    }

    fn description(&self) -> &'static str {
        "List the agents you can draw on (global and project): each one's name, \
         description and declared tools. `tools: null` means the agent uses the \
         default tool set, not none. Use agents.get for one agent's detail."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(match load(&self.0) {
            Ok(agents) => done(Value::Array(agents.iter().map(agent_row).collect())),
            Err(msg) => failed(msg),
        })
    }
}

// ---- agents.get -------------------------------------------------------------

/// `agents.get { name }` -> one agent's detail (no system prompt).
struct AgentsGet(Arc<RosterCtx>);

#[async_trait]
impl Tool for AgentsGet {
    fn name(&self) -> &'static str {
        "agents.get"
    }

    fn description(&self) -> &'static str {
        "Get one agent's detail by name: description, provider, model, declared \
         tools, the agents it may dispatch, and its permission mode."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "The agent's name, as agents.list shows it" }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = req_str(&input, "name")?;
        let agents = match load(&self.0) {
            Ok(a) => a,
            Err(msg) => return Ok(failed(msg)),
        };
        Ok(match agents.iter().find(|a| a.name == name) {
            Some(a) => done(json!({
                "name": a.name,
                "description": a.description,
                "provider": a.provider,
                "model": a.model,
                "tools": a.tools,
                "dispatchable_agents": a.dispatchable_agents,
                "permission_mode": a.permission_mode,
                "max_turns": a.max_turns,
            })),
            None => failed(format!("agent not found: {name}")),
        })
    }
}

// ---- workflows.list ---------------------------------------------------------

/// `workflows.list` -> `[{id, name, description, scope, step_count, parse_error}]`.
struct WorkflowsList(Arc<RosterCtx>);

#[async_trait]
impl Tool for WorkflowsList {
    fn name(&self) -> &'static str {
        "workflows.list"
    }

    fn description(&self) -> &'static str {
        "List the workflows you can run (global and project). `id` is the runnable \
         identifier (the file stem); `name` is the declared display name. A \
         workflow that failed to parse is listed with its parse_error."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(done(Value::Array(
            workflows(&self.0).iter().map(workflow_row).collect(),
        )))
    }
}

// ---- workflows.get ----------------------------------------------------------

/// `workflows.get { id }` -> one workflow's summary, addressed by its `id`.
struct WorkflowsGet(Arc<RosterCtx>);

#[async_trait]
impl Tool for WorkflowsGet {
    fn name(&self) -> &'static str {
        "workflows.get"
    }

    fn description(&self) -> &'static str {
        "Get one workflow's summary by its `id` (the runnable file stem workflows.list \
         shows, not the declared name): description, scope, declared inputs, step count."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "The workflow's id, as workflows.list shows it" }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = req_str(&input, "id")?;
        Ok(match workflows(&self.0).into_iter().find(|w| w.id == id) {
            Some(w) => {
                let mut row = workflow_row(&w);
                row["input_keys"] = json!(w.input_keys);
                done(row)
            }
            None => failed(format!("workflow not found: {id}")),
        })
    }
}

// ---- catalog.search ---------------------------------------------------------

/// `catalog.search { query }` -> `{hits: [{kind, id|name, description}], warnings?}`.
struct CatalogSearch(Arc<RosterCtx>);

/// Case-insensitive substring match of an already-lowercased `needle` over
/// any of `fields`.
fn matches(needle: &str, fields: &[Option<&str>]) -> bool {
    fields
        .iter()
        .flatten()
        .any(|f| f.to_lowercase().contains(needle))
}

#[async_trait]
impl Tool for CatalogSearch {
    fn name(&self) -> &'static str {
        "catalog.search"
    }

    fn description(&self) -> &'static str {
        "Search the agent and workflow catalog: a case-insensitive substring match \
         over agent name/description and workflow id/name/description. Each hit is \
         tagged kind=agent|workflow. `warnings` is present when part of the catalog \
         could not be read."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string", "description": "Substring to look for (case-insensitive)" }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let needle = req_str(&input, "query")?.to_lowercase();
        let mut hits: Vec<Value> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        match load(&self.0) {
            Ok(agents) => {
                for a in &agents {
                    if matches(&needle, &[Some(&a.name), a.description.as_deref()]) {
                        hits.push(json!({
                            "kind": "agent",
                            "name": a.name,
                            "description": a.description,
                        }));
                    }
                }
            }
            // One bad agent file must not hide the workflows from the search.
            Err(msg) => warnings.push(msg),
        }

        for w in &workflows(&self.0) {
            if matches(
                &needle,
                &[Some(&w.id), Some(&w.name), w.description.as_deref()],
            ) {
                hits.push(json!({
                    "kind": "workflow",
                    "id": w.id,
                    "name": w.name,
                    "description": w.description,
                }));
            }
        }

        let mut out = json!({ "hits": hits });
        if !warnings.is_empty() {
            out["warnings"] = json!(warnings);
        }
        Ok(done(out))
    }
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
/// catalog as `name -- description`, re-asserted on every turn.
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
/// cannot be loaded (one unparseable `.md` aborts `load_agents`) the pool
/// agents are listed by name with no descriptions; a pool entry the catalog does
/// not hold is marked `(not found)`. Nothing here panics or returns an error
/// into the turn. A pool with no agents and no workflows emits nothing.
pub struct RosterCollector {
    pool_agents: Vec<String>,
    pool_workflows: Vec<String>,
    ctx: Arc<RosterCtx>,
}

impl RosterCollector {
    pub fn new(pool_agents: Vec<String>, pool_workflows: Vec<String>, ctx: Arc<RosterCtx>) -> Self {
        Self {
            pool_agents,
            pool_workflows,
            ctx,
        }
    }

    fn agent_lines(&self) -> Vec<String> {
        if self.pool_agents.is_empty() {
            return Vec::new();
        }
        let catalog = match load(&self.ctx) {
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
        let catalog = workflows(&self.ctx);
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
    ctx: Arc<RosterCtx>,
) -> Arc<dyn TurnCollector> {
    Arc::new(RosterCollector::new(pool_agents, pool_workflows, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const RECON: &str = "---\nname: recon\ndescription: Surveys the layout of a repository.\ntools: [read_file, grep]\n---\nYou survey a repository.\n";
    const WRITER: &str = "---\nname: writer\n---\nYou write text.\n";
    const ONE_STEP: &str = "name: Plain Flow\ndescription: A single placeholder step.\ninputs:\n  topic:\n    type: string\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n";

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Two agents (one described) + one workflow whose stem differs from its
    /// declared name.
    fn fixture() -> (tempfile::TempDir, Arc<RosterCtx>) {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        write(&global, "agents/recon.md", RECON);
        write(&global, "agents/writer.md", WRITER);
        write(&global, "workflows/plain-flow.yaml", ONE_STEP);
        let ctx = Arc::new(RosterCtx {
            global,
            project: None,
        });
        (tmp, ctx)
    }

    fn tool(ctx: &Arc<RosterCtx>, name: &str) -> Arc<dyn Tool> {
        roster_tools(ctx.clone())
            .into_iter()
            .find(|t| t.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
    }

    async fn call(ctx: &Arc<RosterCtx>, name: &str, input: Value) -> ToolOutput {
        tool(ctx, name)
            .invoke(input, &ToolContext::default())
            .await
            .unwrap()
    }

    fn json_of(out: &ToolOutput) -> Value {
        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        serde_json::from_str(&out.stdout).unwrap()
    }

    #[test]
    fn exposes_the_five_tools() {
        let (_tmp, ctx) = fixture();
        let mut names: Vec<_> = roster_tools(ctx).iter().map(|t| t.name()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "agents.get",
                "agents.list",
                "catalog.search",
                "workflows.get",
                "workflows.list"
            ]
        );
    }

    #[tokio::test]
    async fn agents_list_returns_names_descriptions_and_tools() {
        let (_tmp, ctx) = fixture();
        let v = json_of(&call(&ctx, "agents.list", json!({})).await);
        let rows = v.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let recon = rows.iter().find(|r| r["name"] == "recon").unwrap();
        assert_eq!(recon["description"], "Surveys the layout of a repository.");
        assert_eq!(recon["tools"], json!(["read_file", "grep"]));
        let writer = rows.iter().find(|r| r["name"] == "writer").unwrap();
        assert!(writer["description"].is_null());
        // Compact: never the system prompt / raw file.
        assert!(!v.to_string().contains("You survey"));
    }

    #[tokio::test]
    async fn agents_list_tolerates_a_bad_agent_file() {
        let (_tmp, ctx) = fixture();
        write(&ctx.global, "agents/broken.md", "no frontmatter here\n");
        let out = call(&ctx, "agents.list", json!({})).await;
        let err = out.error.expect("a bad .md is reported, not propagated");
        assert!(err.starts_with("could not load agents:"), "{err}");
        assert!(err.contains("broken.md"), "names the bad file: {err}");
    }

    #[tokio::test]
    async fn agents_get_returns_detail_and_errors_when_unknown() {
        let (_tmp, ctx) = fixture();
        let v = json_of(&call(&ctx, "agents.get", json!({"name": "recon"})).await);
        assert_eq!(v["name"], "recon");
        assert_eq!(v["description"], "Surveys the layout of a repository.");
        assert_eq!(v["tools"], json!(["read_file", "grep"]));
        assert!(!v.to_string().contains("You survey"));

        let out = call(&ctx, "agents.get", json!({"name": "nope"})).await;
        assert_eq!(out.error.as_deref(), Some("agent not found: nope"));

        let err = tool(&ctx, "agents.get")
            .invoke(json!({}), &ToolContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn workflows_list_returns_the_workflow_by_its_id() {
        let (_tmp, ctx) = fixture();
        let v = json_of(&call(&ctx, "workflows.list", json!({})).await);
        let rows = v.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "plain-flow");
        assert_eq!(rows[0]["name"], "Plain Flow");
        assert_eq!(rows[0]["description"], "A single placeholder step.");
        assert_eq!(rows[0]["scope"], "global");
        assert_eq!(rows[0]["step_count"], 1);
        assert!(rows[0]["parse_error"].is_null());
    }

    #[tokio::test]
    async fn workflows_list_still_lists_a_broken_file() {
        let (_tmp, ctx) = fixture();
        write(
            &ctx.global,
            "workflows/broken.yaml",
            "name: [unclosed\nsteps: {",
        );
        let v = json_of(&call(&ctx, "workflows.list", json!({})).await);
        let broken = v
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "broken")
            .unwrap();
        assert!(broken["parse_error"].is_string());
    }

    #[tokio::test]
    async fn workflows_get_addresses_by_id_not_declared_name() {
        let (_tmp, ctx) = fixture();
        let v = json_of(&call(&ctx, "workflows.get", json!({"id": "plain-flow"})).await);
        assert_eq!(v["id"], "plain-flow");
        assert_eq!(v["name"], "Plain Flow");
        assert_eq!(v["input_keys"], json!(["topic"]));

        // The declared name is display-only: it is not an address.
        let out = call(&ctx, "workflows.get", json!({"id": "Plain Flow"})).await;
        assert_eq!(out.error.as_deref(), Some("workflow not found: Plain Flow"));
    }

    #[tokio::test]
    async fn catalog_search_finds_the_agent_and_not_unrelated_entries() {
        let (_tmp, ctx) = fixture();
        let v = json_of(&call(&ctx, "catalog.search", json!({"query": "RECON"})).await);
        let hits = v["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0]["kind"], "agent");
        assert_eq!(hits[0]["name"], "recon");
        assert!(v.get("warnings").is_none());
    }

    #[tokio::test]
    async fn catalog_search_matches_workflows_by_id_name_and_description() {
        let (_tmp, ctx) = fixture();
        for q in ["plain-flow", "plain flow", "placeholder"] {
            let v = json_of(&call(&ctx, "catalog.search", json!({"query": q})).await);
            let hits = v["hits"].as_array().unwrap();
            assert_eq!(hits.len(), 1, "{q}: {hits:?}");
            assert_eq!(hits[0]["kind"], "workflow");
            assert_eq!(hits[0]["id"], "plain-flow");
        }
    }

    #[tokio::test]
    async fn catalog_search_still_searches_workflows_when_an_agent_is_broken() {
        let (_tmp, ctx) = fixture();
        write(&ctx.global, "agents/broken.md", "no frontmatter here\n");
        let v = json_of(&call(&ctx, "catalog.search", json!({"query": "plain"})).await);
        assert_eq!(v["hits"].as_array().unwrap().len(), 1);
        let w = v["warnings"].as_array().unwrap();
        assert_eq!(w.len(), 1);
        assert!(w[0].as_str().unwrap().contains("broken.md"));
    }

    #[tokio::test]
    async fn project_scope_is_the_rupu_dir_and_shadows_global() {
        let (tmp, ctx) = fixture();
        let project = tmp.path().join("proj/.rupu");
        write(
            &project,
            "agents/recon.md",
            "---\nname: recon\ndescription: Project-local recon.\n---\nbody\n",
        );
        write(&project, "workflows/local-flow.yaml", ONE_STEP);
        let ctx = Arc::new(RosterCtx {
            global: ctx.global.clone(),
            project: Some(project),
        });
        let agents = json_of(&call(&ctx, "agents.list", json!({})).await);
        assert_eq!(agents.as_array().unwrap().len(), 2);
        let recon = agents
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "recon")
            .unwrap();
        assert_eq!(recon["description"], "Project-local recon.");
        let wfs = json_of(&call(&ctx, "workflows.list", json!({})).await);
        let ids: Vec<_> = wfs
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["local-flow", "plain-flow"]);
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
        let (_tmp, ctx) = fixture();
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
        let (_tmp, ctx) = fixture();
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
        let (_tmp, ctx) = fixture();
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
        let (_tmp, ctx) = fixture();
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
        let (_tmp, ctx) = fixture();
        write(&ctx.global, "agents/broken.md", "no frontmatter here\n");
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
        let (_tmp, ctx) = fixture();
        write(
            &ctx.global,
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
        let (_tmp, ctx) = fixture();
        let long = "word ".repeat(100);
        write(
            &ctx.global,
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
        let (_tmp, ctx) = fixture();
        assert!(roster_collector(vec![], vec![], ctx)
            .collect(&turn())
            .is_empty());
    }
}
