//! The lead's `dispatch` / `join` tools: spawn a pool agent as a
//! process-isolated unit through the [`FleetSupervisor`], then wait for its
//! outcome.
//!
//! * `dispatch { agent, prompt }` is fail-closed. An `agent` outside the
//!   definition's `pool.agents` is refused (`ToolOutput.error`), as is any
//!   dispatch at or beyond [`MAX_DEPTH`]. The lead is depth 0, so the depth
//!   guard only bites for a future sub-lead; it is kept so that cannot be
//!   forgotten when one lands. Each unit gets a UNIQUE participant
//!   (`<agent>#<n>`): the supervisor registers a broadcast cursor under that
//!   name at dispatch, and reusing a name would let a second unit's
//!   registration swallow the first one's unseen broadcasts.
//! * `run_workflow { workflow, inputs? }` starts a pool WORKFLOW as a unit (its
//!   own `rupu workflow run` process), after five fail-closed checks that all
//!   run before anything is spawned (see [`RunWorkflowTool`]).
//! * `join { handle, timeout_secs? }` blocks (real sleeps) until the unit is
//!   terminal or the timeout elapses, so it runs on the blocking pool rather
//!   than on the executor thread driving the lead's turn.
//!
//! Error discipline matches the other fleet tools: a refused or failed
//! operation on a well-formed call is `Ok(ToolOutput { error: Some(..) })` so
//! the model sees it and can react; `Err(ToolError::InvalidInput)` is for
//! arguments that cannot be parsed at all.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::{json, Value};

use crate::supervisor::FleetSupervisor;
use crate::unit::{UnitKind, UnitSpec, UnitStatus};

/// The deepest dispatch chain allowed. The lead is depth 0 and its units are
/// depth 1; a dispatch from depth `MAX_DEPTH` or deeper is refused.
pub const MAX_DEPTH: u32 = 5;

/// How long `join` waits when `timeout_secs` is omitted.
const DEFAULT_JOIN_TIMEOUT_SECS: u64 = 300;
/// Hard ceiling on `join`'s `timeout_secs`, so one call cannot park a lead turn
/// forever; the lead can join again to keep waiting.
const MAX_JOIN_TIMEOUT_SECS: u64 = 3600;

/// What the two tools share.
struct DispatchCtx {
    sup: Arc<FleetSupervisor>,
    pool: Arc<Vec<String>>,
    engagement: Vec<String>,
    /// The dispatching participant's depth in the dispatch tree.
    depth: u32,
    /// Mints each unit's unique participant suffix.
    next: AtomicU32,
}

impl DispatchCtx {
    /// A participant id no earlier dispatch from this ctx has used.
    fn mint_participant(&self, agent: &str) -> String {
        let n = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        format!("{agent}#{n}")
    }
}

/// The lead's `dispatch` and `join` tools (the lead is depth 0).
///
/// `pool` is the definition's `pool.agents` (the only agents `dispatch` will
/// start); `engagement` is the profile set every unit is bound to.
pub fn fleet_dispatch_tools(
    sup: Arc<FleetSupervisor>,
    pool: Arc<Vec<String>>,
    engagement: Vec<String>,
) -> Vec<Arc<dyn Tool>> {
    fleet_dispatch_tools_at_depth(sup, pool, engagement, 0)
}

/// [`fleet_dispatch_tools`] for a dispatcher at `depth` in the dispatch tree.
/// A dispatcher at [`MAX_DEPTH`] or deeper gets a `dispatch` that always
/// refuses.
pub fn fleet_dispatch_tools_at_depth(
    sup: Arc<FleetSupervisor>,
    pool: Arc<Vec<String>>,
    engagement: Vec<String>,
    depth: u32,
) -> Vec<Arc<dyn Tool>> {
    let ctx = Arc::new(DispatchCtx {
        sup,
        pool,
        engagement,
        depth,
        next: AtomicU32::new(0),
    });
    vec![Arc::new(DispatchTool(ctx.clone())), Arc::new(JoinTool(ctx))]
}

/// Where `run_workflow` looks up and vets a workflow.
pub struct WorkflowToolCtx {
    /// The global rupu root (`~/.rupu`).
    pub global: PathBuf,
    /// The project's `.rupu` DIRECTORY (not the project root), when there is one.
    pub project: Option<PathBuf>,
    /// The workflow ids the flow's pool names (resolved: `workflows: all` is
    /// already the catalog's ids). The only workflows `run_workflow` will start.
    pub pool_workflows: Vec<String>,
}

/// The lead's `dispatch`, `join` and `run_workflow` tools (the lead is depth 0).
///
/// The three share ONE participant counter: an agent and a workflow with the
/// same name must never mint the same `<name>#<n>`, because the supervisor
/// registers a broadcast cursor per participant.
pub fn fleet_unit_tools(
    sup: Arc<FleetSupervisor>,
    pool: Arc<Vec<String>>,
    engagement: Vec<String>,
    workflows: WorkflowToolCtx,
) -> Vec<Arc<dyn Tool>> {
    let ctx = Arc::new(DispatchCtx {
        sup,
        pool,
        engagement,
        depth: 0,
        next: AtomicU32::new(0),
    });
    vec![
        Arc::new(DispatchTool(ctx.clone())),
        Arc::new(JoinTool(ctx.clone())),
        Arc::new(RunWorkflowTool { ctx, wf: workflows }),
    ]
}

// ---- output / argument helpers ---------------------------------------------

fn done(stdout: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: stdout.into(),
        error: None,
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A refusal / failure the model should see (not a run-aborting `Err`).
fn failed(msg: impl Into<String>) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg.into()),
        duration_ms: 0,
        derived: None,
        structured: None,
    }
}

/// A required, non-blank string argument.
fn req_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match input.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(ToolError::InvalidInput(format!(
            "{key} (non-empty string) required"
        ))),
    }
}

// ---- dispatch --------------------------------------------------------------

/// `dispatch { agent, prompt }` -> `{ "handle": "<unit id>" }`.
struct DispatchTool(Arc<DispatchCtx>);

#[async_trait]
impl Tool for DispatchTool {
    fn name(&self) -> &'static str {
        "dispatch"
    }

    fn description(&self) -> &'static str {
        "Start a pool agent as an independent unit working on a prompt, in its own \
         process. Returns a handle immediately without waiting; pass it to `join` \
         to wait for the unit's result. Only agents in the flow's pool can be \
         dispatched."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["agent", "prompt"],
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Name of a pool agent to run"
                },
                "prompt": {
                    "type": "string",
                    "description": "The unit's task: what to do and what to report back"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let agent = req_str(&input, "agent")?;
        let prompt = req_str(&input, "prompt")?;
        let c = &self.0;

        // Fail closed: the pool is the whole allowlist, checked before anything
        // is spawned.
        if !c.pool.iter().any(|a| a == agent) {
            return Ok(failed(format!(
                "dispatch refused: agent `{agent}` is not in this flow's pool"
            )));
        }
        if c.depth >= MAX_DEPTH {
            return Ok(failed(format!(
                "dispatch refused: maximum dispatch depth ({MAX_DEPTH}) reached"
            )));
        }

        let spec = UnitSpec {
            agent: agent.to_string(),
            prompt: prompt.to_string(),
            engagement: c.engagement.clone(),
            participant: c.mint_participant(agent),
            kind: UnitKind::Agent,
            inputs: vec![],
        };
        let participant = spec.participant.clone();
        Ok(match c.sup.dispatch(spec) {
            Ok(handle) => done(json!({ "handle": handle, "participant": participant }).to_string()),
            Err(e) => failed(format!("dispatch failed: {e}")),
        })
    }
}

// ---- run_workflow ------------------------------------------------------------

/// `run_workflow { workflow, inputs? }` -> `{ "handle": .., "participant": .. }`.
///
/// Starts a pool workflow as a process-isolated unit (`rupu workflow run`).
/// Fail-closed: every check below runs, in this order, before anything is
/// spawned, and a failure is a `ToolOutput.error` the lead can react to.
///
/// 1. the workflow is in the flow's pool;
/// 2. it exists and parses;
/// 3. it has no approval gate and no host / distribute placement (a unit's
///    stdin and remote hosts are not wired for it in v1);
/// 4. every agent it dispatches is in the flow's pool;
/// 5. its `inputs:` resolve (a missing required input, an undeclared one, a
///    type or enum mismatch). The unit's stderr is discarded, so this is the
///    only place the lead can learn why a workflow would refuse to start.
struct RunWorkflowTool {
    ctx: Arc<DispatchCtx>,
    wf: WorkflowToolCtx,
}

/// The tool's `inputs` object as the workflow runner's `BTreeMap<String,
/// String>`. Scalars stringify (a number or bool as the model wrote it) and
/// `null` means "not given"; an array or object has no `--input k=v` form, so
/// it is refused rather than guessed at.
fn workflow_inputs(input: &Value) -> Result<BTreeMap<String, String>, String> {
    let obj = match input.get("inputs") {
        None | Some(Value::Null) => return Ok(BTreeMap::new()),
        Some(Value::Object(o)) => o,
        Some(_) => return Err("`inputs` must be an object of name: value pairs".into()),
    };
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        match v {
            Value::Null => {}
            Value::String(s) => {
                out.insert(k.clone(), s.clone());
            }
            Value::Number(_) | Value::Bool(_) => {
                out.insert(k.clone(), v.to_string());
            }
            _ => {
                return Err(format!(
                    "input `{k}` must be a string, number or boolean, not an array or object"
                ))
            }
        }
    }
    Ok(out)
}

#[async_trait]
impl Tool for RunWorkflowTool {
    fn name(&self) -> &'static str {
        "run_workflow"
    }

    fn description(&self) -> &'static str {
        "Start a pool workflow as an independent unit, in its own process. Returns \
         a handle immediately without waiting; pass it to `join` to wait for the \
         workflow's result. Only workflows in the flow's pool can be started, and \
         only if every agent they dispatch is also in the pool. Workflows with an \
         approval gate or a host / distribute placement cannot be run as a unit."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["workflow"],
            "properties": {
                "workflow": {
                    "type": "string",
                    "description": "Id of a pool workflow to run"
                },
                "inputs": {
                    "type": "object",
                    "description": "The workflow's declared inputs, as name: value pairs",
                    "additionalProperties": { "type": ["string", "number", "boolean"] }
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let workflow = req_str(&input, "workflow")?;
        let inputs = match workflow_inputs(&input) {
            Ok(m) => m,
            Err(e) => return Err(ToolError::InvalidInput(e)),
        };
        let c = &self.ctx;

        // 1. The pool is the whole allowlist.
        if !self.wf.pool_workflows.iter().any(|w| w == workflow) {
            return Ok(failed(format!(
                "workflow '{workflow}' is not in this flow's pool"
            )));
        }
        if c.depth >= MAX_DEPTH {
            return Ok(failed(format!(
                "run_workflow refused: maximum dispatch depth ({MAX_DEPTH}) reached"
            )));
        }

        // 2. It must exist and parse.
        let Some(wf) = rupu_orchestrator::catalog::load_workflow(
            &self.wf.global,
            self.wf.project.as_deref(),
            workflow,
        ) else {
            return Ok(failed(format!(
                "unknown or unparseable workflow '{workflow}'"
            )));
        };

        // 3. No operator-in-the-loop and no remote placement in v1.
        if wf.has_approval_gate() || wf.has_placed_step() {
            return Ok(failed(format!(
                "v1 cannot run a gated or host/distribute workflow as a unit ({workflow})"
            )));
        }

        // 4. A workflow may only dispatch agents the flow's pool names.
        let pool_agents: BTreeSet<String> = c.pool.iter().cloned().collect();
        let missing: Vec<String> = wf
            .dispatched_agents()
            .difference(&pool_agents)
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Ok(failed(format!(
                "workflow '{workflow}' dispatches agents outside the pool: {}",
                missing.join(", ")
            )));
        }

        // 5. Its inputs must resolve -- the spawned process's stderr is nulled,
        //    so a bad input would otherwise surface only as an opaque failure.
        if let Err(e) = rupu_orchestrator::runner::resolve_inputs(&wf, &inputs) {
            return Ok(failed(format!(
                "invalid inputs for workflow '{workflow}': {e}"
            )));
        }

        let spec = UnitSpec {
            agent: workflow.to_string(),
            prompt: String::new(),
            engagement: c.engagement.clone(),
            participant: c.mint_participant(workflow),
            kind: UnitKind::Workflow,
            inputs: inputs.into_iter().collect(),
        };
        let participant = spec.participant.clone();
        Ok(match c.sup.dispatch(spec) {
            Ok(handle) => done(json!({ "handle": handle, "participant": participant }).to_string()),
            Err(e) => failed(format!("run_workflow failed: {e}")),
        })
    }
}

// ---- join ------------------------------------------------------------------

/// `join { handle, timeout_secs? }` -> `{ status, output?, success?, error? }`.
struct JoinTool(Arc<DispatchCtx>);

/// The model-facing JSON for a unit's status.
fn status_json(st: &UnitStatus) -> Value {
    match st {
        UnitStatus::Pending => json!({ "status": "pending" }),
        UnitStatus::Running => json!({ "status": "running" }),
        UnitStatus::Done(o) => json!({
            "status": "done",
            "success": o.success,
            "output": o.output,
        }),
        UnitStatus::Failed(why) => json!({ "status": "failed", "error": why }),
    }
}

/// `join`'s wait: absent / `null` is the default, a non-negative integer is
/// seconds (capped at [`MAX_JOIN_TIMEOUT_SECS`]), anything else is an error
/// rather than silently ignored.
fn parse_timeout(input: &Value) -> Result<Duration, ToolError> {
    match input.get("timeout_secs") {
        None | Some(Value::Null) => Ok(Duration::from_secs(DEFAULT_JOIN_TIMEOUT_SECS)),
        Some(v) => match v.as_u64() {
            Some(n) => Ok(Duration::from_secs(n.min(MAX_JOIN_TIMEOUT_SECS))),
            None => Err(ToolError::InvalidInput(
                "timeout_secs must be a non-negative integer when given".into(),
            )),
        },
    }
}

#[async_trait]
impl Tool for JoinTool {
    fn name(&self) -> &'static str {
        "join"
    }

    fn description(&self) -> &'static str {
        "Wait for a dispatched unit to finish and return its result. Reports \
         status `done` (with the unit's output and whether it succeeded), `failed`, \
         or `running` / `pending` if it has not finished within the timeout (join \
         again to keep waiting)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["handle"],
            "properties": {
                "handle": {
                    "type": "string",
                    "description": "The handle `dispatch` returned"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": MAX_JOIN_TIMEOUT_SECS,
                    "description": "How long to wait, in seconds; default 300"
                }
            }
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let handle = req_str(&input, "handle")?.to_string();
        let timeout = parse_timeout(&input)?;
        let sup = self.0.sup.clone();

        // `FleetSupervisor::join` sleeps between polls; run it on the blocking
        // pool so the executor driving this turn is never parked on it.
        let joined =
            tokio::task::spawn_blocking(move || sup.join(&handle, timeout, &|| chrono::Utc::now()))
                .await;
        Ok(match joined {
            Ok(st) => done(status_json(&st).to_string()),
            Err(e) => failed(format!("join failed: {e}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{MockUnitLauncher, UnitOutcome};

    fn tools_with(
        launcher: Arc<MockUnitLauncher>,
        dir: &std::path::Path,
        depth: u32,
    ) -> (Arc<dyn Tool>, Arc<dyn Tool>) {
        let sup = Arc::new(FleetSupervisor::new(launcher, dir.to_path_buf()));
        let mut v = fleet_dispatch_tools_at_depth(
            sup,
            Arc::new(vec!["recon".to_string(), "exploit".to_string()]),
            vec!["network".to_string()],
            depth,
        );
        let join = v.pop().unwrap();
        let dispatch = v.pop().unwrap();
        (dispatch, join)
    }

    fn done_status(out: &str) -> UnitStatus {
        UnitStatus::Done(UnitOutcome {
            output: out.into(),
            success: true,
        })
    }

    #[tokio::test]
    async fn tool_names_are_dispatch_and_join() {
        let dir = tempfile::tempdir().unwrap();
        let (d, j) = tools_with(Arc::new(MockUnitLauncher::scripted(vec![])), dir.path(), 0);
        assert_eq!((d.name(), j.name()), ("dispatch", "join"));
    }

    #[tokio::test]
    async fn dispatch_refuses_an_agent_outside_the_pool_and_spawns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (d, _) = tools_with(launcher.clone(), dir.path(), 0);
        let out = d
            .invoke(
                json!({ "agent": "not-in-pool", "prompt": "scan" }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let err = out.error.expect("refused");
        assert!(err.contains("not in this flow's pool"), "{err}");
        assert!(out.stdout.is_empty());
        assert!(launcher.spawned().is_empty(), "nothing was spawned");
    }

    #[tokio::test]
    async fn dispatch_refuses_at_max_depth_but_allows_just_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (at_max, _) = tools_with(launcher.clone(), dir.path(), MAX_DEPTH);
        let out = at_max
            .invoke(
                json!({ "agent": "recon", "prompt": "scan" }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(out.error.unwrap().contains("depth"), "refused at max depth");
        assert!(launcher.spawned().is_empty());

        let (below, _) = tools_with(launcher.clone(), dir.path(), MAX_DEPTH - 1);
        let out = below
            .invoke(
                json!({ "agent": "recon", "prompt": "scan" }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(launcher.spawned().len(), 1);
    }

    #[tokio::test]
    async fn every_dispatch_gets_a_unique_participant_and_the_def_engagement() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (d, _) = tools_with(launcher.clone(), dir.path(), 0);
        for agent in ["recon", "recon", "exploit"] {
            let out = d
                .invoke(
                    json!({ "agent": agent, "prompt": "go" }),
                    &ToolContext::default(),
                )
                .await
                .unwrap();
            assert!(out.error.is_none(), "{:?}", out.error);
        }
        let spawned = launcher.spawned();
        let participants: Vec<_> = spawned.iter().map(|s| s.participant.as_str()).collect();
        assert_eq!(participants, ["recon#1", "recon#2", "exploit#3"]);
        assert!(spawned.iter().all(|s| s.engagement == ["network"]));
        assert_eq!(spawned[0].agent, "recon");
        assert_eq!(spawned[0].prompt, "go");
    }

    #[tokio::test]
    async fn dispatch_returns_the_handle_and_join_returns_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![
            UnitStatus::Running,
            done_status("found 3 hosts"),
        ]));
        let (d, j) = tools_with(launcher, dir.path(), 0);
        let out = d
            .invoke(
                json!({ "agent": "recon", "prompt": "scan" }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let handle = serde_json::from_str::<Value>(&out.stdout).unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(handle.starts_with("run_"), "{handle}");

        let out = j
            .invoke(
                json!({ "handle": handle, "timeout_secs": 5 }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["status"], "done");
        assert_eq!(v["success"], true);
        assert_eq!(v["output"], "found 3 hosts");
    }

    #[tokio::test]
    async fn join_times_out_on_a_unit_that_never_finishes_and_reports_it_running() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (d, j) = tools_with(launcher, dir.path(), 0);
        let out = d
            .invoke(
                json!({ "agent": "recon", "prompt": "scan" }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let handle = serde_json::from_str::<Value>(&out.stdout).unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let out = j
            .invoke(
                json!({ "handle": handle, "timeout_secs": 0 }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["status"], "running");
        assert!(v.get("output").is_none());
    }

    #[tokio::test]
    async fn join_of_an_unknown_handle_reports_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (_, j) = tools_with(Arc::new(MockUnitLauncher::scripted(vec![])), dir.path(), 0);
        let out = j
            .invoke(json!({ "handle": "run_nope" }), &ToolContext::default())
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["status"], "failed");
        assert!(v["error"].as_str().unwrap().contains("unknown"), "{v}");
    }

    #[tokio::test]
    async fn malformed_arguments_are_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let (d, j) = tools_with(Arc::new(MockUnitLauncher::scripted(vec![])), dir.path(), 0);
        let ctx = ToolContext::default();
        for bad in [
            json!({}),
            json!({ "agent": "recon" }),
            json!({ "agent": " ", "prompt": "x" }),
        ] {
            assert!(
                matches!(d.invoke(bad, &ctx).await, Err(ToolError::InvalidInput(_))),
                "dispatch needs a non-empty agent and prompt"
            );
        }
        for bad in [
            json!({}),
            json!({ "handle": "h", "timeout_secs": "soon" }),
            json!({ "handle": "h", "timeout_secs": -1 }),
        ] {
            assert!(
                matches!(j.invoke(bad, &ctx).await, Err(ToolError::InvalidInput(_))),
                "join needs a handle and an integer timeout"
            );
        }
    }

    #[test]
    fn join_timeout_defaults_and_caps() {
        assert_eq!(
            parse_timeout(&json!({})).unwrap(),
            Duration::from_secs(DEFAULT_JOIN_TIMEOUT_SECS)
        );
        assert_eq!(
            parse_timeout(&json!({ "timeout_secs": 7 })).unwrap(),
            Duration::from_secs(7)
        );
        assert_eq!(
            parse_timeout(&json!({ "timeout_secs": 999_999 })).unwrap(),
            Duration::from_secs(MAX_JOIN_TIMEOUT_SECS)
        );
    }

    // ---- run_workflow --------------------------------------------------------

    const BENIGN_WF: &str = "name: sweep\nsteps:\n  - id: only\n    agent: recon\n    prompt: hi\n";

    /// A catalog with each workflow written under `<dir>/global/workflows/`,
    /// and the three unit tools over it. The pool is the agent `recon` and
    /// exactly the workflows named in `pool_workflows`.
    fn wf_tools(
        launcher: Arc<MockUnitLauncher>,
        dir: &std::path::Path,
        workflows: &[(&str, &str)],
        pool_workflows: &[&str],
    ) -> (Arc<dyn Tool>, Arc<dyn Tool>) {
        let global = dir.join("global");
        std::fs::create_dir_all(global.join("workflows")).unwrap();
        for (id, yaml) in workflows {
            std::fs::write(global.join("workflows").join(format!("{id}.yaml")), yaml).unwrap();
        }
        let sup = Arc::new(FleetSupervisor::new(launcher, dir.join("run")));
        let mut v = fleet_unit_tools(
            sup,
            Arc::new(vec!["recon".to_string()]),
            vec!["network".to_string()],
            WorkflowToolCtx {
                global,
                project: None,
                pool_workflows: pool_workflows.iter().map(|s| s.to_string()).collect(),
            },
        );
        let run_workflow = v.pop().unwrap();
        let _join = v.pop().unwrap();
        let dispatch = v.pop().unwrap();
        (dispatch, run_workflow)
    }

    /// Invoke `run_workflow` and assert it refused with `needle`, spawning nothing.
    async fn assert_refused(
        launcher: &MockUnitLauncher,
        tool: &Arc<dyn Tool>,
        input: Value,
        needle: &str,
    ) {
        let out = tool.invoke(input, &ToolContext::default()).await.unwrap();
        let err = out.error.expect("refused");
        assert!(err.contains(needle), "{err}");
        assert!(out.stdout.is_empty());
        assert!(launcher.spawned().is_empty(), "nothing was spawned");
    }

    #[tokio::test]
    async fn tool_names_include_run_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![]));
        let sup = Arc::new(FleetSupervisor::new(launcher, dir.path().to_path_buf()));
        let v = fleet_unit_tools(
            sup,
            Arc::new(vec![]),
            vec![],
            WorkflowToolCtx {
                global: dir.path().to_path_buf(),
                project: None,
                pool_workflows: vec![],
            },
        );
        let names: Vec<_> = v.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["dispatch", "join", "run_workflow"]);
    }

    #[tokio::test]
    async fn run_workflow_refuses_a_workflow_outside_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        // `sweep` exists on disk and would pass every other check, but the
        // pool does not name it.
        let (_, rw) = wf_tools(launcher.clone(), dir.path(), &[("sweep", BENIGN_WF)], &[]);
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "sweep" }),
            "workflow 'sweep' is not in this flow's pool",
        )
        .await;
    }

    #[tokio::test]
    async fn run_workflow_refuses_an_unknown_or_unparseable_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (_, rw) = wf_tools(
            launcher.clone(),
            dir.path(),
            &[("broken", "name: [not a workflow")],
            // `ghost` is pool-listed with no file; `broken` has a file that
            // does not parse.
            &["ghost", "broken"],
        );
        for id in ["ghost", "broken"] {
            assert_refused(
                &launcher,
                &rw,
                json!({ "workflow": id }),
                &format!("unknown or unparseable workflow '{id}'"),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn run_workflow_refuses_a_gated_or_placed_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (_, rw) = wf_tools(
            launcher.clone(),
            dir.path(),
            &[
                (
                    "gated",
                    "name: gated\nsteps:\n  - id: g\n    approval:\n      required: true\n",
                ),
                (
                    "inline-gated",
                    "name: ig\nsteps:\n  - id: s\n    agent: recon\n    prompt: hi\n    approval:\n      required: true\n",
                ),
                (
                    "placed",
                    "name: placed\nsteps:\n  - id: s\n    agent: recon\n    prompt: hi\n    host: worker-1\n",
                ),
            ],
            &["gated", "inline-gated", "placed"],
        );
        for id in ["gated", "inline-gated", "placed"] {
            assert_refused(
                &launcher,
                &rw,
                json!({ "workflow": id }),
                &format!("v1 cannot run a gated or host/distribute workflow as a unit ({id})"),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn run_workflow_refuses_a_workflow_that_dispatches_an_agent_outside_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        // `recon` is in the pool; `exploit` and `rogue` are not.
        let wf = "name: wide\nsteps:\n  - id: a\n    agent: recon\n    prompt: x\n  \
                  - id: b\n    agent: rogue\n    prompt: y\n  \
                  - id: c\n    agent: exploit\n    prompt: z\n";
        let (_, rw) = wf_tools(launcher.clone(), dir.path(), &[("wide", wf)], &["wide"]);
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "wide" }),
            "workflow 'wide' dispatches agents outside the pool: exploit, rogue",
        )
        .await;
    }

    #[tokio::test]
    async fn run_workflow_refuses_inputs_the_workflow_would_reject() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let wf = "name: typed\n\
                  inputs:\n  \
                    topic: { type: string, required: true }\n  \
                    depth: { type: int, default: 1 }\n\
                  steps:\n  - id: s\n    agent: recon\n    prompt: \"{{ inputs.topic }}\"\n";
        let (_, rw) = wf_tools(launcher.clone(), dir.path(), &[("typed", wf)], &["typed"]);
        // A missing required input.
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "typed" }),
            "input `topic` is required",
        )
        .await;
        // An undeclared input.
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "typed", "inputs": { "topic": "x", "extra": "y" } }),
            "input `extra` is not declared",
        )
        .await;
        // A type mismatch.
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "typed", "inputs": { "topic": "x", "depth": "deep" } }),
            "not a valid int",
        )
        .await;
    }

    #[tokio::test]
    async fn run_workflow_checks_run_in_the_documented_order() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        // Gated AND dispatching an out-of-pool agent AND missing a required
        // input: only the first failing check is reported.
        let wf = "name: all-bad\n\
                  inputs:\n  need: { type: string, required: true }\n\
                  steps:\n  - id: g\n    agent: rogue\n    prompt: x\n    approval:\n      required: true\n";
        let (_, rw) = wf_tools(
            launcher.clone(),
            dir.path(),
            &[("all-bad", wf)],
            &["all-bad"],
        );
        assert_refused(
            &launcher,
            &rw,
            json!({ "workflow": "all-bad" }),
            "gated or host/distribute",
        )
        .await;

        // Not in the pool beats every later check too.
        let (_, rw2) = wf_tools(launcher.clone(), dir.path(), &[("all-bad", wf)], &[]);
        assert_refused(
            &launcher,
            &rw2,
            json!({ "workflow": "all-bad" }),
            "is not in this flow's pool",
        )
        .await;
    }

    #[tokio::test]
    async fn run_workflow_spawns_a_workflow_unit_with_the_inputs_and_engagement() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let wf = "name: typed\n\
                  inputs:\n  \
                    topic: { type: string, required: true }\n  \
                    depth: { type: int, default: 1 }\n  \
                    loud: { type: bool }\n\
                  steps:\n  - id: s\n    agent: recon\n    prompt: \"{{ inputs.topic }}\"\n";
        let (_, rw) = wf_tools(launcher.clone(), dir.path(), &[("typed", wf)], &["typed"]);
        let out = rw
            .invoke(
                // A number and a bool are stringified; null means "not given".
                json!({ "workflow": "typed",
                        "inputs": { "topic": "ssh", "depth": 3, "loud": true, "x": null } }),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        // `x` is null (omitted), so it is not an undeclared input.
        assert!(out.error.is_none(), "{:?}", out.error);
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert!(v["handle"].as_str().unwrap().starts_with("run_"), "{v}");
        assert_eq!(v["participant"], "typed#1");

        let spawned = launcher.spawned();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].kind, UnitKind::Workflow);
        assert_eq!(spawned[0].agent, "typed");
        assert_eq!(spawned[0].prompt, "");
        assert_eq!(spawned[0].engagement, ["network"]);
        assert_eq!(spawned[0].participant, "typed#1");
        assert_eq!(
            spawned[0].inputs,
            [
                ("depth".to_string(), "3".to_string()),
                ("loud".to_string(), "true".to_string()),
                ("topic".to_string(), "ssh".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn an_agent_and_a_workflow_of_the_same_name_get_distinct_participants() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        // The pool has an agent `recon` AND a workflow `recon`.
        let (d, rw) = wf_tools(
            launcher.clone(),
            dir.path(),
            &[("recon", BENIGN_WF)],
            &["recon"],
        );
        let ctx = ToolContext::default();
        let a = d
            .invoke(json!({ "agent": "recon", "prompt": "go" }), &ctx)
            .await
            .unwrap();
        let w = rw
            .invoke(json!({ "workflow": "recon" }), &ctx)
            .await
            .unwrap();
        assert!(a.error.is_none() && w.error.is_none(), "{a:?} {w:?}");
        let participants: Vec<_> = launcher
            .spawned()
            .iter()
            .map(|s| s.participant.clone())
            .collect();
        assert_eq!(participants, ["recon#1", "recon#2"]);
    }

    #[tokio::test]
    async fn run_workflow_malformed_arguments_are_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![]));
        let (_, rw) = wf_tools(
            launcher.clone(),
            dir.path(),
            &[("sweep", BENIGN_WF)],
            &["sweep"],
        );
        let ctx = ToolContext::default();
        for bad in [
            json!({}),
            json!({ "workflow": " " }),
            json!({ "workflow": "sweep", "inputs": "topic=x" }),
            json!({ "workflow": "sweep", "inputs": { "k": ["a"] } }),
            json!({ "workflow": "sweep", "inputs": { "k": { "a": 1 } } }),
        ] {
            assert!(
                matches!(rw.invoke(bad, &ctx).await, Err(ToolError::InvalidInput(_))),
                "run_workflow needs a workflow id and an object of scalar inputs"
            );
        }
        assert!(launcher.spawned().is_empty());
    }
}
