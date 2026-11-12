//! Durable Board work. Authority belongs to the runtime port, never tool JSON.
use crate::{Effect, Service, Tool, ToolContext, ToolDescriptor, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_fleet::work_queue::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkOwnerKind {
    Session,
    Workflow,
    Standalone,
    Agentiflow,
}

/// Trusted runtime handoff. Never deserialized from model or MCP input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkOwnerBinding {
    pub global: PathBuf,
    pub kind: WorkOwnerKind,
    pub id: String,
    pub directory: PathBuf,
    pub message_space: PathBuf,
}

/// Runtime implementation owns the lifecycle lease, exact owner checks, and
/// actor. Tools cannot supply or replace any of those through their arguments.
pub trait WorkQueuePort: Send + Sync {
    fn binding(&self) -> WorkOwnerBinding;
    fn actor(&self) -> WorkActor;
    /// Revoke fresh writes at every runner exit, including cancellation/unwind.
    fn finish_execution(&self);
    fn list(&self, query: WorkPageQuery) -> Result<WorkPage, WorkError>;
    fn get(&self, work_id: &str) -> Result<WorkItem, WorkError>;
    fn timeline(&self, query: WorkTimelineQuery) -> Result<WorkTimelinePage, WorkError>;
    fn apply(&self, command: WorkCommand, source: WorkSource) -> Result<WorkReply, WorkError>;
}

const fn descriptor(
    name: &'static str,
    effect: Effect,
    description: &'static str,
    schema: fn() -> Value,
) -> ToolDescriptor {
    ToolDescriptor {
        name,
        aliases: &[],
        effect,
        needs: &[Service::MessageBus, Service::WorkQueue],
        uses: &[],
        wildcard: true,
        description,
        input_schema: schema,
    }
}
pub static POST: ToolDescriptor = descriptor("board.work_post", Effect::Record,
    "Post durable queued work in this run's Board. Does not launch or wake an agent. Read work_list for the exact queue ID/revision first; use null expected_queue_id only for an uninitialized queue. Retry an uncertain result with the identical request_id and command.", || schema("post"));
pub static LIST: ToolDescriptor = descriptor("board.work_list", Effect::Read,
    "Read a bounded page of durable Board work, newest first, with exact queue identity/revision and count scope. Missing storage is not evidence about legacy Board posts or leases. Pass the returned next cursor unchanged.", || schema("list"));
pub static GET: ToolDescriptor = descriptor("board.work_get", Effect::Read,
    "Read one work item's current state, exact attempt and recorded result. A reported result is not independent verification. Earlier events are in work_timeline.", || schema("get"));
pub static TIMELINE: ToolDescriptor = descriptor("board.work_timeline", Effect::Read,
    "Read one work item's immutable recorded events, oldest first. Each event preserves the exact execution and source call; missing transcripts do not erase that provenance.", || schema("timeline"));
pub static CLAIM: ToolDescriptor = descriptor("board.work_claim", Effect::Record,
    "Claim queued work as this exact runtime execution. Returns a new work_attempt_id. Claiming does not start execution or grant additional permissions. Use the current item revision and exact queue ID.", || schema("claim"));
pub static START: ToolDescriptor = descriptor("board.work_start", Effect::Record,
    "Explicitly report that this execution is starting its claimed work. Requires this execution's exact work_attempt_id. Does not dispatch a child or run commands.", || schema("start"));
pub static BLOCK: ToolDescriptor = descriptor("board.work_block", Effect::Record,
    "Report this execution's working attempt blocked with a concrete reason. Does not stop or interrupt an agent.", || schema("block"));
pub static RESUME: ToolDescriptor = descriptor(
    "board.work_resume",
    Effect::Record,
    "Report that this execution resumes its blocked attempt. Does not launch or wake an agent.",
    || schema("resume"),
);
pub static COMPLETE: ToolDescriptor = descriptor("board.work_complete", Effect::Record,
    "Record this execution's result summary for its working or blocked attempt. This is an executor-reported outcome, not independent verification. Evidence stays in the immutable Queue event.", || schema("complete"));
pub static FAIL: ToolDescriptor = descriptor("board.work_fail", Effect::Record,
    "Record why this execution's claimed, working or blocked attempt failed. A later execution cannot inherit it; ask the operator to requeue abandoned work.", || schema("fail"));
pub static DESCRIPTORS: &[&ToolDescriptor] = &[
    &POST, &LIST, &GET, &TIMELINE, &CLAIM, &START, &BLOCK, &RESUME, &COMPLETE, &FAIL,
];

fn schema(action: &str) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    if matches!(action, "list" | "timeline") {
        properties.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":MAX_PAGE_LIMIT,"default":DEFAULT_PAGE_LIMIT}));
        properties.insert("cursor".into(), json!({"type":["object","null"],"description":"Exact next object returned by this tool; do not construct or change it."}));
    }
    if action == "list" {
        properties.insert("state".into(), json!({"type":"string","enum":["queued","claimed","working","blocked","completed","failed","cancelled"]}));
        properties.insert("claimant_run_id".into(), json!({"type":"string"}));
    } else if action != "post" {
        properties.insert("work_id".into(), json!({"type":"string"}));
        required.push("work_id");
    }
    if !matches!(action, "list" | "get" | "timeline") {
        for (name, value) in [
            ("request_id", json!({"type":"string"})),
            ("expected_queue_id", json!({"type":["string","null"]})),
            ("expected_revision", json!({"type":"integer","minimum":0})),
        ] {
            properties.insert(name.into(), value);
            required.push(name);
        }
        if !matches!(action, "post" | "claim") {
            properties.insert("work_attempt_id".into(), json!({"type":"string"}));
            required.push("work_attempt_id");
        }
        for field in match action {
            "post" => vec!["title", "instructions"],
            "block" => vec!["reason"],
            "complete" | "fail" => vec!["summary"],
            _ => vec![],
        } {
            properties.insert(field.into(), json!({"type":"string"}));
            required.push(field);
        }
    }
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListInput {
    state: Option<WorkState>,
    claimant_run_id: Option<String>,
    limit: Option<usize>,
    cursor: Option<WorkPagePosition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetInput {
    work_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineInput {
    work_id: String,
    limit: Option<usize>,
    cursor: Option<WorkTimelinePosition>,
}
fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))
}
fn limit(limit: Option<usize>) -> Result<usize, ToolError> {
    let n = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
    if !(1..=MAX_PAGE_LIMIT).contains(&n) {
        return Err(ToolError::InvalidInput("limit must be 1..200".into()));
    }
    Ok(n)
}
fn command(action: &str, input: Value) -> Result<WorkCommand, ToolError> {
    let Value::Object(mut fields) = input else {
        return Err(ToolError::InvalidInput("expected an object".into()));
    };
    let mut root = serde_json::Map::new();
    for key in ["request_id", "expected_queue_id", "expected_revision"] {
        let value = fields
            .remove(key)
            .ok_or_else(|| ToolError::InvalidInput(format!("missing {key}")))?;
        root.insert(key.into(), value);
    }
    if fields.contains_key("action") {
        return Err(ToolError::InvalidInput(
            "action is fixed by the tool".into(),
        ));
    }
    fields.insert("action".into(), json!(action));
    root.insert("operation".into(), Value::Object(fields));
    parse(Value::Object(root))
}
fn result(value: Value) -> ToolOutput {
    let mut out = crate::output::ok_json(value.clone());
    out.structured = Some(value);
    out
}
fn error(error: WorkError) -> ToolOutput {
    let mut out = result(
        json!({"error":{"code":error.code(),"message":error.to_string(),"retry":error.retry(),"request_id":error.request_id()},"execution_effect":"none"}),
    );
    out.error = Some(error.to_string());
    out
}

pub fn body(name: &str) -> Option<Arc<dyn Tool>> {
    DESCRIPTORS
        .iter()
        .find(|d| d.name == name)
        .map(|d| Arc::new(WorkTool(d)) as Arc<dyn Tool>)
}
struct WorkTool(&'static ToolDescriptor);
#[async_trait]
impl Tool for WorkTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        self.0
    }
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let port = ctx.services.work_queue.clone().ok_or_else(|| {
            ToolError::Execution("durable Queue is unavailable for this execution".into())
        })?;
        let action = self.0.name.trim_start_matches("board.work_");
        let mut request_id = None;
        let task: Box<dyn FnOnce() -> Result<Value, WorkError> + Send> = match action {
            "list" => {
                let p: ListInput = parse(input)?;
                let n = limit(p.limit)?;
                Box::new(move || {
                    port.list(WorkPageQuery {
                        filters: WorkFilters {
                            state: p.state,
                            claimant_run_id: p.claimant_run_id,
                        },
                        limit: n,
                        cursor: p.cursor,
                    })
                    .map(|v| json!(v))
                })
            }
            "get" => {
                let p: GetInput = parse(input)?;
                Box::new(move || port.get(&p.work_id).map(|v| json!(v)))
            }
            "timeline" => {
                let p: TimelineInput = parse(input)?;
                let n = limit(p.limit)?;
                Box::new(move || {
                    port.timeline(WorkTimelineQuery {
                        work_id: p.work_id,
                        limit: n,
                        cursor: p.cursor,
                    })
                    .map(|v| json!(v))
                })
            }
            _ => {
                let command = command(action, input)?;
                request_id = Some(command.request_id.clone());
                let source = WorkSource {
                    turn_index: ctx.call.turn_index.ok_or_else(|| {
                        ToolError::Execution("runtime turn identity unavailable".into())
                    })?,
                    call_sequence: ctx.call.call_sequence.ok_or_else(|| {
                        ToolError::Execution("runtime call sequence unavailable".into())
                    })?,
                    tool_call_id: ctx.call.tool_call_id.clone().ok_or_else(|| {
                        ToolError::Execution("runtime call identity unavailable".into())
                    })?,
                };
                Box::new(move || port.apply(command, source).map(|v| json!(v)))
            }
        };
        match tokio::task::spawn_blocking(task).await {
            Ok(Ok(value)) => Ok(result(value)),
            Ok(Err(failure)) => {
                let mut out = error(failure);
                if let Some(id) = request_id {
                    if let Some(value) = out.structured.as_mut() {
                        if value["error"]["request_id"].is_null() {
                            value["error"]["request_id"] = json!(id);
                        }
                        out.stdout = value.to_string();
                    }
                }
                Ok(out)
            }
            Err(_) => match request_id {
                Some(request_id) => Ok(error(WorkError::Reconciliation {
                    request_id,
                    message: "Queue task interrupted; replay the identical request to reconcile"
                        .into(),
                })),
                None => Err(ToolError::Execution(
                    "Queue observation interrupted; refresh the observation".into(),
                )),
            },
        }
    }
}
