//! `dispatch`: start a child run (spec W7 §3.1), and the legacy names that
//! map onto it (§3.2).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{fold_inputs, parse_inputs, parse_timeout, workflow_input_strings, SYNC_WAIT};
use crate::descriptor::{Alias, Effect, Service, ToolDescriptor};
use crate::output::{failed, ok, req_str};
use crate::ports::{
    ChildHandle, ChildKind, ChildRequest, ChildState, ChildStatus, ChildTarget, JoinSet, Launcher,
    Until,
};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};

pub static DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: "dispatch",
    aliases: &[
        Alias::any("dispatch_agent"),
        Alias::any("dispatch_agents_parallel"),
        Alias::any("run_workflow"),
    ],
    effect: Effect::Spawn,
    needs: &[Service::Launcher],
    uses: &[],
    wildcard: true,
    description: "Start a child run and get a handle back at once; the child works on its own. \
     `kind` picks how it runs: `subagent` (the default: in this process, for a quick delegation \
     such as reviewing a file), `agent` (its own `rupu run` process, for long or heavy work), or \
     `workflow` (its own `rupu workflow run`). Name an agent from your dispatchable agents, a \
     workflow from your dispatchable workflows, or pass a `file` that `workflows.generate` \
     returned. Call `join` with the handle to collect the result, or pass `wait: true` to start \
     the child and wait for it in one call. Children you never join are stopped when you finish.",
    input_schema: schema,
};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["subagent", "agent", "workflow"],
                "description": "How the child runs; default `subagent`"
            },
            "name": {
                "type": "string",
                "description": "The agent (or, for kind `workflow`, the workflow) to run"
            },
            "file": {
                "type": "string",
                "description": "Kind `workflow` only: a workflow file `workflows.generate` returned, instead of `name`"
            },
            "prompt": {
                "type": "string",
                "description": "An agent child's task: what to do and what to report back"
            },
            "inputs": {
                "type": "object",
                "description": "A workflow's declared inputs as name: value pairs; for an agent, structured inputs added to its prompt",
                "additionalProperties": true
            },
            "wait": {
                "type": "boolean",
                "description": "Wait for the child to finish and return its result too; default false"
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 0,
                "maximum": super::MAX_JOIN_TIMEOUT_SECS,
                "description": "With `wait`: how long to wait, in seconds; default 300"
            }
        }
    })
}

/// The `dispatch` tool and its aliases.
#[derive(Debug, Default, Clone)]
pub struct DispatchTool;

/// Which shape a call speaks: the canonical one, or a legacy name's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    New,
    /// `dispatch_agent {agent, prompt, inputs?}`: a sub-agent, run to the end.
    DispatchAgent,
    /// `dispatch_agents_parallel {agents: [...], max_parallel?}`.
    DispatchAgentsParallel,
    /// The agentiflow `dispatch {agent, prompt}`: an agent process, async.
    FlowDispatch,
    /// `run_workflow {workflow, inputs?}`.
    RunWorkflow,
}

/// The model only sees `dispatch`, so a prompt written for a legacy name
/// arrives as `dispatch` in the legacy shape: an `agents` list is
/// `dispatch_agents_parallel`; `agent` without `kind` is the agentiflow's
/// `dispatch` inside a flow (where that tool existed) and `dispatch_agent`
/// anywhere else.
fn form(called_as: Option<&str>, input: &Value, in_flow: bool) -> Form {
    match called_as {
        Some("dispatch_agent") => Form::DispatchAgent,
        Some("dispatch_agents_parallel") => Form::DispatchAgentsParallel,
        Some("run_workflow") => Form::RunWorkflow,
        _ if input.get("agents").is_some_and(Value::is_array) => Form::DispatchAgentsParallel,
        _ if input.get("agent").is_some() && input.get("kind").is_none() => {
            if in_flow {
                Form::FlowDispatch
            } else {
                Form::DispatchAgent
            }
        }
        _ => Form::New,
    }
}

#[async_trait]
impl Tool for DispatchTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DESCRIPTOR
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let form = form(
            ctx.call.called_as.as_deref(),
            &input,
            ctx.identity.launch.flow.is_some(),
        );
        let Some(launcher) = ctx.services.launcher.clone() else {
            return Ok(failed(
                "launcher_not_configured: this run can't start child runs".to_string(),
            )
            .timed(started));
        };
        let out = match form {
            Form::New => new_form(&input, ctx, &launcher).await?,
            Form::DispatchAgent => dispatch_agent(input, ctx, &launcher).await?,
            Form::DispatchAgentsParallel => dispatch_agents_parallel(input, ctx, &launcher).await?,
            Form::FlowDispatch => {
                let agent = req_str(&input, "agent")?;
                let prompt = req_str(&input, "prompt")?;
                let req = ChildRequest {
                    kind: ChildKind::Agent,
                    target: ChildTarget::Name(agent.to_string()),
                    prompt: Some(prompt.to_string()),
                    inputs: BTreeMap::new(),
                };
                match launcher.launch(req, ctx).await {
                    Ok(h) => participant_json(&h),
                    Err(e) => failed(format!("dispatch refused: {}", e.message)),
                }
            }
            Form::RunWorkflow => {
                let workflow = req_str(&input, "workflow")?;
                let inputs = parse_inputs(&input)?;
                workflow_input_strings(&inputs).map_err(ToolError::InvalidInput)?;
                let req = ChildRequest {
                    kind: ChildKind::Workflow,
                    target: ChildTarget::Name(workflow.to_string()),
                    prompt: None,
                    inputs,
                };
                match launcher.launch(req, ctx).await {
                    Ok(h) => participant_json(&h),
                    Err(e) => failed(e.message),
                }
            }
        };
        Ok(out.timed(started))
    }
}

/// The legacy flow shape: `{handle, participant}`.
fn participant_json(h: &ChildHandle) -> ToolOutput {
    let participant = h.address.clone().unwrap_or_else(|| h.name.clone());
    ok(json!({ "handle": h.handle, "participant": participant }).to_string())
}

/// `{handle, kind, name, run_id, codename?, address?, transcript?}`.
fn handle_json(h: &ChildHandle) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("handle".into(), json!(h.handle));
    m.insert("kind".into(), json!(h.kind.as_str()));
    m.insert("name".into(), json!(h.name));
    m.insert("run_id".into(), json!(h.run_id));
    if let Some(c) = &h.codename {
        m.insert("codename".into(), json!(c));
    }
    if let Some(a) = &h.address {
        m.insert("address".into(), json!(a));
    }
    if let Some(t) = &h.transcript {
        m.insert("transcript".into(), json!(t.display().to_string()));
    }
    m
}

/// The canonical form: `{kind?, name | file, prompt?, inputs?, wait?,
/// timeout_secs?}`.
async fn new_form(
    input: &Value,
    ctx: &ToolContext,
    launcher: &Arc<dyn Launcher>,
) -> Result<ToolOutput, ToolError> {
    let kind = match input.get("kind") {
        None | Some(Value::Null) => ChildKind::Subagent,
        Some(Value::String(w)) => ChildKind::parse(w).ok_or_else(|| {
            ToolError::InvalidInput(format!(
                "kind must be `subagent`, `agent` or `workflow`, not `{w}`"
            ))
        })?,
        Some(_) => {
            return Err(ToolError::InvalidInput(
                "kind must be a string when given".into(),
            ))
        }
    };
    let name = crate::output::opt_str(input, "name")?;
    let file = crate::output::opt_str(input, "file")?;
    let target = match (name, file) {
        (Some(n), None) => ChildTarget::Name(n),
        (None, Some(f)) if kind == ChildKind::Workflow => ChildTarget::File(f.into()),
        (None, Some(_)) => {
            return Err(ToolError::InvalidInput(
                "`file` is for kind `workflow` only".into(),
            ))
        }
        (Some(_), Some(_)) => {
            return Err(ToolError::InvalidInput(
                "give `name` or `file`, not both".into(),
            ))
        }
        (None, None) => {
            return Err(ToolError::InvalidInput(
                "name (non-empty string) required".into(),
            ))
        }
    };
    let inputs = parse_inputs(input)?;
    let prompt = match kind {
        ChildKind::Workflow => {
            workflow_input_strings(&inputs).map_err(ToolError::InvalidInput)?;
            None
        }
        ChildKind::Subagent | ChildKind::Agent => Some(req_str(input, "prompt")?.to_string()),
    };
    let wait = match input.get("wait") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(ToolError::InvalidInput(
                "wait must be a boolean when given".into(),
            ))
        }
    };
    let timeout = parse_timeout(input)?;
    let req = ChildRequest {
        kind,
        target,
        prompt,
        inputs,
    };
    let handle = match launcher.launch(req, ctx).await {
        Ok(h) => h,
        Err(e) => return Ok(failed(e.to_string())),
    };
    let mut body = handle_json(&handle);
    if wait {
        let status = launcher
            .join(
                &ctx.identity,
                JoinSet::Handles(vec![handle.handle.clone()]),
                timeout,
                Until::All,
            )
            .await;
        if let Some(s) = status.first() {
            body.insert("result".into(), s.to_json());
        }
    }
    Ok(ok(Value::Object(body).to_string()))
}

/// One sub-agent's request in the legacy shapes.
#[derive(Deserialize, Clone)]
struct AgentRequest {
    #[serde(default)]
    id: String,
    agent: String,
    prompt: String,
    #[serde(default)]
    inputs: Option<Value>,
}

impl AgentRequest {
    fn child(&self) -> Result<ChildRequest, ToolError> {
        let inputs = parse_inputs(&json!({ "inputs": self.inputs }))?;
        Ok(ChildRequest {
            kind: ChildKind::Subagent,
            target: ChildTarget::Name(self.agent.clone()),
            prompt: Some(fold_inputs(&self.prompt, &inputs)),
            inputs: BTreeMap::new(),
        })
    }
}

/// `dispatch_agent`'s result for a child that ran: the pre-W7
/// `DispatchOutcome` JSON (the web `ToolCard` parses it).
fn outcome_json(s: &ChildStatus) -> Value {
    let success = s.state == ChildState::Done && s.success == Some(true);
    let tokens = s.tokens.map(|t| t.input + t.output).unwrap_or(0);
    let mut body = json!({
        "ok": success,
        "agent": s.name,
        "output": if success { s.output.clone().unwrap_or_default() } else { String::new() },
        "findings": [],
        "tokens_used": tokens,
        "duration_ms": s.duration_ms.unwrap_or(0),
        "transcript_path": s.transcript.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        "sub_run_id": s.run_id,
        "codename": s.codename,
    });
    if let Some(error) = s.error.as_ref().filter(|_| !success) {
        body["error"] = json!(error);
    }
    body
}

/// A child that ran to an answer (which may be a failure) is `Done`; one that
/// could not run (its agent missing, its provider unbuildable) or was stopped
/// is not, and the legacy tools reported that as a dispatch failure.
fn ran(s: &ChildStatus) -> bool {
    s.state == ChildState::Done
}

/// `dispatch_agent {agent, prompt, inputs?}` = `dispatch {kind: subagent,
/// wait: true}`, answering in the old shape.
async fn dispatch_agent(
    input: Value,
    ctx: &ToolContext,
    launcher: &Arc<dyn Launcher>,
) -> Result<ToolOutput, ToolError> {
    let req: AgentRequest =
        serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    let handle = match launcher.launch(req.child()?, ctx).await {
        Ok(h) => h,
        Err(e) => return Ok(failed(e.to_string())),
    };
    let status = launcher
        .join(
            &ctx.identity,
            JoinSet::Handles(vec![handle.handle]),
            SYNC_WAIT,
            Until::All,
        )
        .await;
    let Some(s) = status.into_iter().next() else {
        return Ok(failed("dispatch failed: the child was lost"));
    };
    if !ran(&s) {
        return Ok(failed(format!(
            "dispatch failed: {}",
            s.error.unwrap_or_else(|| s.state.as_str().to_string())
        )));
    }
    let body = outcome_json(&s);
    Ok(ok(
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    ))
}

#[derive(Deserialize)]
struct ParallelInput {
    agents: Vec<AgentRequest>,
    #[serde(default)]
    max_parallel: Option<usize>,
}

/// `dispatch_agents_parallel` = N × `dispatch {kind: subagent}` + `join`,
/// at most `max_parallel` at once, every request checked before any child
/// starts; answers with the old aggregated map, keyed by request id.
async fn dispatch_agents_parallel(
    input: Value,
    ctx: &ToolContext,
    launcher: &Arc<dyn Launcher>,
) -> Result<ToolOutput, ToolError> {
    let i: ParallelInput =
        serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    if i.agents.is_empty() {
        return Ok(failed("no_agents: `agents` must be non-empty"));
    }
    let mut seen = BTreeSet::new();
    for req in &i.agents {
        if !seen.insert(req.id.clone()) {
            return Ok(failed(format!(
                "duplicate_id: `{}` appears more than once in `agents`",
                req.id
            )));
        }
    }
    let mut pending: VecDeque<(String, ChildRequest)> = VecDeque::new();
    for req in &i.agents {
        let child = req.child()?;
        if let Err(e) = launcher.check(&child, ctx).await {
            return Ok(failed(format!("{e} (id `{}`)", req.id)));
        }
        pending.push_back((req.id.clone(), child));
    }

    let max_parallel = i.max_parallel.unwrap_or(i.agents.len()).max(1);
    let mut in_flight: Vec<(String, String)> = Vec::new();
    let mut done: BTreeMap<String, Value> = BTreeMap::new();
    let mut all_succeeded = true;
    let agent_of: BTreeMap<String, String> = i
        .agents
        .iter()
        .map(|r| (r.id.clone(), r.agent.clone()))
        .collect();
    let mut record = |id: String, entry: Value, success: bool| {
        if !success {
            all_succeeded = false;
        }
        done.insert(id, entry);
    };
    loop {
        while in_flight.len() < max_parallel {
            let Some((id, child)) = pending.pop_front() else {
                break;
            };
            match launcher.launch(child, ctx).await {
                Ok(h) => in_flight.push((id, h.handle)),
                Err(e) => {
                    let agent = agent_of.get(&id).cloned().unwrap_or_default();
                    record(
                        id,
                        json!({ "ok": false, "agent": agent, "error": e.to_string() }),
                        false,
                    );
                }
            }
        }
        if in_flight.is_empty() {
            break;
        }
        let handles: Vec<String> = in_flight.iter().map(|(_, h)| h.clone()).collect();
        let statuses = launcher
            .join(
                &ctx.identity,
                JoinSet::Handles(handles),
                SYNC_WAIT,
                Until::Any,
            )
            .await;
        for s in statuses.into_iter().filter(|s| s.state.is_terminal()) {
            let Some(pos) = in_flight.iter().position(|(_, h)| *h == s.handle) else {
                continue;
            };
            let (id, _) = in_flight.remove(pos);
            if ran(&s) {
                let entry = outcome_json(&s);
                let success = entry["ok"] == json!(true);
                record(id, entry, success);
            } else {
                let error = s.error.clone().unwrap_or_else(|| s.state.as_str().into());
                record(
                    id,
                    json!({ "ok": false, "agent": s.name, "error": error }),
                    false,
                );
            }
        }
    }

    let body = json!({
        "ok": all_succeeded,
        "results": Value::Object(done.into_iter().collect()),
        "all_succeeded": all_succeeded,
    });
    Ok(ok(
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    ))
}
