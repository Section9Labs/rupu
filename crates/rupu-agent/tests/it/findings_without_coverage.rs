//! `report_finding` must be usable WITHOUT the coverage harness.
//!
//! Recording a finding and running the coverage harness are different things.
//! The harness is code-shaped — it needs a catalog of concerns and marks
//! (concern_id, file_path) pairs — so an assessment of hosts, endpoints or
//! cloud resources has no catalog and, before this, no way to record a finding
//! at all. It reported findings somewhere else entirely and the control plane
//! showed zero.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{target_id, CoveragePaths};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

/// A finding with no `file_path` — the shape a network assessment produces.
fn finding_input() -> serde_json::Value {
    serde_json::json!({
        "scope": "repo",
        "summary": "Console endpoint accepts a percent-encoded approval bypass",
        "severity": "medium",
        "evidence": { "rationale": "Observed on the live endpoint; no file involved." }
    })
}

pub(crate) fn opts_for(
    workspace: &std::path::Path,
    agent_tools: Option<Vec<String>>,
    turns: Vec<ScriptedTurn>,
) -> AgentRunOpts {
    AgentRunOpts {
        seed_source: None,
        collectors: Vec::new(),
        extra_tools: Vec::new(),
        agent_name: "net-assessor".into(),
        agent_system_prompt: "You assess hosts.".into(),
        agent_tools,
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_findings_test".into(),
        workspace_id: "ws_findings_test".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            // These tests exercise the lightweight record.
            findings: Some(
                rupu_coverage::FindingWriteOptions::default()
                    .with_profile(rupu_coverage::FindingProfile::Summary),
            ),
            ..Default::default()
        },
        user_message: "Assess the endpoint.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        on_usage: None,
        // The whole point: no coverage harness.
        concerns: None,
        scope_name: None,
        limits: rupu_providers::model_limits::ModelLimits::unknown(),
        surface_tag: Some("autoflow".into()),
        pause: None,
        codename: None,
        recovery: Default::default(),
    }
}

fn call_then_stop() -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "t1".into(),
            tool_name: "report_finding".into(),
            tool_input: finding_input(),
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Recorded.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

#[tokio::test]
async fn granted_agent_records_a_finding_without_a_concerns_block() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    run_agent(opts_for(
        &workspace,
        Some(vec!["report_finding".to_string()]),
        call_then_stop(),
    ))
    .await
    .expect("agent run should succeed");

    let paths = CoveragePaths::new(&workspace, &target_id(&workspace, "net-assessor"));
    let text = std::fs::read_to_string(&paths.findings)
        .unwrap_or_else(|e| panic!("findings ledger at {:?} should exist: {e}", paths.findings));
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("one record");
    let rec: serde_json::Value = serde_json::from_str(line).expect("valid JSON record");

    assert_eq!(rec["severity"], "medium");
    assert_eq!(rec["scope"], "repo");
    assert!(rec["file_path"].is_null(), "network finding has no file");

    // Attribution used to be set only when coverage was enabled, which would
    // have written this record with empty run_id/model — a silent hole in the
    // audit trail rather than a loud failure.
    assert_eq!(
        rec["declared_by"]["run_id"], "run_findings_test",
        "attribution must survive without the coverage harness"
    );
    assert_eq!(rec["declared_by"]["model"], "mock-1");
    assert_eq!(rec["declared_by"]["surface"], "autoflow");
    // Agent + provider ride along so the CP can show `agent · provider/model`.
    assert_eq!(rec["declared_by"]["agent"], "net-assessor");
    assert_eq!(rec["declared_by"]["provider"], "mock");
}

#[tokio::test]
async fn tool_is_absent_when_not_granted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Same run, same script, but `report_finding` is not in `tools:`.
    // Registration is an explicit grant, not automatic — if this ever starts
    // producing a ledger, the grant has stopped meaning anything.
    let _ = run_agent(opts_for(
        &workspace,
        Some(vec!["read_file".to_string()]),
        call_then_stop(),
    ))
    .await;

    let paths = CoveragePaths::new(&workspace, &target_id(&workspace, "net-assessor"));
    assert!(
        !paths.findings.exists(),
        "ungranted agent must not be able to write findings"
    );
}

#[tokio::test]
async fn a_run_stream_receives_the_finding_with_its_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let stream = tmp.path().join("runs/run_findings_test/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_findings_test").unwrap();

    let mut opts = opts_for(
        &workspace,
        Some(vec!["report_finding".to_string()]),
        call_then_stop(),
    );
    opts.tool_context.coverage_stream = Some(stream.clone());
    run_agent(opts).await.expect("agent run should succeed");

    let lines: Vec<rupu_coverage::StreamLine> = std::fs::read_to_string(&stream)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(matches!(lines[0], rupu_coverage::StreamLine::Begin { .. }));
    let found = lines.iter().any(|l| {
        matches!(
            l,
            rupu_coverage::StreamLine::Findings { scope_name, record }
                if scope_name == "net-assessor" && record.severity == rupu_coverage::Severity::Medium
        )
    });
    assert!(found, "the finding must reach the stream: {lines:?}");
}

// ---------------------------------------------------------------------------
// finding.verify: the same opt-in registration, for a verifier run.
// ---------------------------------------------------------------------------

/// File a full-report finding as a DIFFERENT run, into the ledger the
/// verifier (agent `net-assessor`) will see, and return its id.
fn seed_other_runs_finding(workspace: &std::path::Path) -> String {
    let report: rupu_coverage::FindingReport = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let paths = CoveragePaths::new(workspace, &target_id(workspace, "net-assessor"));
    paths.ensure_dir().unwrap();
    let input = rupu_coverage::ReportFindingInput {
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: rupu_coverage::FindingScope::Repo,
        summary: None,
        severity: None,
        concern_id: None,
        evidence: None,
        report: Some(report),
        asset: None,
    };
    let attribution = rupu_coverage::Attribution {
        run_id: "run_filer".into(),
        model: "m".into(),
        surface: rupu_coverage::Surface::Workflow,
        codename: None,
        agent: None,
        provider: None,
    };
    let opts = rupu_coverage::FindingWriteOptions::default()
        .with_profile(rupu_coverage::FindingProfile::Full);
    rupu_coverage::report_finding(&paths, attribution, input, &opts)
        .unwrap()
        .id
}

fn verify_then_stop(finding_id: &str) -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "v1".into(),
            tool_name: "finding.verify".into(),
            tool_input: serde_json::json!({
                "finding_id": finding_id,
                "status": "confirmed",
                "notes": "reproduced",
                // Not an input the tool reads: the identity is the run's.
                "by_run": "run_filer",
            }),
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Verified.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

fn recorded_verification(
    workspace: &std::path::Path,
    id: &str,
) -> Option<rupu_coverage::Verification> {
    let paths = CoveragePaths::new(workspace, &target_id(workspace, "net-assessor"));
    rupu_coverage::read_findings(&paths)
        .unwrap()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
        .report
        .unwrap()
        .verification
}

#[tokio::test]
async fn granted_verifier_records_a_verdict_as_its_own_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let id = seed_other_runs_finding(&workspace);

    // No `concerns:` block and no `report_finding`: `finding.verify` alone in
    // `tools:` is enough.
    run_agent(opts_for(
        &workspace,
        Some(vec!["finding.verify".to_string()]),
        verify_then_stop(&id),
    ))
    .await
    .expect("agent run should succeed");

    let v = recorded_verification(&workspace, &id).expect("a verdict was recorded");
    assert_eq!(v.status, rupu_coverage::VerificationStatus::Confirmed);
    assert_eq!(v.by_run.as_deref(), Some("run_findings_test"));
    assert_eq!(v.by_agent.as_deref(), Some("net-assessor"));
    assert_eq!(v.notes.as_deref(), Some("reproduced"));
}

#[tokio::test]
async fn finding_verify_is_absent_when_not_granted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let id = seed_other_runs_finding(&workspace);

    // Granting `report_finding` (or anything else) does not grant
    // `finding.verify`.
    let _ = run_agent(opts_for(
        &workspace,
        Some(vec!["report_finding".to_string(), "read_file".to_string()]),
        verify_then_stop(&id),
    ))
    .await;

    assert!(
        recorded_verification(&workspace, &id).is_none(),
        "an ungranted agent must not be able to verify a finding"
    );
}

#[tokio::test]
async fn finding_verify_is_not_granted_by_an_absent_or_wildcard_tools_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let id = seed_other_runs_finding(&workspace);

    // Only an exact `finding.verify` entry grants it, as with `report_finding`.
    for tools in [None, Some(vec!["*".to_string()])] {
        let _ = run_agent(opts_for(&workspace, tools.clone(), verify_then_stop(&id))).await;
        assert!(
            recorded_verification(&workspace, &id).is_none(),
            "tools {tools:?} must not grant finding.verify"
        );
    }
}

/// An agent that ALSO runs the coverage harness (`concerns:`).
fn concerns_opts_for(
    workspace: &std::path::Path,
    agent_tools: Option<Vec<String>>,
    turns: Vec<ScriptedTurn>,
) -> AgentRunOpts {
    let mut opts = opts_for(workspace, agent_tools, turns);
    opts.concerns = Some(rupu_coverage::ConcernsBlock {
        entries: vec![rupu_coverage::ConcernsEntry::Include(
            rupu_coverage::IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: rupu_coverage::CatalogMode::Auto,
                filter: None,
            },
        )],
    });
    opts
}

#[tokio::test]
async fn a_concerns_agent_holds_finding_verify_only_when_it_lists_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let id = seed_other_runs_finding(&workspace);

    // The coverage bundle brings `report_finding` and the coverage tools, but
    // not a verdict: without the `tools:` entry the call finds no such tool.
    run_agent(concerns_opts_for(
        &workspace,
        Some(vec!["read_file".to_string()]),
        verify_then_stop(&id),
    ))
    .await
    .expect("agent run should succeed");
    assert!(
        recorded_verification(&workspace, &id).is_none(),
        "a concerns agent must not auto-hold finding.verify"
    );

    // Listing it grants it, against the SAME ledger the bundle writes to.
    run_agent(concerns_opts_for(
        &workspace,
        Some(vec!["finding.verify".to_string()]),
        verify_then_stop(&id),
    ))
    .await
    .expect("agent run should succeed");
    let v = recorded_verification(&workspace, &id).expect("a verdict was recorded");
    assert_eq!(v.status, rupu_coverage::VerificationStatus::Confirmed);
    assert_eq!(v.by_run.as_deref(), Some("run_findings_test"));
    assert_eq!(v.by_agent.as_deref(), Some("net-assessor"));
}
