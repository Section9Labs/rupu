//! End-to-end Slice 3c: a synced placed step's file edit reaches a later
//! local step; a fan-out disjoint-edit case merges without conflict.

#![deny(clippy::all)]

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::{AgentRunOpts, RunError};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitDispatch, UnitDispatcher, UnitOutcome,
    WorkspaceConflict, WorkspaceDelta,
};
use rupu_orchestrator::{RunStatus, RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Shared: panic factory for workflows where all steps run remotely
// ---------------------------------------------------------------------------

/// Panics if `build_opts_for_step` is ever called. Used in the fanout test
/// where every unit is routed to a remote dispatcher — no local dispatch
/// should occur.
struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        _rendered_prompt: String,
        _run_id: String,
        _workspace_id: String,
        _workspace_path: std::path::PathBuf,
        _transcript_path: std::path::PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!("PanicFactory: build_opts_for_step must not be called in this test");
    }
}

// ---------------------------------------------------------------------------
// Test 1 helpers — placed sync step followed by a local downstream step
// ---------------------------------------------------------------------------

/// Simulates a remote worker that edits `foo.txt`.
///
/// `dispatch_unit` records whether the unit received a `workspace_path` and
/// returns a `WorkspaceDelta` with `changed = ["foo.txt"]` and
/// `payload = b"EDITED"`.
///
/// `apply_workspace_deltas` writes each delta's payload to
/// `workspace_path/<changed[0]>`, simulating the codec apply that the real
/// rupu-workspace crate performs.
struct PlacedSyncDispatcher {
    saw_workspace_path: Mutex<bool>,
}

impl PlacedSyncDispatcher {
    fn new() -> Self {
        Self {
            saw_workspace_path: Mutex::new(false),
        }
    }

    fn did_see_workspace_path(&self) -> bool {
        *self.saw_workspace_path.lock().unwrap()
    }
}

#[async_trait]
impl UnitDispatcher for PlacedSyncDispatcher {
    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        _host: &str,
    ) -> Result<UnitOutcome, RunError> {
        *self.saw_workspace_path.lock().unwrap() = unit.workspace_path.is_some();
        Ok(UnitOutcome {
            output: "REMOTE-EDITED".to_string(),
            success: true,
            error: None,
            workspace_delta: Some(WorkspaceDelta {
                changed: vec!["foo.txt".to_string()],
                deleted: vec![],
                payload: b"EDITED".to_vec(),
            }),
        })
    }

    async fn apply_workspace_deltas(
        &self,
        workspace_path: &Path,
        deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        for delta in deltas {
            if let Some(filename) = delta.changed.first() {
                std::fs::write(workspace_path.join(filename), &delta.payload)
                    .expect("PlacedSyncDispatcher: apply write failed");
            }
        }
        Ok(())
    }
}

/// Local downstream factory. Reads `workspace_path/foo.txt` at build time
/// and emits `"file: <content>"` as the step output — proving the synced
/// edit was applied before this step ran.
struct ReadingFactory {
    saw_content: Mutex<Option<String>>,
}

impl ReadingFactory {
    fn new() -> Self {
        Self {
            saw_content: Mutex::new(None),
        }
    }

    fn saw_content(&self) -> Option<String> {
        self.saw_content.lock().unwrap().clone()
    }
}

#[async_trait]
impl StepFactory for ReadingFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let content = std::fs::read_to_string(workspace_path.join("foo.txt"))
            .unwrap_or_else(|_| "<missing>".to_string());
        *self.saw_content.lock().unwrap() = Some(content.clone());
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("file: {content}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "reader".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext::default(),
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: false,
            suppress_stream_stdout: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: step_id.to_string(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2 helpers — fan-out with three disjoint file edits
// ---------------------------------------------------------------------------

const FANOUT_FILES: [&str; 3] = ["x.txt", "y.txt", "z.txt"];
const FANOUT_CONTENTS: [&[u8]; 3] = [b"content-x", b"content-y", b"content-z"];

/// Fan-out dispatcher: unit at `index` N returns a delta for the N-th file
/// in `FANOUT_FILES` with the N-th content from `FANOUT_CONTENTS`.
///
/// `apply_workspace_deltas` writes all deltas' payloads to their respective
/// files under `workspace_path` in a single call.
struct FanoutSyncDispatcher {
    apply_call_count: Mutex<usize>,
}

impl FanoutSyncDispatcher {
    fn new() -> Self {
        Self {
            apply_call_count: Mutex::new(0),
        }
    }

    fn apply_call_count(&self) -> usize {
        *self.apply_call_count.lock().unwrap()
    }
}

#[async_trait]
impl UnitDispatcher for FanoutSyncDispatcher {
    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        _host: &str,
    ) -> Result<UnitOutcome, RunError> {
        let idx = unit.index;
        let filename = FANOUT_FILES[idx].to_string();
        let content = FANOUT_CONTENTS[idx].to_vec();
        Ok(UnitOutcome {
            output: format!("out-{idx}"),
            success: true,
            error: None,
            workspace_delta: Some(WorkspaceDelta {
                changed: vec![filename],
                deleted: vec![],
                payload: content,
            }),
        })
    }

    async fn apply_workspace_deltas(
        &self,
        workspace_path: &Path,
        deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        *self.apply_call_count.lock().unwrap() += 1;
        for delta in deltas {
            if let Some(filename) = delta.changed.first() {
                std::fs::write(workspace_path.join(filename), &delta.payload)
                    .expect("FanoutSyncDispatcher: apply write failed");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Workflow YAML fixtures
// ---------------------------------------------------------------------------

/// Two-step workflow: placed + synced step followed by a local step.
const WF_PLACED_SYNC_LOCAL: &str = r#"
name: ws-sync-e2e-placed
steps:
  - id: edit
    agent: editor
    prompt: "edit foo.txt"
    host: worker-1
    workspace: sync
  - id: verify
    agent: verifier
    prompt: "check: {{ steps.edit.output }}"
"#;

/// Fan-out over three items with `distribute:` + `workspace: sync`.
const WF_FANOUT_SYNC: &str = r#"
name: ws-sync-e2e-fanout
steps:
  - id: edit
    agent: editor
    for_each: "x\ny\nz"
    prompt: "edit {{ item }}.txt"
    max_parallel: 3
    workspace: sync
    distribute:
      hosts: [w1, w2]
"#;

// ---------------------------------------------------------------------------
// Test 1 — synced placed step; edit is visible to downstream local step
// ---------------------------------------------------------------------------

#[tokio::test]
async fn synced_placed_step_edit_is_visible_to_downstream_local_step() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = Arc::new(RunStore::new(runs_root));

    // Seed the workspace with the original file before the run.
    std::fs::write(tmp.path().join("foo.txt"), b"orig").unwrap();

    let dispatcher = Arc::new(PlacedSyncDispatcher::new());
    let factory = Arc::new(ReadingFactory::new());

    let wf = Workflow::parse(WF_PLACED_SYNC_LOCAL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_sync_e2e_placed".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: factory.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_PLACED_SYNC_LOCAL.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("placed-sync workflow should succeed");

    // --- Placed step received workspace_path in its UnitDispatch ---
    assert!(
        dispatcher.did_see_workspace_path(),
        "synced placed step must receive workspace_path in UnitDispatch"
    );

    // --- apply_workspace_deltas wrote EDITED to foo.txt on the coordinator workspace ---
    let on_disk = std::fs::read_to_string(tmp.path().join("foo.txt"))
        .expect("foo.txt must exist on disk after apply_workspace_deltas");
    assert_eq!(
        on_disk, "EDITED",
        "apply_workspace_deltas must write 'EDITED' payload to workspace_path/foo.txt"
    );

    // --- Downstream local step saw the edited content when it was built ---
    let saw = factory
        .saw_content()
        .expect("ReadingFactory must have been invoked for the local downstream step");
    assert_eq!(
        saw, "EDITED",
        "local step must read 'EDITED' from workspace_path/foo.txt after the synced step applied its delta"
    );

    // --- Both steps succeeded ---
    assert_eq!(res.step_results.len(), 2, "two step results expected");
    assert!(
        res.step_results[0].success,
        "placed synced step must succeed"
    );
    assert!(
        res.step_results[1].success,
        "local downstream step must succeed"
    );
    assert!(
        res.step_results[1].output.contains("EDITED"),
        "downstream output must embed the edited content; got: {}",
        res.step_results[1].output
    );

    // --- One coherent run, status Completed ---
    assert!(!res.run_id.is_empty(), "run_id must be populated");
    let record = store.load(&res.run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "run must reach Completed"
    );
    assert!(record.finished_at.is_some(), "finished_at must be set");
    assert!(
        record.error_message.is_none(),
        "no error_message on success"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — fan-out disjoint edits merge into the coordinator workspace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fanout_sync_disjoint_edits_merge() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = Arc::new(RunStore::new(runs_root));

    let dispatcher = Arc::new(FanoutSyncDispatcher::new());

    let wf = Workflow::parse(WF_FANOUT_SYNC).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_sync_e2e_fanout".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_FANOUT_SYNC.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: Some(dispatcher.clone()),
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("fanout-sync workflow should succeed");

    // --- Step succeeded with all three items ---
    assert_eq!(res.step_results.len(), 1, "one step result expected");
    let step = &res.step_results[0];
    assert!(
        step.success,
        "all units succeeded → step must report success"
    );
    assert_eq!(step.items.len(), 3, "three items processed");

    // --- apply_workspace_deltas called exactly once with all three deltas ---
    assert_eq!(
        dispatcher.apply_call_count(),
        1,
        "apply_workspace_deltas must be called exactly once (after all units complete)"
    );

    // --- All three disjoint files written to workspace_path ---
    let x = std::fs::read_to_string(tmp.path().join("x.txt")).expect("x.txt must exist after sync");
    let y = std::fs::read_to_string(tmp.path().join("y.txt")).expect("y.txt must exist after sync");
    let z = std::fs::read_to_string(tmp.path().join("z.txt")).expect("z.txt must exist after sync");

    assert_eq!(x, "content-x", "x.txt must contain 'content-x'");
    assert_eq!(y, "content-y", "y.txt must contain 'content-y'");
    assert_eq!(z, "content-z", "z.txt must contain 'content-z'");

    // --- One coherent run, status Completed ---
    assert!(!res.run_id.is_empty(), "run_id must be populated");
    let record = store.load(&res.run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "run must reach Completed"
    );
    assert!(record.finished_at.is_some(), "finished_at must be set");
    assert!(
        record.error_message.is_none(),
        "no error_message on success"
    );
}
