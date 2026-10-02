//! A remote unit's coverage stream is merged into the coordinator workspace on
//! every outcome — success and failure — and a missing stream is a warning,
//! never a failed unit (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_coverage::{target_id, CoveragePaths, StreamLine, STREAM_VERSION};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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
        panic!("remote units must not be built locally");
    }
}

fn stream_with_finding(id: &str) -> Vec<u8> {
    let record = rupu_coverage::FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: rupu_coverage::FindingScope::File,
        summary: "s".into(),
        severity: rupu_coverage::Severity::High,
        concern_id: None,
        evidence: rupu_coverage::FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: rupu_coverage::Attribution {
            run_id: "run_U".into(),
            model: "m".into(),
            surface: rupu_coverage::Surface::Agent,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: chrono::Utc::now(),
        profile: rupu_coverage::FindingProfile::Summary,
        report: None,
    };
    let mut out = String::new();
    for line in [
        StreamLine::Begin {
            v: STREAM_VERSION,
            run_id: "run_U".into(),
        },
        StreamLine::Findings {
            scope_name: "sec".into(),
            record,
        },
    ] {
        out.push_str(&serde_json::to_string(&line).unwrap());
        out.push('\n');
    }
    out.into_bytes()
}

/// Returns a scripted result per dispatch, in order.
struct Scripted {
    results: Mutex<Vec<Result<UnitOutcome, UnitFailure>>>,
}

#[async_trait]
impl UnitDispatcher for Scripted {
    async fn dispatch_unit(
        &self,
        _unit: UnitDispatch,
        _host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        self.results.lock().unwrap().remove(0)
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<Event>>);

impl EventSink for Collect {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.0.lock().unwrap().push(ev.clone());
    }
}

async fn run(
    yaml: &str,
    results: Vec<Result<UnitOutcome, UnitFailure>>,
) -> (tempfile::TempDir, Arc<Collect>) {
    let tmp = tempfile::tempdir().unwrap();
    let sink = Arc::new(Collect::default());
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_cov".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::new(RunStore::new(tmp.path().join("runs")))),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: Some(Arc::new(Scripted {
            results: Mutex::new(results),
        })),
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    let _ = run_workflow(opts).await;
    (tmp, sink)
}

const PLACED: &str =
    "name: w\nsteps:\n  - id: s\n    agent: sec\n    prompt: p\n    host: host_01R\n    continue_on_error: true\n";

fn findings_in(ws: &std::path::Path) -> Vec<rupu_coverage::FindingRecord> {
    rupu_coverage::read_findings(&CoveragePaths::new(ws, &target_id(ws, "sec"))).unwrap()
}

#[tokio::test]
async fn a_successful_placed_units_findings_land_under_the_coordinators_target() {
    let (tmp, _sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(stream_with_finding("f_ok")),
        })],
    )
    .await;
    let got = findings_in(tmp.path());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "f_ok");
}

#[tokio::test]
async fn a_failed_units_findings_still_land() {
    let (tmp, _sink) = run(
        PLACED,
        vec![Err(UnitFailure {
            error: RunError::Provider("unit died".into()),
            coverage: UnitCoverage::Stream(stream_with_finding("f_failed")),
        })],
    )
    .await;
    assert_eq!(findings_in(tmp.path())[0].id, "f_failed");
}

#[tokio::test]
async fn unavailable_coverage_is_a_step_warning_not_a_failure() {
    let (tmp, sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Unavailable("host predates it".into()),
        })],
    )
    .await;
    assert!(findings_in(tmp.path()).is_empty());
    let events = sink.0.lock().unwrap();
    let warned = events.iter().any(|e| matches!(
        e,
        Event::StepWarning { step_id, message, .. }
            if step_id == "s" && message.contains("host_01R") && message.contains("host predates it")
    ));
    assert!(warned, "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StepCompleted { success: true, .. })),
        "a coverage warning must never fail the unit: {events:?}"
    );
}

#[tokio::test]
async fn a_stream_without_a_begin_line_is_a_warning() {
    let (_tmp, sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(Vec::new()),
        })],
    )
    .await;
    let events = sink.0.lock().unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepWarning { message, .. }
            if message.contains("no coverage stream") && message.contains("workspace-sync delta")
    )));
}

#[tokio::test]
async fn a_fan_out_primary_failure_and_its_retry_both_merge() {
    let yaml = "name: w\nsteps:\n  - id: fan\n    agent: sec\n    actions: []\n    for_each: \"only\"\n    prompt: \"p {{ item }}\"\n    distribute:\n      hosts: [host_01A, host_01B]\n";
    let (tmp, _sink) = run(
        yaml,
        vec![
            Err(UnitFailure {
                error: RunError::Provider("primary died".into()),
                coverage: UnitCoverage::Stream(stream_with_finding("f_primary")),
            }),
            Ok(UnitOutcome {
                output: "ok".into(),
                success: true,
                error: None,
                workspace_delta: None,
                coverage: UnitCoverage::Stream(stream_with_finding("f_retry")),
            }),
        ],
    )
    .await;
    let mut ids: Vec<String> = findings_in(tmp.path()).into_iter().map(|f| f.id).collect();
    ids.sort();
    assert_eq!(ids, vec!["f_primary", "f_retry"]);
}
