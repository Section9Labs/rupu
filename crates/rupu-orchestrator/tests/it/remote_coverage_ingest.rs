//! A remote unit's coverage stream is merged into the coordinator workspace on
//! every outcome — success and failure — and a missing stream is a warning,
//! never a failed unit (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_coverage::{target_id, CoveragePaths, StreamLine, STREAM_VERSION};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, PreparedWorkspace, StepFactory, UnitCoverage, UnitDispatch,
    UnitDispatcher, UnitFailure, UnitOutcome, WorkspaceConflict, WorkspaceDelta,
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
    run_with(
        yaml,
        Arc::new(Scripted {
            results: Mutex::new(results),
        }),
    )
    .await
}

async fn run_with(
    yaml: &str,
    dispatcher: Arc<dyn UnitDispatcher>,
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
        unit_dispatcher: Some(dispatcher),
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

fn warnings(sink: &Collect) -> Vec<String> {
    sink.0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::StepWarning { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// A stream the coordinator could not confirm is whole (read while the unit
/// may still have been running) merges exactly like a complete one, AND says
/// that findings recorded after it was collected may be missing.
#[tokio::test]
async fn a_partial_stream_merges_and_warns_that_later_findings_may_be_missing() {
    let (tmp, sink) = run(
        PLACED,
        vec![Err(UnitFailure {
            error: RunError::Provider("poll failed".into()),
            coverage: UnitCoverage::Partial {
                bytes: stream_with_finding("f_partial"),
                reason: "read while the unit may still be running".into(),
            },
        })],
    )
    .await;
    assert_eq!(findings_in(tmp.path())[0].id, "f_partial");
    let w = warnings(&sink);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("host_01R"), "{w:?}");
    assert!(
        w[0].contains("read while the unit may still be running"),
        "{w:?}"
    );
    assert!(w[0].contains("may be missing"), "{w:?}");
}

/// A complete stream merges silently: no warning is the signal that nothing
/// was lost.
#[tokio::test]
async fn a_complete_stream_merges_without_a_warning() {
    let (tmp, sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(stream_with_finding("f_whole")),
        })],
    )
    .await;
    assert_eq!(findings_in(tmp.path())[0].id, "f_whole");
    assert!(warnings(&sink).is_empty(), "{:?}", warnings(&sink));
}

/// The no-begin-line warning must not claim the host is old: a current host
/// writes the begin line at start, so its absence there means the stream
/// failed to start or was lost in transport.
#[tokio::test]
async fn the_no_begin_line_warning_covers_an_old_host_and_a_lost_stream() {
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
    let w = warnings(&sink);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("predate coverage streaming"), "{w:?}");
    assert!(w[0].contains("lost in transport"), "{w:?}");
    assert!(!w[0].contains("upgrade rupu"), "{w:?}");
}

// ── I3: the delta's `.rupu/coverage/` vs the unit's coverage stream ─────────

/// The coverage file a host's scratch workspace wrote: carried by the
/// collected delta like any other file.
const SCRATCH_COVERAGE: &str = ".rupu/coverage/scratch_target/findings.jsonl";

/// A synced unit's delta: one ordinary file plus the host's coverage file.
/// The payload is a JSON `path → content` map this dispatcher's own
/// `apply_workspace_deltas` writes, standing in for the workspace codec.
fn delta_with_coverage(file: &str) -> WorkspaceDelta {
    let files: BTreeMap<String, String> = [
        (file.to_string(), "edited".to_string()),
        (SCRATCH_COVERAGE.to_string(), "{}\n".to_string()),
    ]
    .into();
    WorkspaceDelta {
        changed: files.keys().cloned().collect(),
        deleted: vec![],
        payload: serde_json::to_vec(&files).unwrap(),
    }
}

/// Answers each unit by its index, and implements the delta hooks over the
/// JSON payload of [`delta_with_coverage`].
struct SyncByIndex {
    results: Mutex<BTreeMap<usize, Result<UnitOutcome, UnitFailure>>>,
}

#[async_trait]
impl UnitDispatcher for SyncByIndex {
    async fn prepare_workspace(
        &self,
        _workspace_path: &std::path::Path,
    ) -> Result<PreparedWorkspace, RunError> {
        Ok(PreparedWorkspace::new(b"packed".to_vec()))
    }

    async fn dispatch_unit(
        &self,
        unit: UnitDispatch,
        _host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        self.results.lock().unwrap().remove(&unit.index).unwrap()
    }

    async fn strip_delta_coverage(&self, delta: &WorkspaceDelta) -> Result<WorkspaceDelta, String> {
        let mut files: BTreeMap<String, String> =
            serde_json::from_slice(&delta.payload).map_err(|e| e.to_string())?;
        files.retain(|p, _| !p.starts_with(".rupu/coverage/"));
        Ok(WorkspaceDelta {
            changed: files.keys().cloned().collect(),
            deleted: vec![],
            payload: serde_json::to_vec(&files).unwrap(),
        })
    }

    async fn apply_workspace_deltas(
        &self,
        workspace_path: &std::path::Path,
        deltas: &[WorkspaceDelta],
    ) -> Result<(), WorkspaceConflict> {
        for d in deltas {
            let files: BTreeMap<String, String> = serde_json::from_slice(&d.payload).unwrap();
            for (rel, body) in files {
                let p = workspace_path.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
        }
        Ok(())
    }
}

fn synced_ok(file: &str, coverage: UnitCoverage) -> Result<UnitOutcome, UnitFailure> {
    Ok(UnitOutcome {
        output: "ok".into(),
        success: true,
        error: None,
        workspace_delta: Some(delta_with_coverage(file)),
        coverage,
    })
}

const PLACED_SYNC: &str = "name: w\nsteps:\n  - id: s\n    agent: sec\n    prompt: p\n    host: host_01R\n    workspace: sync\n";

/// The unit's stream arrived with its begin line and merged: it is the
/// unit's coverage, so the delta's copy (keyed to the host's scratch path)
/// is not applied — the rest of the delta is.
#[tokio::test]
async fn a_placed_units_merged_stream_replaces_its_delta_coverage() {
    let (tmp, _sink) = run_with(
        PLACED_SYNC,
        Arc::new(SyncByIndex {
            results: Mutex::new(
                [(
                    0,
                    synced_ok(
                        "src.txt",
                        UnitCoverage::Stream(stream_with_finding("f_stream")),
                    ),
                )]
                .into(),
            ),
        }),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("src.txt")).unwrap(),
        "edited"
    );
    assert!(
        !tmp.path().join(SCRATCH_COVERAGE).exists(),
        "the stream won: the delta's coverage must not be applied"
    );
    assert_eq!(findings_in(tmp.path())[0].id, "f_stream");
}

/// No stream arrived (an older host): the delta's coverage is the only copy,
/// so it applies exactly as it did before coverage streaming.
#[tokio::test]
async fn a_placed_unit_without_a_stream_keeps_its_delta_coverage() {
    for coverage in [
        UnitCoverage::Unavailable("host predates it".into()),
        UnitCoverage::Stream(Vec::new()),
    ] {
        let (tmp, _sink) = run_with(
            PLACED_SYNC,
            Arc::new(SyncByIndex {
                results: Mutex::new([(0, synced_ok("src.txt", coverage.clone()))].into()),
            }),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(SCRATCH_COVERAGE)).unwrap(),
            "{}\n",
            "{coverage:?}: no stream, so the delta's coverage applies"
        );
        assert!(tmp.path().join("src.txt").exists());
    }
}

/// Fan-out: the decision is per unit — unit 0's stream merged (its delta
/// coverage is stripped), unit 1 sent none (its delta coverage applies).
#[tokio::test]
async fn fan_out_units_strip_delta_coverage_only_when_their_own_stream_merged() {
    let yaml = "name: w\nsteps:\n  - id: fan\n    agent: sec\n    actions: []\n    for_each: \"a\\nb\"\n    prompt: \"p {{ item }}\"\n    workspace: sync\n    distribute:\n      hosts: [host_01A, host_01B]\n";
    let mut unit_1 = synced_ok("b.txt", UnitCoverage::Unavailable("old host".into()));
    if let Ok(o) = unit_1.as_mut() {
        // Distinct coverage path per unit, or the tar-style overlap would be
        // the thing under test.
        let files: BTreeMap<String, String> = [
            ("b.txt".to_string(), "edited".to_string()),
            (
                ".rupu/coverage/scratch_b/findings.jsonl".to_string(),
                "{}\n".to_string(),
            ),
        ]
        .into();
        o.workspace_delta = Some(WorkspaceDelta {
            changed: files.keys().cloned().collect(),
            deleted: vec![],
            payload: serde_json::to_vec(&files).unwrap(),
        });
    }
    let (tmp, _sink) = run_with(
        yaml,
        Arc::new(SyncByIndex {
            results: Mutex::new(
                [
                    (
                        0,
                        synced_ok("a.txt", UnitCoverage::Stream(stream_with_finding("f_a"))),
                    ),
                    (1, unit_1),
                ]
                .into(),
            ),
        }),
    )
    .await;
    assert!(tmp.path().join("a.txt").exists() && tmp.path().join("b.txt").exists());
    assert!(
        !tmp.path().join(SCRATCH_COVERAGE).exists(),
        "unit 0's stream merged"
    );
    assert!(
        tmp.path()
            .join(".rupu/coverage/scratch_b/findings.jsonl")
            .exists(),
        "unit 1 sent no stream"
    );
}

// ── I4: a finding's artifacts, end to end through the runner ────────────────

/// A unit's stream whose one finding carries a full report with one artifact,
/// recorded the way the host's own store recorded it (`copied`, no host).
fn stream_with_artifact(id: &str) -> Vec<u8> {
    let mut report: rupu_coverage::FindingReport = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    report.artifacts = vec![rupu_coverage::report::ArtifactRef {
        path: "poc/repro.sh".into(),
        sha256: "c3".repeat(32),
        size: 42,
        kind: Some(rupu_coverage::report::ArtifactKind::Text),
        stored: Some(rupu_coverage::report::ArtifactStorage::Copied),
        host: None,
    }];
    let text = String::from_utf8(stream_with_finding(id)).unwrap();
    let mut out = String::new();
    for line in text.lines() {
        let mut parsed: StreamLine = serde_json::from_str(line).unwrap();
        if let StreamLine::Findings { record, .. } = &mut parsed {
            record.profile = rupu_coverage::FindingProfile::Full;
            record.report = Some(report.clone());
        }
        out.push_str(&serde_json::to_string(&parsed).unwrap());
        out.push('\n');
    }
    out.into_bytes()
}

fn only_artifact(ws: &std::path::Path) -> rupu_coverage::report::ArtifactRef {
    let findings = findings_in(ws);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let report = findings[0]
        .report
        .clone()
        .expect("the report survives the merge");
    assert_eq!(report.artifacts.len(), 1);
    report.artifacts[0].clone()
}

/// A placed step on a registry host: the blob lives in THAT host's store, so
/// the merged finding records its artifact `external` with the host's id —
/// what the artifact pull (Plan B) dereferences — keeping path, hash, size
/// and kind.
#[tokio::test]
async fn a_registry_hosts_artifact_merges_external_with_that_host() {
    let (tmp, _sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(stream_with_artifact("f_art")),
        })],
    )
    .await;
    let a = only_artifact(tmp.path());
    assert_eq!(
        a.stored,
        Some(rupu_coverage::report::ArtifactStorage::External)
    );
    assert_eq!(a.host.as_deref(), Some("host_01R"));
    assert_eq!(
        (a.path.as_str(), a.sha256.as_str(), a.size),
        ("poc/repro.sh", "c3".repeat(32).as_str(), 42)
    );
    assert_eq!(a.kind, Some(rupu_coverage::report::ArtifactKind::Text));
}

/// A `host: local` unit shares the coordinator's artifact store: the
/// artifact stays `copied`, with no host.
#[tokio::test]
async fn a_local_units_artifact_stays_copied() {
    let yaml = "name: w\nsteps:\n  - id: s\n    agent: sec\n    prompt: p\n    host: local\n";
    let (tmp, _sink) = run(
        yaml,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(stream_with_artifact("f_local")),
        })],
    )
    .await;
    let a = only_artifact(tmp.path());
    assert_eq!(
        a.stored,
        Some(rupu_coverage::report::ArtifactStorage::Copied)
    );
    assert_eq!(a.host, None);
}
