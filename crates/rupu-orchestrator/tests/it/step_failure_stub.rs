//! W3 §6 test 4 (`step_failure_stub_sequence_unchanged`): a workflow step
//! whose agent cannot start — its file is missing, or its provider cannot be
//! built — fails with the same run record, step error, typed cause and
//! executor events it did before W3b turned the old error-stub providers
//! (`agent_load_error_stub` / `provider_build_error_stub`) into
//! `AssembleError`s, and its transcript reads the same: `run_start` →
//! `model_limits` notice → `user_message` → `turn_start` → (`outcome`) →
//! `run_complete`, with the same text. The expected values below were
//! captured from the stub-provider implementation.
//!
//! Deliberately NOT carried over (the stubs ran the agent loop against a
//! fake provider, so the transcript claimed things that never happened): a
//! `tool_grant` line for a run that offered no tools, a rung-3 `recovery`
//! line whose "add fallbacks" hint could never apply (a stub step built no
//! fallback ladder), and, for a missing agent, the step's rendered prompt
//! recorded as its system prompt.

use std::sync::{Arc, Mutex};

use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts};
use rupu_orchestrator::{RunStore, Workflow};
use serde_json::{json, Value};

#[derive(Default)]
struct CollectSink(Mutex<Vec<Event>>);

impl EventSink for CollectSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.0.lock().unwrap().push(ev.clone());
    }
}

/// Keys whose values change run to run.
const VOLATILE: &[&str] = &[
    "workflow_path",
    "at",
    "started_at",
    "finished_at",
    "ended_at",
    "ts",
    "timestamp",
    "duration_ms",
    "run_id",
    "agent_run_id",
    "transcript_path",
    "workspace_id",
    "workspace_path",
    "id",
    "sha256",
];

fn scrub(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for k in VOLATILE {
                if m.contains_key(*k) {
                    m.insert((*k).to_string(), Value::String("<volatile>".into()));
                }
            }
            for (_, x) in m.iter_mut() {
                scrub(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

struct Outcome {
    error: String,
    step: Value,
    events: Vec<Value>,
    transcript: Vec<Value>,
}

const WF: &str = r#"
name: stubbed
steps:
  - id: only
    agent: AGENT
    actions: []
    prompt: "go"
"#;

async fn run(agent_file: Option<&str>) -> Outcome {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    std::fs::create_dir_all(global.join("agents")).unwrap();
    if let Some(body) = agent_file {
        std::fs::write(global.join("agents").join("AGENT.md"), body).unwrap();
    }
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let store = Arc::new(RunStore::new(global.join("runs")));
    let sink = Arc::new(CollectSink::default());
    let workflow = Workflow::parse(WF).unwrap();
    let factory = crate::support::step_factory(&global, workflow.clone());
    let run_id = "run_stubbed";
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow,
        inputs: Default::default(),
        workspace_id: "ws_stub".into(),
        workspace_path: workspace,
        transcript_dir: tmp.path().join("transcripts"),
        factory,
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF.to_string()),
        resume_from: None,
        run_id_override: Some(run_id.to_string()),
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_services: None,
        pause: None,
        naming: None,
    };
    let error = match run_workflow(opts).await {
        Ok(_) => panic!("the step must fail"),
        Err(e) => e.to_string(),
    };
    let record = store.load(run_id).unwrap();
    let mut step = serde_json::json!({
        "status": record.status,
        "error": record.error_message,
        "cause": record.cause,
        "steps": store.read_step_results(run_id).unwrap(),
    });
    scrub(&mut step);
    let transcript_path = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            Event::AgentStarted {
                transcript_path, ..
            } => Some(transcript_path.clone()),
            _ => None,
        })
        .expect("an AgentStarted event");
    let events = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|e| {
            let mut v = serde_json::to_value(e).unwrap();
            scrub(&mut v);
            v
        })
        .collect();
    let transcript = std::fs::read_to_string(&transcript_path)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            scrub(&mut v);
            v
        })
        .collect();
    Outcome {
        error,
        step,
        events,
        transcript,
    }
}

const MISSING_ERROR: &str = "agent `AGENT` not found or failed to load: agent not found: AGENT\n  Checked the project agents dir (.rupu/agents/) and the global agents dir.";

const AUTH_ERROR: &str = "auth config error: anthropic: missing credential for provider anthropic (no credentials configured for anthropic. Run: rupu auth login --account anthropic --mode <api-key|sso>): configure with `rupu auth login --provider anthropic` or set the env var the provider expects\n  Run: rupu auth login --provider anthropic --mode <api-key|sso>";

const V: &str = "<volatile>";

/// The executor events of a step that failed before its agent ran, as the
/// stub implementation emitted them.
fn events(provider: &str, model: &str, error: &str, cause: Value) -> Vec<Value> {
    let mut failed = json!({
        "error": error, "run_id": V, "step_id": "only", "type": "step_failed",
    });
    if !cause.is_null() {
        failed["cause"] = cause;
    }
    vec![
        json!({"event_version": 1, "run_id": V, "started_at": V, "type": "run_started", "workflow_path": V}),
        json!({"agent": "AGENT", "codename": "tan-tower/gazelle", "kind": "linear", "run_id": V, "step_id": "only", "type": "step_started"}),
        json!({"note": null, "run_id": V, "step_id": "only", "transcript_path": V, "type": "step_working"}),
        json!({"agent": "AGENT", "agent_run_id": V, "codename": "tan-tower/gazelle", "model": model, "provider": provider, "run_id": V, "step_id": "only", "transcript_path": V, "type": "agent_started"}),
        failed,
        json!({"error": error, "finished_at": V, "run_id": V, "type": "run_failed"}),
    ]
}

fn auth_cause() -> Value {
    json!({
        "class": "provider_error",
        "detail": AUTH_ERROR,
        "error_class": "auth",
        "id": V,
        "severity": "error",
        "title": "provider error · auth",
        "wire": {"message": AUTH_ERROR},
    })
}

fn run_start(provider: &str, model: &str, system_prompt: Option<&str>) -> Value {
    let mut data = json!({
        "agent": "AGENT", "codename": "tan-tower/gazelle", "customer": null, "mode": "bypass",
        "model": model, "provider": provider, "run_id": V, "schema": 2, "started_at": V,
        "workspace_id": V,
    });
    if let Some(p) = system_prompt {
        data["system_prompt"] = json!(p);
    }
    json!({"data": data, "type": "run_start"})
}

fn preamble() -> Vec<Value> {
    vec![
        json!({"data": {"kind": "model_limits", "message": "input unknown · output unknown (provider max) · compaction off — no limit source"}, "type": "notice"}),
        json!({"data": {"content": "go"}, "type": "user_message"}),
        json!({"data": {"turn_idx": 0}, "type": "turn_start"}),
    ]
}

#[tokio::test]
async fn step_failure_stub_sequence_unchanged() {
    // The agent file is missing.
    let missing = run(None).await;
    let error = format!("agent failure in step only: {MISSING_ERROR}");
    assert_eq!(missing.error, error);
    assert_eq!(
        missing.step,
        json!({"cause": null, "error": error, "status": "failed", "steps": []})
    );
    assert_eq!(
        missing.events,
        events("unresolved", "-", &error, Value::Null)
    );
    let mut transcript = vec![run_start("unresolved", "-", None)];
    transcript.extend(preamble());
    transcript.push(json!({"data": {"duration_ms": V, "error": MISSING_ERROR, "run_id": V, "status": "error", "total_tokens": 0}, "type": "run_complete"}));
    assert_eq!(missing.transcript, transcript, "{}", dump(&missing));

    // The agent loads; its provider has no credential.
    let unbuildable = run(Some(
        "---\nname: AGENT\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nhi\n",
    ))
    .await;
    let error = format!("agent failure in step only: provider: {AUTH_ERROR}");
    assert_eq!(unbuildable.error, error);
    assert_eq!(
        unbuildable.step,
        json!({"cause": auth_cause(), "error": error, "status": "failed", "steps": []})
    );
    assert_eq!(
        unbuildable.events,
        events("anthropic", "claude-sonnet-4-6", &error, auth_cause())
    );
    let mut transcript = vec![run_start(
        "anthropic",
        "claude-sonnet-4-6",
        Some("hi\n\n\nYour call sign in this run is `gazelle` (crew `tan-tower`). Sign any comments, issues or PR notes you post with it."),
    )];
    transcript.extend(preamble());
    transcript.push(json!({"data": {"outcome": auth_cause(), "turn_idx": 0}, "type": "outcome"}));
    transcript.push(json!({"data": {"duration_ms": V, "error": format!("provider: {AUTH_ERROR}"), "outcome": auth_cause(), "run_id": V, "status": "error", "total_tokens": 0}, "type": "run_complete"}));
    assert_eq!(unbuildable.transcript, transcript, "{}", dump(&unbuildable));
}

/// An agent naming an undeclared provider fails the same way, with the
/// config error `rupu run` gives (it used to be the factory's "unknown
/// provider" behind a wrong `rupu auth login` hint): a preflight failure, no
/// provider request, no cause.
#[tokio::test]
async fn a_step_on_an_undeclared_provider_fails_with_the_config_error() {
    let o = run(Some(
        "---\nname: AGENT\nprovider: nosuch\nmodel: m-1\n---\nhi\n",
    ))
    .await;
    assert!(
        o.error.starts_with(
            "agent failure in step only: provider 'nosuch' is not a built-in provider"
        ),
        "{}",
        o.error
    );
    assert!(
        !o.error.contains("rupu auth login --provider"),
        "{}",
        o.error
    );
    assert_eq!(o.step["cause"], Value::Null);
    assert_eq!(o.events[3]["provider"], "nosuch");
    assert_eq!(o.events[3]["model"], "m-1");
    let last = o.transcript.last().unwrap();
    assert_eq!(last["type"], "run_complete");
    assert_eq!(last["data"]["status"], "error");
    assert!(last["data"].get("outcome").is_none(), "{last}");
}

fn dump(o: &Outcome) -> String {
    let mut s = format!("error: {}\nstep: {}\nevents:\n", o.error, o.step);
    for e in &o.events {
        s.push_str(&format!("  {e}\n"));
    }
    s.push_str("transcript:\n");
    for e in &o.transcript {
        s.push_str(&format!("  {e}\n"));
    }
    s
}
