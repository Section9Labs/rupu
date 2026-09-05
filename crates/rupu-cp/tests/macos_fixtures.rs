#![deny(clippy::all)]
//! Golden fixtures for the macOS app (apps/rupu-macos/Fixtures/).
//! `cargo test -p rupu-cp --test macos_fixtures` asserts no drift;
//! `REGEN_FIXTURES=1` rewrites. Swift decodes these in RupuAPITests.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use rupu_ast::AstNode;
use rupu_config::{KeyProvenance, KeySource};
use rupu_coverage::{
    AssertionStatus, Attribution as CoverageAttribution, CatalogMode, Concern, ConcernAssertion,
    Evidence as CoverageEvidence, FileView, FindingEvidence as CoverageFindingEvidence,
    FindingRecord as CoverageFindingRecord, FindingScope as CoverageFindingScope, FlatCatalog,
    Severity as CoverageSeverity, Surface as CoverageSurface, TouchStrength,
};
use rupu_cp::api::autoflow_claims::ClaimRow;
use rupu_cp::api::code::{FileContent, FileListResult, TreeEntry, TreeResult};
use rupu_cp::api::config::{ConfigView, RuntimeStatus};
use rupu_cp::api::findings::{FindingOut, FindingsResponse, FindingsSummary};
use rupu_cp::api::graph::{ApprovalGateDto, GateDto, StepDag, StepNodeDto, SubStepDto};
use rupu_cp::api::projects::ProjectRow;
use rupu_cp::api::runs::RunListRow;
use rupu_cp::api::source::{AstResponse, SourceLine, SourceSlice};
use rupu_cp::api::usage_outliers::OutlierRun;
use rupu_cp::host::dashboard_summary::{
    ActiveCounts, ActiveLongest, CycleCounts, DashboardSummary, FleetCounts, TerminalBucket,
    ThroughputBucket,
};
use rupu_cp::usage::{UsageBreakdownRow, UsageSummary};
use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{AwaitingGate, RunStatus, StepKind};
use rupu_orchestrator::{FindingRecord, RunRecord, StepResultRecord, UnitCheckpoint};
use rupu_runtime::{WorkerCapabilities, WorkerKind, WorkerRecord};
use rupu_workspace::{AutoflowClaimRecord, ClaimStatus};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/rupu-macos/Fixtures")
}

fn check_fixture(name: &str, value: &impl serde::Serialize) {
    let path = fixtures_dir().join(name);
    let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
    if std::env::var_os("REGEN_FIXTURES").is_some() {
        std::fs::write(&path, rendered + "\n").expect("write fixture");
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
    assert_eq!(
        on_disk.trim_end(),
        rendered,
        "fixture {name} drifted from the Rust types; run `make macos-fixtures` and update the Swift models"
    );
}

#[test]
fn events_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let events: Vec<Event> = vec![
        Event::RunStarted {
            event_version: 1,
            run_id: "run-01".into(),
            workflow_path: "wf/x.yaml".into(),
            started_at: t,
        },
        Event::StepStarted {
            run_id: "run-01".into(),
            step_id: "plan".into(),
            kind: StepKind::Linear,
            agent: Some("rupuso".into()),
            host: None,
        },
        Event::StepWorking {
            run_id: "run-01".into(),
            step_id: "plan".into(),
            note: Some("thinking".into()),
            transcript_path: Some("t/plan.jsonl".into()),
        },
        Event::StepAwaitingApproval {
            run_id: "run-01".into(),
            step_id: "gate".into(),
            reason: "manual gate".into(),
        },
        Event::StepCompleted {
            run_id: "run-01".into(),
            step_id: "plan".into(),
            success: true,
            duration_ms: 4200,
            host: Some("mini".into()),
        },
        Event::StepFailed {
            run_id: "run-01".into(),
            step_id: "build".into(),
            error: "exit 1".into(),
        },
        Event::StepSkipped {
            run_id: "run-01".into(),
            step_id: "deploy".into(),
            reason: "branch untaken".into(),
        },
        Event::UnitStarted {
            run_id: "run-01".into(),
            step_id: "fan".into(),
            index: 0,
            unit_key: "crates/a".into(),
            agent: None,
            transcript_path: "t/u0.jsonl".into(),
            host: None,
        },
        Event::UnitCompleted {
            run_id: "run-01".into(),
            step_id: "fan".into(),
            index: 0,
            unit_key: "crates/a".into(),
            success: true,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        },
        Event::PanelRound {
            run_id: "run-01".into(),
            step_id: "review".into(),
            round: 2,
            max_iterations: 5,
            max_severity_remaining: Some("high".into()),
        },
        Event::RunCompleted {
            run_id: "run-01".into(),
            status: RunStatus::Completed,
            finished_at: t,
        },
        Event::RunFailed {
            run_id: "run-01".into(),
            error: "step build failed".into(),
            finished_at: t,
        },
        Event::RunPaused {
            run_id: "run-01".into(),
        },
        Event::RunResumed {
            run_id: "run-01".into(),
        },
        Event::StepPaused {
            run_id: "run-01".into(),
            step_id: "plan".into(),
        },
        Event::StepResumed {
            run_id: "run-01".into(),
            step_id: "plan".into(),
        },
        Event::DispatchStarted {
            run_id: "run-01".into(),
            sub_run_id: "run-02".into(),
            agent: Some("reviewer".into()),
            transcript_path: "t/d.jsonl".into(),
        },
        Event::DispatchCompleted {
            run_id: "run-01".into(),
            sub_run_id: "run-02".into(),
            success: true,
            tokens_in: 1000,
            tokens_out: 200,
        },
    ];
    assert_events_cover_every_variant(&events);
    check_fixture("events.json", &events);
}

/// Exhaustiveness guard for the hand-enumerated `events` vec above: the
/// vec alone catches field changes on an *existing* variant (drift shows
/// up as a fixture diff), but a brand-new `Event` variant compiles fine
/// without ever being added to it — Swift then decodes that variant only
/// as `.unknown`. Matching each already-constructed element against every
/// current variant with NO wildcard arm turns that into a compile error
/// instead: the match's exhaustiveness is checked against the `Event`
/// type itself, not against what happens to be in the vec, so a new
/// variant fails to compile here until a case is added.
///
/// adding a variant? extend events_fixture_is_current and regenerate via
/// make macos-fixtures
fn assert_events_cover_every_variant(events: &[Event]) {
    for event in events {
        match event {
            Event::RunStarted { .. } => {}
            Event::StepStarted { .. } => {}
            Event::StepWorking { .. } => {}
            Event::StepAwaitingApproval { .. } => {}
            Event::StepCompleted { .. } => {}
            Event::StepFailed { .. } => {}
            Event::StepSkipped { .. } => {}
            Event::UnitStarted { .. } => {}
            Event::UnitCompleted { .. } => {}
            Event::PanelRound { .. } => {}
            Event::RunCompleted { .. } => {}
            Event::RunFailed { .. } => {}
            Event::RunPaused { .. } => {}
            Event::RunResumed { .. } => {}
            Event::StepPaused { .. } => {}
            Event::StepResumed { .. } => {}
            Event::DispatchStarted { .. } => {}
            Event::DispatchCompleted { .. } => {}
        }
    }
}

/// A `RunRecord` with every field filled to a deterministic, non-`now()`
/// value. Callers override the fields their scenario cares about via
/// struct-update syntax (`..sample_run_record(...)`).
fn sample_run_record(id: &str, started_at: chrono::DateTime<chrono::Utc>) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "nightly-health".into(),
        status: RunStatus::Running,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws-1".into(),
        workspace_path: PathBuf::from("/tmp/proj"),
        transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at,
        finished_at: None,
        error_message: None,
        awaiting: Vec::new(),
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        resume_gate_id: None,
        resume_approver: None,
        reject_cleanup_pending: None,
        permission_mode: Some("ask".into()),
        final_output: None,
        loop_progress: Default::default(),
    }
}

#[test]
fn run_list_row_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let mut row1 = serde_json::to_value(RunListRow {
        id: "run-01".into(),
        workflow_name: "nightly-health".into(),
        status: RunStatus::Running,
        started_at: t,
        finished_at: None,
        trigger: "cron",
        usage: UsageSummary {
            input_tokens: 1000,
            output_tokens: 200,
            cached_tokens: 0,
            total_tokens: 1200,
            cost_usd: Some(0.12),
            priced: true,
            runs: 1,
        },
        turns: 4,
        duration_ms: None,
    })
    .expect("serialize RunListRow");
    row1["host_id"] = serde_json::json!("local");

    let mut row2 = serde_json::to_value(RunListRow {
        id: "run-02".into(),
        workflow_name: "nightly-health".into(),
        status: RunStatus::AwaitingApproval,
        started_at: t,
        finished_at: None,
        trigger: "manual",
        usage: UsageSummary::default(),
        turns: 0,
        duration_ms: None,
    })
    .expect("serialize RunListRow");
    row2["host_id"] = serde_json::json!("mini");

    check_fixture("run_list_row.json", &vec![row1, row2]);
}

#[test]
fn event_rows_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let events: Vec<Event> = vec![
        Event::RunStarted {
            event_version: 1,
            run_id: "run-01".into(),
            workflow_path: "wf/x.yaml".into(),
            started_at: t,
        },
        Event::StepCompleted {
            run_id: "run-01".into(),
            step_id: "plan".into(),
            success: true,
            duration_ms: 4200,
            host: None,
        },
        Event::RunCompleted {
            run_id: "run-01".into(),
            status: RunStatus::Completed,
            finished_at: t,
        },
    ];
    // Mirrors `recent_events`'s injected `ts`/`pos` — see api/events.rs.
    let rows: Vec<serde_json::Value> = events
        .iter()
        .enumerate()
        .map(|(pos, event)| {
            let mut row = serde_json::to_value(event).expect("serialize event");
            if let Some(obj) = row.as_object_mut() {
                obj.insert("ts".to_string(), serde_json::json!(1_755_691_200_000i64));
                obj.insert("pos".to_string(), serde_json::json!(pos));
            }
            row
        })
        .collect();
    check_fixture("event_rows.json", &rows);
}

#[test]
fn transcript_events_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let events: Vec<rupu_transcript::Event> = vec![
        rupu_transcript::Event::RunStart {
            run_id: "run-01".into(),
            workspace_id: "ws-1".into(),
            agent: "rupuso".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at: t,
            mode: rupu_transcript::RunMode::Ask,
            schema: None,
            system_prompt: None,
        },
        rupu_transcript::Event::TurnStart { turn_idx: 0 },
        rupu_transcript::Event::AssistantDelta {
            content: "Thinking".into(),
        },
        rupu_transcript::Event::AssistantMessage {
            content: "Here is the plan".into(),
            thinking: Some("reasoning trace".into()),
        },
        rupu_transcript::Event::AssistantMessage {
            content: "Done".into(),
            thinking: None,
        },
        rupu_transcript::Event::ToolCall {
            call_id: "call-1".into(),
            tool: "Read".into(),
            input: serde_json::json!({ "file_path": "src/a.rs" }),
        },
        rupu_transcript::Event::ToolResult {
            call_id: "call-2".into(),
            output: "error output".into(),
            error: Some("boom".into()),
            duration_ms: 5,
            structured: Some(serde_json::json!({ "tool": "ast_grep", "matchCount": 2 })),
        },
        rupu_transcript::Event::ToolResult {
            call_id: "call-1".into(),
            output: "ok".into(),
            error: None,
            duration_ms: 12,
            structured: None,
        },
        rupu_transcript::Event::FileEdit {
            path: "src/a.rs".into(),
            kind: rupu_transcript::FileEditKind::Modify,
            diff: "@@ -1 +1 @@\n-old\n+new\n".into(),
        },
        rupu_transcript::Event::CommandRun {
            argv: vec!["cargo".into(), "test".into()],
            cwd: "/repo".into(),
            exit_code: 0,
            stdout_bytes: 100,
            stderr_bytes: 0,
        },
        rupu_transcript::Event::ActionEmitted {
            kind: "issues.create".into(),
            payload: serde_json::json!({ "title": "x" }),
            allowed: true,
            applied: true,
            reason: Some("auto-approved".into()),
        },
        rupu_transcript::Event::ActionEmitted {
            kind: "issues.create".into(),
            payload: serde_json::json!({ "title": "y" }),
            allowed: false,
            applied: false,
            reason: None,
        },
        rupu_transcript::Event::GateRequested {
            gate_id: "gate-1".into(),
            prompt: "Deploy to prod?".into(),
            decision: Some("approved".into()),
            decided_by: Some("matt".into()),
        },
        rupu_transcript::Event::GateRequested {
            gate_id: "gate-2".into(),
            prompt: "Deploy to staging?".into(),
            decision: None,
            decided_by: None,
        },
        rupu_transcript::Event::TurnEnd {
            turn_idx: 0,
            tokens_in: Some(500),
            tokens_out: Some(120),
            stop_reason: None,
            response_id: None,
        },
        rupu_transcript::Event::TurnEnd {
            turn_idx: 1,
            tokens_in: None,
            tokens_out: None,
            stop_reason: None,
            response_id: None,
        },
        rupu_transcript::Event::Usage {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            served_model: Some("claude-sonnet-4-6-20260201".into()),
            input_tokens: 1000,
            output_tokens: 200,
            cached_tokens: 50,
        },
        rupu_transcript::Event::Usage {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            served_model: None,
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
        },
        rupu_transcript::Event::RunComplete {
            run_id: "run-01".into(),
            status: rupu_transcript::RunStatus::Error,
            total_tokens: 1200,
            duration_ms: 42_000,
            error: Some("step build failed".into()),
        },
        rupu_transcript::Event::RunComplete {
            run_id: "run-01".into(),
            status: rupu_transcript::RunStatus::Ok,
            total_tokens: 1200,
            duration_ms: 42_000,
            error: None,
        },
        rupu_transcript::Event::ToolAudit {
            tool: "issues.create".into(),
            declared: true,
            granted: true,
            blocked: false,
            restricted: true,
        },
        // A denied catalog call: narrowed out of the step's `actions:`
        // allowlist, so it never reached the registry (macOS Task 2 —
        // decode coverage for `blocked: true`, the one outcome operators
        // must see; see rupu-cli/src/output/live_run.rs).
        rupu_transcript::Event::ToolAudit {
            tool: "bash".into(),
            declared: false,
            granted: false,
            blocked: true,
            restricted: true,
        },
        // ast_grep's real `tool_result.structured` shape (rupu-tools's
        // `ast_grep.rs` emitter) — locks the Swift decode Task 6 will add.
        rupu_transcript::Event::ToolResult {
            call_id: "call-3".into(),
            output: "src/lib.rs:10:5: fn process(x: i32) -> i32 {\n".into(),
            error: None,
            duration_ms: 8,
            structured: Some(serde_json::json!({
                "tool": "ast_grep",
                "pattern": "fn $NAME($$$ARGS)",
                "lang": "rust",
                "matchCount": 1,
                "fileCount": 1,
                "truncated": false,
                "matches": [
                    {
                        "file": "src/lib.rs",
                        "range": { "startLine": 10, "startCol": 5, "endLine": 12, "endCol": 6 },
                        "text": "fn process(x: i32) -> i32 {\n    x + 1\n}",
                        "metaVars": {
                            "single": {
                                "NAME": { "text": "process", "textOffset": { "start": 3, "end": 10 } }
                            },
                            "multi": {
                                "ARGS": [
                                    { "text": "x: i32", "textOffset": { "start": 11, "end": 17 } }
                                ]
                            }
                        }
                    }
                ]
            })),
        },
        rupu_transcript::Event::NetFlow {
            flow: Box::new(FlowRecord {
                id: FlowId::from_parts(1, 1),
                ts: t,
                ctx: FlowCtx {
                    run_id: Some("run-01".into()),
                    step_id: Some("plan".into()),
                    agent: Some("rupuso".into()),
                    workspace_id: Some("ws-1".into()),
                    origin: Origin::Provider("anthropic".into()),
                },
                fidelity: Fidelity::Http,
                method: "POST".into(),
                scheme: "https".into(),
                host: "api.anthropic.com".into(),
                port: 443,
                path: "/v1/messages".into(),
                peer_ip: None,
                resolved_ips: vec![],
                http_version: Some("HTTP/1.1".into()),
                status: Some(200),
                outcome: Outcome::Ok,
                error: None,
                bytes_out: Some(512),
                bytes_in: Some(2048),
                body_complete: true,
                ttfb_ms: Some(120),
                duration_ms: Some(430),
            }),
        },
        // Schema v2 (transcript fidelity plan 1, Task 5): real reasoning
        // block, byte-exact `raw` (signature intact) for replay.
        rupu_transcript::Event::Thinking {
            text: Some("Weighing whether to run the full suite before committing.".into()),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            raw: serde_json::json!({
                "type": "thinking",
                "thinking": "Weighing whether to run the full suite before committing.",
                "signature": "sig-abc123",
            }),
        },
        // Redacted counterpart: `text: None` — the block existed but its
        // content is not human-readable; `raw` still carries the opaque
        // provider payload for replay.
        rupu_transcript::Event::Thinking {
            text: None,
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            raw: serde_json::json!({
                "type": "redacted_thinking",
                "data": "opaque-base64-blob",
            }),
        },
        rupu_transcript::Event::ThinkingDelta {
            content: "Weighing whether".into(),
        },
        rupu_transcript::Event::UserMessage {
            content: "Please review the auth module for security issues.".into(),
        },
        // Seed form 1: vouched-for by a prior transcript (chain resolves
        // recursively) — `messages` absent.
        rupu_transcript::Event::Seed {
            message_count: 7,
            sha256: "cd".repeat(32),
            source_transcript: Some("t/run_prev.jsonl".into()),
            messages: None,
        },
        // Seed form 2: inline fallback — full `Vec<Message>` embedded,
        // `source_transcript` absent.
        rupu_transcript::Event::Seed {
            message_count: 3,
            sha256: "ab".repeat(32),
            source_transcript: None,
            messages: Some(serde_json::json!([
                { "role": "user", "content": [{ "type": "text", "text": "start the review" }] }
            ])),
        },
        rupu_transcript::Event::Compaction {
            seq: 1,
            summarized_messages: 12,
            backup_path: "t/run-01.compacted.1.json".into(),
            messages: serde_json::json!([
                { "role": "user", "content": [{ "type": "text", "text": "continue the review" }] }
            ]),
        },
        rupu_transcript::Event::Notice {
            kind: "context_trim".into(),
            message: "Trimmed 3 tool results to fit the context window".into(),
        },
        rupu_transcript::Event::Notice {
            kind: "provider_retry".into(),
            message: "Retrying after a 529 overloaded response".into(),
        },
    ];
    assert_transcript_events_cover_every_variant(&events);
    check_fixture("transcript_events.json", &events);
}

/// Exhaustiveness guard for `rupu_transcript::Event`, mirroring
/// `assert_events_cover_every_variant`'s guard for the orchestrator's
/// `Event` above — a new transcript variant must fail to compile here
/// until a case is added.
///
/// adding a variant? extend transcript_events_fixture_is_current and
/// regenerate via make macos-fixtures
fn assert_transcript_events_cover_every_variant(events: &[rupu_transcript::Event]) {
    for event in events {
        match event {
            rupu_transcript::Event::RunStart { .. } => {}
            rupu_transcript::Event::TurnStart { .. } => {}
            rupu_transcript::Event::AssistantDelta { .. } => {}
            rupu_transcript::Event::AssistantMessage { .. } => {}
            rupu_transcript::Event::ToolCall { .. } => {}
            rupu_transcript::Event::ToolResult { .. } => {}
            rupu_transcript::Event::FileEdit { .. } => {}
            rupu_transcript::Event::CommandRun { .. } => {}
            rupu_transcript::Event::ActionEmitted { .. } => {}
            rupu_transcript::Event::GateRequested { .. } => {}
            rupu_transcript::Event::TurnEnd { .. } => {}
            rupu_transcript::Event::Usage { .. } => {}
            rupu_transcript::Event::RunComplete { .. } => {}
            rupu_transcript::Event::ToolAudit { .. } => {}
            rupu_transcript::Event::NetFlow { .. } => {}
            rupu_transcript::Event::Thinking { .. } => {}
            rupu_transcript::Event::ThinkingDelta { .. } => {}
            rupu_transcript::Event::UserMessage { .. } => {}
            rupu_transcript::Event::Seed { .. } => {}
            rupu_transcript::Event::Compaction { .. } => {}
            rupu_transcript::Event::Notice { .. } => {}
            // `Unknown` is the forward-compatibility catch-all for an
            // unrecognized `type` tag (see event.rs's doc comment) — it is
            // never constructed by a writer, so there is nothing to put in
            // the sample vec above and no fixture row for it. Kept as an
            // explicit arm (not folded into a wildcard) so this match stays
            // an exhaustiveness guard: a genuinely new variant still fails
            // to compile here until a case is added for it.
            rupu_transcript::Event::Unknown => {}
        }
    }
}

#[test]
fn run_detail_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let mut run = RunRecord {
        status: RunStatus::AwaitingApproval,
        awaiting: vec![AwaitingGate {
            step_id: "gate".into(),
            prompt: Some("deploy?".into()),
            since: t,
            expires_at: Some(t + chrono::Duration::hours(1)),
        }],
        active_step_id: Some("gate".into()),
        active_step_kind: Some(StepKind::ApprovalGate),
        ..sample_run_record("run-01", t)
    };
    run.sync_awaiting_compat();

    let step_linear = StepResultRecord {
        step_id: "plan".into(),
        run_id: "run-01".into(),
        transcript_path: PathBuf::from("t/plan.jsonl"),
        output: "plan complete".into(),
        success: true,
        skipped: false,
        rendered_prompt: "Plan the work".into(),
        kind: StepKind::Linear,
        items: vec![],
        findings: vec![],
        iterations: 0,
        resolved: true,
        finished_at: t,
        loop_iteration: None,
        run_outcome: None,
        host: None,
    };
    let step_panel = StepResultRecord {
        step_id: "review".into(),
        run_id: "run-01".into(),
        transcript_path: PathBuf::from("t/review.jsonl"),
        output: "panel review complete".into(),
        success: true,
        skipped: false,
        rendered_prompt: "Review the change".into(),
        kind: StepKind::Panel,
        items: vec![],
        findings: vec![FindingRecord {
            source: "panelist-a".into(),
            severity: "high".into(),
            title: "Missing null check".into(),
            body: "Potential panic on None".into(),
        }],
        iterations: 2,
        resolved: true,
        finished_at: t,
        loop_iteration: None,
        run_outcome: None,
        host: None,
    };

    let usage = UsageSummary {
        input_tokens: 5000,
        output_tokens: 1200,
        cached_tokens: 300,
        total_tokens: 6200,
        cost_usd: Some(0.85),
        priced: true,
        runs: 2,
    };

    let value = serde_json::json!({
        "run": run,
        "steps": [step_linear, step_panel],
        "usage": usage,
    });
    check_fixture("run_detail.json", &value);
}

#[test]
fn run_graph_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let run = RunRecord {
        active_step_id: Some("review".into()),
        active_step_kind: Some(StepKind::Panel),
        ..sample_run_record("run-02", t)
    };

    // One node per kind — precedence order per `graph.rs::map_step`:
    // parallel > panel > gate > action > for_each/run > step.
    let workflow = StepDag {
        steps: vec![
            StepNodeDto {
                id: "plan".into(),
                kind: "step".into(),
                agent: Some("rupuso".into()),
                for_each: None,
                parallel: None,
                panelists: None,
                gate: None,
                action: None,
                approval_gate: None,
            },
            StepNodeDto {
                id: "fan".into(),
                kind: "for_each".into(),
                agent: Some("reviewer".into()),
                for_each: Some("{{ files }}".into()),
                parallel: None,
                panelists: None,
                gate: None,
                action: None,
                approval_gate: None,
            },
            StepNodeDto {
                id: "par".into(),
                kind: "parallel".into(),
                agent: None,
                for_each: None,
                parallel: Some(vec![
                    SubStepDto {
                        id: "par_a".into(),
                        agent: "alpha".into(),
                    },
                    SubStepDto {
                        id: "par_b".into(),
                        agent: "beta".into(),
                    },
                ]),
                panelists: None,
                gate: None,
                action: None,
                approval_gate: None,
            },
            StepNodeDto {
                id: "review".into(),
                kind: "panel".into(),
                agent: None,
                for_each: None,
                parallel: None,
                panelists: Some(vec!["panelist-a".into(), "panelist-b".into()]),
                gate: Some(GateDto {
                    max_iterations: 5,
                    until_severity: "high".into(),
                    fix_with: "fixer".into(),
                }),
                action: None,
                approval_gate: None,
            },
            StepNodeDto {
                id: "approve".into(),
                kind: "gate".into(),
                agent: None,
                for_each: None,
                parallel: None,
                panelists: None,
                gate: None,
                action: None,
                approval_gate: Some(ApprovalGateDto {
                    auto_approve: true,
                    has_on_reject: true,
                    timeout_seconds: Some(3600),
                }),
            },
            StepNodeDto {
                id: "create_pr".into(),
                kind: "action".into(),
                agent: None,
                for_each: None,
                parallel: None,
                panelists: None,
                gate: None,
                action: Some("scm.prs.create".into()),
                approval_gate: None,
            },
            StepNodeDto {
                id: "build".into(),
                kind: "run".into(),
                agent: None,
                for_each: Some("{{ targets }}".into()),
                parallel: None,
                panelists: None,
                gate: None,
                action: Some("cargo build".into()),
                approval_gate: None,
            },
        ],
    };

    let step_linear = StepResultRecord {
        step_id: "plan".into(),
        run_id: "run-02".into(),
        transcript_path: PathBuf::from("t/plan.jsonl"),
        output: "plan complete".into(),
        success: true,
        skipped: false,
        rendered_prompt: "Plan the work".into(),
        kind: StepKind::Linear,
        items: vec![],
        findings: vec![],
        iterations: 0,
        resolved: true,
        finished_at: t,
        loop_iteration: None,
        run_outcome: None,
        host: None,
    };

    // One durable checkpoint (`success: true`) plus one events-only
    // synthesized unit (`success: null`) — hand-built `json!` to lock the
    // shape mismatch `merge_event_units` (graph.rs) produces: a
    // synthesized unit carries only `step_id`/`index`/`item`/
    // `transcript_path`/`success`, never the checkpoint's full field set.
    let checkpoint_unit = UnitCheckpoint {
        step_id: "fan".into(),
        index: 0,
        item: serde_json::json!("crates/a"),
        run_id: "sub_01".into(),
        transcript_path: PathBuf::from("t/u0.jsonl"),
        output: "reviewed".into(),
        success: true,
        finished_at: t,
        host: None,
    };
    let synthesized_unit = serde_json::json!({
        "step_id": "fan",
        "index": 1,
        "item": "crates/b",
        "transcript_path": "t/u1.jsonl",
        "success": null,
    });

    let usage = UsageSummary {
        input_tokens: 3000,
        output_tokens: 700,
        cached_tokens: 100,
        total_tokens: 3700,
        cost_usd: Some(0.42),
        priced: true,
        runs: 1,
    };

    let value = serde_json::json!({
        "run": run,
        "workflow": workflow,
        "step_results": [step_linear],
        "units": [
            serde_json::to_value(&checkpoint_unit).expect("serialize checkpoint unit"),
            synthesized_unit,
        ],
        "usage": usage,
    });
    check_fixture("run_graph.json", &value);
}

// ── Write-path fixtures (Phase 3) ───────────────────────────────────────────
//
// Response shapes hand-built here (not behind a private DTO) — the launch
// responses and the approve/reject/cancel error shapes are plain
// `serde_json::json!` objects the handlers construct inline (see
// `api/agents.rs::run_agent`/`start_session`, `api/workflows.rs::launch_run`,
// `api/runs.rs::approve_run`/`reject_run`/`cancel_run`), so there's no DTO
// type to import — mirroring them here locks the wire shape the Swift app
// decodes. The request-body round-trip tests for the private `*Body` structs
// live in-module next to their types instead (see `api/runs.rs`,
// `api/agents.rs`, `api/workflows.rs`, `api/sessions.rs`).

#[test]
fn launch_responses_fixture_is_current() {
    // Mirrors, in order: `run_agent`'s local-launch response
    // (`{"run_id","host_id"}`), `start_session`'s local-start response
    // (`{"session_id","host_id"}`), and any remote-proxied control response
    // (`{"ok":true,"host_id"}` — shared by approve/reject/cancel/pause/
    // resume/archive/restore/send when `?host=<remote>` is set).
    let value = serde_json::json!([
        { "run_id": "run-01", "host_id": "local" },
        { "session_id": "ses-01", "host_id": "local" },
        { "ok": true, "host_id": "mini" },
    ]);
    check_fixture("launch_responses.json", &value);
}

#[test]
fn run_control_response_fixture_is_current() {
    // The shape `run_response()` (api/runs.rs) produces for a local
    // approve/reject/cancel — reload the record, attach steps + usage, then
    // inject `host_id: "local"`. Reject/cancel are IMMEDIATE (status flips in
    // the response, unlike approve's marker-only design — see the API facts
    // doc) — a `cancelled` record models that.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let record = RunRecord {
        status: RunStatus::Cancelled,
        finished_at: Some(t + chrono::Duration::minutes(5)),
        error_message: Some("Cancelled from control plane".into()),
        ..sample_run_record("run-01", t)
    };
    let steps: Vec<StepResultRecord> = Vec::new();
    let usage = UsageSummary::default();
    let mut value = serde_json::json!({ "run": record, "steps": steps, "usage": usage });
    value["host_id"] = serde_json::json!("local");
    check_fixture("run_control_response.json", &value);
}

#[test]
fn api_errors_fixture_is_current() {
    // `{"error": "<message>"}` — the shape every 400/404/409/500/501
    // `ApiError` serializes to. `AmbiguousGate`'s message embeds the parked
    // gate candidates (see `ApprovalError::AmbiguousGate` / `map_approval_err`
    // in api/runs.rs); `session ... is stopped` mirrors `send_session`'s 409.
    let value = serde_json::json!([
        { "error": "run x not found" },
        { "error": "ambiguous gate: candidates [a, b]" },
        { "error": "session s is stopped" },
    ]);
    check_fixture("api_errors.json", &value);
}

// ── Flows & composition (v2 top bar) ────────────────────────────────────────

#[test]
fn projects_fixture_is_current() {
    // `GET /api/projects` (`list_projects` in api/projects.rs) returns
    // `Vec<ProjectRow>` directly — this mirrors the exact type, not a
    // hand-built `json!`, so drift on `ProjectRow` itself always shows up
    // here rather than only surfacing in a live `cp serve` response.
    let row = ProjectRow {
        ws_id: "ws-1".into(),
        name: "rupu".into(),
        path: "/Users/matt/Code/rupu".into(),
        repo_remote: Some("git@github.com:section9labs/rupu.git".into()),
        branch: Some("main".into()),
        repo_home_url: Some("https://github.com/section9labs/rupu".into()),
        created_at: "2026-08-01T09:00:00Z".into(),
        last_run_at: Some("2026-08-20T12:00:00Z".into()),
        usage: UsageSummary {
            input_tokens: 5000,
            output_tokens: 1200,
            cached_tokens: 300,
            total_tokens: 6200,
            cost_usd: Some(0.85),
            priced: true,
            runs: 2,
        },
        run_count: 14,
        last_active: Some("2026-08-20T12:00:00Z".into()),
    };
    check_fixture("projects.json", &vec![row]);
}

// ── Overview dashboard (Phase 4) ────────────────────────────────────────────

#[test]
fn dashboard_fixture_is_current() {
    // `GET /api/dashboard` (`get_dashboard` in api/dashboard.rs) returns a
    // `DashboardResponse` that `#[serde(flatten)]`s a `DashboardSummary` at
    // the top level alongside `hosts`/`findings_partial`/`cycles_partial`/
    // `fleet_partial`. `DashboardResponse`/`HostFreshness` are private to
    // that module (no DTO to import), so this builds the real, public
    // `DashboardSummary` (and its nested pub types) and merges in the
    // hand-built `hosts`/`*_partial` fields — mirroring the flatten exactly,
    // the same approach `run_detail_fixture_is_current` above uses for
    // private response shapes.
    //
    // Fixture shape: 2 hosts (one `ok`, one `offline`); `findings_open` is
    // the poisoned field (`None` with `findings_partial: true`) while
    // `cycles`/`fleet` are fully reported (`cycles_partial`/`fleet_partial:
    // false`); non-empty terminal/throughput buckets; `active_longest`
    // present; `fleet.issues_capped: true`.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let summary = DashboardSummary {
        active: ActiveCounts {
            running: 3,
            awaiting_approval: 1,
            paused: 0,
            pending: 2,
        },
        active_longest: Some(ActiveLongest {
            run_id: "run-01".into(),
            workflow_name: "nightly-health".into(),
            age_ms: 120_000,
        }),
        terminal_buckets: vec![
            TerminalBucket {
                ts: t,
                completed: 4,
                failed: 1,
                rejected: 0,
                cancelled: 0,
            },
            TerminalBucket {
                ts: t + chrono::Duration::days(1),
                completed: 2,
                failed: 0,
                rejected: 1,
                cancelled: 0,
            },
        ],
        throughput_buckets: vec![
            ThroughputBucket {
                ts: t,
                manual: 2,
                cron: 3,
                event: 0,
            },
            ThroughputBucket {
                ts: t + chrono::Duration::days(1),
                manual: 1,
                cron: 1,
                event: 1,
            },
        ],
        cycles: CycleCounts {
            total: 10,
            clean: Some(8),
            with_failures: Some(2),
        },
        // The poisoned field: this host merge omitted findings entirely.
        findings_open: None,
        fleet: FleetCounts {
            repos: Some(5),
            providers_configured: Some(2),
            providers_unhealthy: Some(0),
            autoflows_enabled: Some(3),
            autoflows_disabled: Some(1),
            workers: Some(2),
            claims_active: Some(4),
            issues_pending: Some(1),
            issues_open: Some(120),
            issues_capped: true,
            inventory_captured_at: Some(t),
        },
        captured_at: t,
    };

    let mut value = serde_json::to_value(&summary).expect("serialize dashboard summary");
    let obj = value
        .as_object_mut()
        .expect("DashboardSummary serializes to a JSON object");
    obj.insert(
        "hosts".into(),
        serde_json::json!([
            {
                "host_id": "mini",
                "name": "mini",
                "transport_kind": "local",
                "state": "ok",
                "captured_at": t,
                "reason": null,
            },
            {
                "host_id": "kuki",
                "name": "kuki",
                "transport_kind": "ssh",
                "state": "offline",
                "captured_at": null,
                "reason": "connection refused",
            },
        ]),
    );
    obj.insert("findings_partial".into(), serde_json::json!(true));
    obj.insert("cycles_partial".into(), serde_json::json!(false));
    obj.insert("fleet_partial".into(), serde_json::json!(false));

    check_fixture("dashboard.json", &value);
}

// ── Fleet & project detail (Phase 5A) ───────────────────────────────────────
//
// `WorkerView` (api/workers.rs), `AutoflowDefRow` (api/autoflows.rs), and
// `ProjectDetail`/`SessionDto` (api/projects.rs, api/sessions.rs) are private
// (or `pub(crate)`) to their modules — no DTO to import across the crate
// boundary from an integration test. Each fixture below hand-builds the
// exact wire shape instead, mirroring the pattern `run_detail_fixture_is_current`
// and `dashboard_fixture_is_current` already use for the same reason. Where a
// `pub` type exists (`WorkerRecord` from `rupu_runtime`, `ProjectRow`/
// `RunListRow`/`UsageSummary` from `rupu_cp`), it's serialized for real
// rather than hand-typed, so drift on THOSE types still shows up here.

#[test]
fn workers_fixture_is_current() {
    // Mirrors `list_workers` (api/workers.rs): `WorkerView` is
    // `#[serde(flatten)] WorkerRecord` plus `active_run_count` /
    // `total_run_count` / `last_run_at`. Two rows: one with full
    // capabilities and run activity, one with every `WorkerCapabilities`
    // list empty (exercising the `skip_serializing_if = "Vec::is_empty"`
    // omission) and no run activity at all (`last_run_at: null`, both
    // counts `0`).
    let busy = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: "worker_local_team-mini_cli".into(),
        kind: WorkerKind::Cli,
        name: "team-mini".into(),
        host: "team-mini.local".into(),
        capabilities: WorkerCapabilities {
            backends: vec!["local_worktree".into()],
            scm_hosts: vec!["github".into()],
            permission_modes: vec!["bypass".into(), "readonly".into()],
        },
        registered_at: "2026-08-19T16:00:00Z".into(),
        last_seen_at: "2026-08-20T12:22:00Z".into(),
    };
    let mut busy_value = serde_json::to_value(&busy).expect("serialize WorkerRecord");
    {
        let obj = busy_value
            .as_object_mut()
            .expect("WorkerRecord is an object");
        obj.insert("active_run_count".into(), serde_json::json!(2));
        obj.insert("total_run_count".into(), serde_json::json!(9));
        obj.insert(
            "last_run_at".into(),
            serde_json::json!("2026-08-20T12:20:00Z"),
        );
    }

    let idle = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: "worker_local_kuki_autoflow_serve".into(),
        kind: WorkerKind::AutoflowServe,
        name: "kuki".into(),
        host: "kuki.local".into(),
        capabilities: WorkerCapabilities::default(),
        registered_at: "2026-08-20T09:00:00Z".into(),
        last_seen_at: "2026-08-20T09:05:00Z".into(),
    };
    let mut idle_value = serde_json::to_value(&idle).expect("serialize WorkerRecord");
    {
        let obj = idle_value
            .as_object_mut()
            .expect("WorkerRecord is an object");
        obj.insert("active_run_count".into(), serde_json::json!(0));
        obj.insert("total_run_count".into(), serde_json::json!(0));
        obj.insert("last_run_at".into(), serde_json::Value::Null);
    }

    check_fixture("workers.json", &vec![busy_value, idle_value]);
}

#[test]
fn autoflow_defs_fixture_is_current() {
    // Mirrors `AutoflowDefRow` (api/autoflows.rs): `{name, slug, trigger,
    // scope, scope_kind, scope_id, enabled}`. One global row (`scope_id:
    // null`), one project row (`scope_id` set) — also covers a `false`
    // `enabled` (a disabled def is listed, not filtered out — see
    // `scan_autoflow_defs`'s doc comment).
    let value = serde_json::json!([
        {
            "name": "nightly-health",
            "slug": "nightly-health",
            "trigger": "cron",
            "scope": "global",
            "scope_kind": "global",
            "scope_id": null,
            "enabled": true,
        },
        {
            "name": "issue-triage",
            "slug": "issue-triage-v2",
            "trigger": "event",
            "scope": "rupu",
            "scope_kind": "project",
            "scope_id": "ws-1",
            "enabled": false,
        },
    ]);
    check_fixture("autoflow_defs.json", &value);
}

#[test]
fn autoflow_set_enabled_fixture_is_current() {
    // Mirrors `SetEnabledResponse` (api/autoflows.rs, `pub(crate)` — no DTO
    // to import across the crate boundary, same rationale
    // `workers_fixture_is_current`'s doc comment gives): `{name, enabled}`,
    // the response `POST /api/autoflows/:name/enable`/`.../disable` return.
    // `enabled: true` here (a disable response is the identical shape with
    // `enabled: false` — nothing else varies, so one fixture case covers
    // both wire values the Swift decode test needs to exercise via a
    // literal `false` constructed in-test).
    let value = serde_json::json!({
        "name": "nightly-health",
        "enabled": true,
    });
    check_fixture("autoflow_set_enabled.json", &value);
}

#[test]
fn project_detail_fixture_is_current() {
    // Mirrors `get_project` (api/projects.rs): typed `project` (`ProjectRow`)
    // and `recent_runs` (`Vec<RunListRow>`) alongside ad-hoc `runs` /
    // `sessions` / `coverage` objects and a project-level `usage` rollup.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let project = ProjectRow {
        ws_id: "ws-1".into(),
        name: "rupu".into(),
        path: "/Users/matt/Code/rupu".into(),
        repo_remote: Some("git@github.com:section9labs/rupu.git".into()),
        branch: Some("main".into()),
        repo_home_url: Some("https://github.com/section9labs/rupu".into()),
        created_at: "2026-08-01T09:00:00Z".into(),
        last_run_at: Some("2026-08-20T12:00:00Z".into()),
        usage: UsageSummary {
            input_tokens: 5000,
            output_tokens: 1200,
            cached_tokens: 300,
            total_tokens: 6200,
            cost_usd: Some(0.85),
            priced: true,
            runs: 2,
        },
        run_count: 14,
        last_active: Some("2026-08-20T12:00:00Z".into()),
    };

    let recent_runs = vec![
        RunListRow {
            id: "run-01".into(),
            workflow_name: "nightly-health".into(),
            status: RunStatus::Completed,
            started_at: t,
            finished_at: Some(t + chrono::Duration::minutes(6)),
            trigger: "cron",
            usage: UsageSummary {
                input_tokens: 1000,
                output_tokens: 200,
                cached_tokens: 0,
                total_tokens: 1200,
                cost_usd: Some(0.12),
                priced: true,
                runs: 1,
            },
            turns: 4,
            duration_ms: Some(360_000),
        },
        RunListRow {
            id: "run-02".into(),
            workflow_name: "issue-triage".into(),
            status: RunStatus::Running,
            started_at: t + chrono::Duration::hours(1),
            finished_at: None,
            trigger: "event",
            usage: UsageSummary::default(),
            turns: 0,
            duration_ms: None,
        },
    ];

    let usage = UsageSummary {
        input_tokens: 5000,
        output_tokens: 1200,
        cached_tokens: 300,
        total_tokens: 6200,
        cost_usd: Some(0.85),
        priced: true,
        runs: 2,
    };

    let value = serde_json::json!({
        "project": project,
        "runs": {
            "total": 14,
            "running": 1,
            "by_status": {
                "awaiting_approval": 1,
                "cancelled": 0,
                "completed": 10,
                "failed": 2,
                "running": 1,
            },
            "by_surface": { "workflow": 9, "autoflow": 5 },
        },
        "sessions": { "total": 3, "active": 1 },
        "coverage": { "targets": 4, "findings": 7 },
        "recent_runs": recent_runs,
        "usage": usage,
    });
    check_fixture("project_detail.json", &value);
}

#[test]
fn project_runs_fixture_is_current() {
    // Mirrors `project_runs` (api/projects.rs) — `Vec<RunListRow>` returned
    // AS-IS with no `host_id` injected (unlike the fleet-wide `run_list_row`
    // fixture, this endpoint never proxies to a remote host — see
    // `resolve_workflow_scoped`'s doc comment on why project routes stay
    // local-only).
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let rows = vec![RunListRow {
        id: "run-01".into(),
        workflow_name: "nightly-health".into(),
        status: RunStatus::Completed,
        started_at: t,
        finished_at: Some(t + chrono::Duration::minutes(6)),
        trigger: "cron",
        usage: UsageSummary {
            input_tokens: 1000,
            output_tokens: 200,
            cached_tokens: 0,
            total_tokens: 1200,
            cost_usd: Some(0.12),
            priced: true,
            runs: 1,
        },
        turns: 4,
        duration_ms: Some(360_000),
    }];
    check_fixture("project_runs.json", &rows);
}

#[test]
fn project_sessions_fixture_is_current() {
    // Mirrors `project_sessions` (api/projects.rs), which filters
    // `collect_sessions`'s output (`SessionDto` + injected `scope`/`usage`)
    // to one workspace — no `host_id` injected (same local-only rationale as
    // `project_runs`). `SessionDto` is private to `api::sessions`, so this
    // hand-builds the shape rather than importing it.
    let value = serde_json::json!([
        {
            "session_id": "ses-01",
            "agent_name": "rupuso",
            "model": "claude-sonnet-4-6",
            "provider_name": "anthropic",
            "status": "running",
            "total_turns": 6,
            "total_tokens_in": 5000,
            "total_tokens_out": 1200,
            "total_tokens_cached": 300,
            "created_at": "2026-08-20T11:00:00Z",
            "updated_at": "2026-08-20T12:00:00Z",
            "active_run_id": "run-02",
            "last_error": null,
            "target": "crates/rupu-cp",
            "workspace_id": "ws-1",
            "scope": "active",
            "usage": {
                "input_tokens": 5000,
                "output_tokens": 1200,
                "cached_tokens": 300,
                "total_tokens": 6200,
                "cost_usd": 0.85,
                "priced": true,
                "runs": 1,
            },
        },
        {
            "session_id": "ses-00",
            "agent_name": "rupuso",
            "model": "claude-sonnet-4-6",
            "provider_name": "anthropic",
            "status": "stopped",
            "total_turns": 12,
            "total_tokens_in": 9000,
            "total_tokens_out": 2100,
            "total_tokens_cached": 0,
            "created_at": "2026-08-15T09:00:00Z",
            "updated_at": "2026-08-15T10:30:00Z",
            "active_run_id": null,
            "last_error": null,
            "target": null,
            "workspace_id": "ws-1",
            "scope": "archived",
            "usage": {
                "input_tokens": 9000,
                "output_tokens": 2100,
                "cached_tokens": 0,
                "total_tokens": 11100,
                "cost_usd": null,
                "priced": false,
                "runs": 1,
            },
        },
    ]);
    check_fixture("project_sessions.json", &value);
}

// ── Security & Usage (Phase 5B) ─────────────────────────────────────────────
//
// `FindingsResponse`/`FindingOut`/`FindingsSummary` (api/findings.rs) and
// `OutlierRun` (api/usage_outliers.rs) are `pub` — mirrored for real below.
// `CoverageSummary`, the `/api/coverage/:target` detail shape, and
// `UsageResponse`/`UnpricedGap`/`HostFreshness`/`UsageRunRow` (all in
// api/coverage.rs / api/usage.rs) are private to their modules — hand-built
// `json!` per the `project_detail_fixture_is_current` precedent above,
// embedding the REAL `rupu_cp::usage::{UsageSummary, UsageBreakdownRow}` and
// `rupu_coverage::{ConcernAssertion, FindingRecord, FileView, FlatCatalog}`
// types wherever one exists so drift on THOSE types still shows up here.

/// A `rupu_coverage::Attribution` pinned to a run id, model held constant —
/// shared by every coverage/finding fixture below.
fn coverage_attribution(run_id: &str) -> CoverageAttribution {
    CoverageAttribution {
        run_id: run_id.into(),
        model: "claude-sonnet-4-6".into(),
        surface: CoverageSurface::Workflow,
    }
}

#[test]
fn findings_global_fixture_is_current() {
    // `GET /api/findings` with no filters: 4 findings across 4 severities
    // from 2 workspaces, one long-form critical write-up (the "critical"/
    // "medium" wire-string case the brief calls out), a permalink on two
    // rows, and one finding with no joined `workflow_name` (unresolved run).
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let findings = vec![
        FindingOut {
            ws_id: "ws-1".into(),
            project: "rupu".into(),
            target_id: "auth-core".into(),
            workflow_name: Some("nightly-security".into()),
            permalink: Some(
                "https://github.com/section9labs/rupu/blob/main/src/auth/session.rs#L42-L58".into(),
            ),
            record: CoverageFindingRecord {
                id: "fnd_critical_1".into(),
                file_path: Some("src/auth/session.rs".into()),
                line_range: Some([42, 58]),
                target_ref: None,
                scope: CoverageFindingScope::Line,
                summary: "Session token comparison uses non-constant-time equality".into(),
                severity: CoverageSeverity::Critical,
                concern_id: Some("timing-attack".into()),
                evidence: CoverageFindingEvidence {
                    code_excerpt: Some("if token == stored_token {".into()),
                    rationale: "The session token is compared with `==`, a non-constant-time \
                                equality check. An attacker with network access to this \
                                unauthenticated, rate-limit-exempt health endpoint can recover \
                                a valid session token one byte at a time by measuring response \
                                latency across repeated requests, eventually forging a fully \
                                authenticated session without ever guessing the token outright."
                        .into(),
                    references: vec!["https://cwe.mitre.org/data/definitions/208.html".into()],
                },
                declared_by: coverage_attribution("run_9k2f"),
                declared_at: t,
            },
        },
        FindingOut {
            ws_id: "ws-1".into(),
            project: "rupu".into(),
            target_id: "web-api".into(),
            workflow_name: Some("nightly-security".into()),
            permalink: None,
            record: CoverageFindingRecord {
                id: "fnd_high_1".into(),
                file_path: None,
                line_range: None,
                target_ref: None,
                scope: CoverageFindingScope::Repo,
                summary: "No rate limiting on the public API surface".into(),
                severity: CoverageSeverity::High,
                concern_id: Some("no-rate-limit".into()),
                evidence: CoverageFindingEvidence {
                    code_excerpt: None,
                    rationale: "repo-wide scan found no rate-limiting middleware".into(),
                    references: vec![],
                },
                declared_by: coverage_attribution("run_9k2f"),
                declared_at: t - chrono::Duration::hours(1),
            },
        },
        FindingOut {
            ws_id: "ws-2".into(),
            project: "phi-cell".into(),
            target_id: "ml-pipeline".into(),
            workflow_name: None,
            permalink: Some(
                "https://github.com/section9labs/phi-cell/blob/main/src/train.rs#L100-L112".into(),
            ),
            record: CoverageFindingRecord {
                id: "fnd_medium_1".into(),
                file_path: Some("src/train.rs".into()),
                line_range: Some([100, 112]),
                target_ref: None,
                scope: CoverageFindingScope::Line,
                summary: "Model checkpoint path built from an unsanitized config value, \
                          allowing a malicious workflow input to write outside the checkpoint \
                          directory via a crafted `cfg.name` containing `../` segments."
                    .into(),
                severity: CoverageSeverity::Medium,
                concern_id: Some("path-traversal".into()),
                evidence: CoverageFindingEvidence {
                    code_excerpt: Some("let p = format!(\"{}/{}\", base, cfg.name);".into()),
                    rationale: "cfg.name is operator-supplied and not sanitized".into(),
                    references: vec![],
                },
                declared_by: coverage_attribution("run_am4d"),
                declared_at: t - chrono::Duration::days(1),
            },
        },
        FindingOut {
            ws_id: "ws-2".into(),
            project: "phi-cell".into(),
            target_id: "ml-pipeline".into(),
            workflow_name: Some("weekly-audit".into()),
            permalink: None,
            record: CoverageFindingRecord {
                id: "fnd_info_1".into(),
                file_path: None,
                line_range: None,
                target_ref: None,
                scope: CoverageFindingScope::Repo,
                summary: "No CODEOWNERS file configured".into(),
                severity: CoverageSeverity::Info,
                concern_id: None,
                evidence: CoverageFindingEvidence {
                    code_excerpt: None,
                    rationale: "repository has no CODEOWNERS file".into(),
                    references: vec![],
                },
                declared_by: coverage_attribution("run_am4d"),
                declared_at: t - chrono::Duration::days(2),
            },
        },
    ];

    let response = FindingsResponse {
        summary: FindingsSummary {
            total: findings.len(),
            critical: 1,
            high: 1,
            medium: 1,
            low: 0,
            info: 1,
        },
        findings,
    };
    check_fixture("findings_global.json", &response);
}

#[test]
fn coverage_summary_fixture_is_current() {
    // `GET /api/coverage` (`list_coverage`, api/coverage.rs): `CoverageSummary`
    // is private to that module, so this hand-builds the exact wire shape —
    // 3 targets across 2 projects, one with `has_catalog: false` and one with
    // zero findings.
    let value = serde_json::json!([
        {
            "ws_id": "ws-1",
            "project": "rupu",
            "target_id": "auth-core",
            "assertion_lines": 128,
            "has_catalog": true,
            "findings": 3,
        },
        {
            "ws_id": "ws-1",
            "project": "rupu",
            "target_id": "web-api",
            "assertion_lines": 64,
            "has_catalog": false,
            "findings": 0,
        },
        {
            "ws_id": "ws-2",
            "project": "phi-cell",
            "target_id": "ml-pipeline",
            "assertion_lines": 40,
            "has_catalog": true,
            "findings": 5,
        },
    ]);
    check_fixture("coverage_summary.json", &value);
}

#[test]
fn coverage_detail_fixture_is_current() {
    // `GET /api/coverage/:target` (`get_coverage`, api/coverage.rs) returns a
    // hand-built `json!({ws_id, project, target_id, assertion_lines,
    // has_catalog, assertions, findings, files})` — exactly those 8 keys,
    // NEVER a `catalog` key (confirmed against `get_coverage`'s body). This
    // fixture mirrors that exactly, using the REAL `ConcernAssertion` /
    // `rupu_coverage::FindingRecord` / `FileView` types. The target's
    // `FlatCatalog` is served by the SEPARATE `GET /api/coverage/:target/catalog`
    // route (`get_catalog`) and has its own fixture — see
    // `coverage_catalog_fixture_is_current` below; Task 2 should decode it as
    // an independent Swift type, not a field on `APICoverageDetail`.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let assertions = vec![
        ConcernAssertion {
            concern_id: "timing-attack".into(),
            file_path: "src/auth/session.rs".into(),
            status: AssertionStatus::Finding,
            evidence: CoverageEvidence {
                summary: "Token compared with `==`, not constant-time".into(),
                line_ranges: vec![[42, 58]],
                finding_ids: vec!["fnd_critical_1".into()],
            },
            declared_by: coverage_attribution("run_9k2f"),
            declared_at: t,
        },
        ConcernAssertion {
            concern_id: "sql-injection".into(),
            file_path: "src/auth/login.rs".into(),
            status: AssertionStatus::Clean,
            evidence: CoverageEvidence {
                summary: "All queries are parameterized".into(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
            declared_by: coverage_attribution("run_9k2f"),
            declared_at: t - chrono::Duration::hours(1),
        },
    ];

    let findings = vec![CoverageFindingRecord {
        id: "fnd_critical_1".into(),
        file_path: Some("src/auth/session.rs".into()),
        line_range: Some([42, 58]),
        target_ref: None,
        scope: CoverageFindingScope::Line,
        summary: "Session token comparison uses non-constant-time equality".into(),
        severity: CoverageSeverity::Critical,
        concern_id: Some("timing-attack".into()),
        evidence: CoverageFindingEvidence {
            code_excerpt: Some("if token == stored_token {".into()),
            rationale: "non-constant-time comparison enables a timing side-channel".into(),
            references: vec!["https://cwe.mitre.org/data/definitions/208.html".into()],
        },
        declared_by: coverage_attribution("run_9k2f"),
        declared_at: t,
    }];

    let files = vec![FileView {
        path: "src/auth/session.rs".into(),
        touch_modes: vec![TouchStrength::Read, TouchStrength::Edit],
        strongest: TouchStrength::Edit,
        read_lines: vec![[1, 120], [42, 58]],
        grep_matches: 0,
        edits: 1,
        first_at: t - chrono::Duration::hours(2),
        last_at: t,
        touched_by: vec![coverage_attribution("run_9k2f")],
    }];

    let value = serde_json::json!({
        "ws_id": "ws-1",
        "project": "rupu",
        "target_id": "auth-core",
        "assertion_lines": 128,
        "has_catalog": true,
        "assertions": assertions,
        "findings": findings,
        "files": files,
    });
    check_fixture("coverage_detail.json", &value);
}

#[test]
fn coverage_catalog_fixture_is_current() {
    // `GET /api/coverage/:target/catalog` (`get_catalog`, api/coverage.rs)
    // returns a BARE `FlatCatalog` at the top level — no wrapper object, no
    // `ws_id`/`target_id` (the target is only in the URL path). `FlatCatalog`
    // is `pub` (`rupu_coverage`) and derives `Serialize`, so this is real,
    // not hand-built. Same 2-concern catalog `coverage_detail_fixture_is_current`
    // used to embed before it was split out to match the real route shape.
    let catalog = FlatCatalog {
        concerns: vec![
            Concern {
                id: "timing-attack".into(),
                name: "Timing side-channel".into(),
                description: "Secret comparisons must be constant-time.".into(),
                severity: CoverageSeverity::Critical,
                applicable_globs: vec!["**/auth/**".into()],
                min_strength: TouchStrength::Read,
                references: vec!["https://cwe.mitre.org/data/definitions/208.html".into()],
                tags: vec!["stride:spoofing".into()],
            },
            Concern {
                id: "sql-injection".into(),
                name: "SQL injection".into(),
                description: "Queries must be parameterized, never string-built.".into(),
                severity: CoverageSeverity::High,
                applicable_globs: vec!["**".into()],
                min_strength: TouchStrength::Read,
                references: vec![],
                tags: vec![],
            },
        ],
        sources: BTreeMap::from([
            ("timing-attack".to_string(), "stride".to_string()),
            ("sql-injection".to_string(), "stride".to_string()),
        ]),
        render_modes: BTreeMap::from([
            ("timing-attack".to_string(), CatalogMode::Full),
            ("sql-injection".to_string(), CatalogMode::Index),
        ]),
    };
    check_fixture("coverage_catalog.json", &catalog);
}

#[test]
fn usage_fixture_is_current() {
    // `GET /api/usage` (`get_usage`, api/usage.rs): `UsageResponse`/
    // `UnpricedGap`/`HostFreshness` are private to that module — hand-built
    // `json!`, embedding the REAL `rupu_cp::usage::{UsageSummary,
    // UsageBreakdownRow}` for the priced pieces. 3 breakdown rows (2 priced
    // models, 1 unpriced), 2 hosts (one ok, one offline) — mirrors
    // `dashboard_fixture_is_current`'s `HostFreshness` shape exactly.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let summary = UsageSummary {
        input_tokens: 1_505_000,
        output_tokens: 305_050,
        cached_tokens: 12_000,
        total_tokens: 1_810_050,
        cost_usd: Some(12.71),
        priced: false, // at least one contributing model is unpriced
        runs: 6,
    };

    let breakdown = vec![
        UsageBreakdownRow {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            agent: "rupuso".into(),
            workflow: String::new(),
            host_id: String::new(),
            workspace_id: String::new(),
            input_tokens: 1_000_000,
            output_tokens: 200_000,
            cached_tokens: 10_000,
            total_tokens: 1_200_000,
            cost_usd: Some(6.0),
            priced: true,
            runs: 4,
        },
        UsageBreakdownRow {
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            agent: "fixer".into(),
            workflow: String::new(),
            host_id: String::new(),
            workspace_id: String::new(),
            input_tokens: 500_000,
            output_tokens: 100_000,
            cached_tokens: 2_000,
            total_tokens: 600_000,
            cost_usd: Some(6.71),
            priced: true,
            runs: 1,
        },
        UsageBreakdownRow {
            provider: "internal-vllm".into(),
            model: "llama-3-70b".into(),
            agent: "explorer".into(),
            workflow: String::new(),
            host_id: String::new(),
            workspace_id: String::new(),
            input_tokens: 5_000,
            output_tokens: 5_050,
            cached_tokens: 0,
            total_tokens: 10_050,
            cost_usd: None,
            priced: false,
            runs: 1,
        },
    ];

    let value = serde_json::json!({
        "summary": summary,
        "breakdown": breakdown,
        "unpriced": {
            "models": ["llama-3-70b"],
            "rows": 1,
        },
        "hosts": [
            {
                "host_id": "local",
                "name": "local",
                "transport_kind": "local",
                "state": "ok",
                "captured_at": t,
                "reason": null,
            },
            {
                "host_id": "kuki",
                "name": "kuki",
                "transport_kind": "ssh",
                "state": "offline",
                "captured_at": null,
                "reason": "connection refused",
            },
        ],
    });
    check_fixture("usage.json", &value);
}

#[test]
fn usage_runs_fixture_is_current() {
    // `GET /api/usage/runs` (`get_usage_runs`, api/usage.rs): `UsageRunRow`
    // is private to that module — hand-built. 6 flat `(run × model)` rows
    // spanning 2 days and 3 models (2 priced + 1 unpriced/nil-cost row, per
    // the brief). `host_id` is always `"local"` — this endpoint is
    // local-only, no host fan-out (see the doc comment on `get_usage_runs`).
    let day1 = Utc.with_ymd_and_hms(2026, 8, 19, 9, 0, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(2026, 8, 20, 9, 0, 0).unwrap();

    let rows = serde_json::json!([
        {
            "run_id": "run-01",
            "started_at": day1,
            "workflow_name": "nightly-health",
            "agent": "rupuso",
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "workspace_id": "ws-1",
            "host_id": "local",
            "input_tokens": 100_000,
            "output_tokens": 20_000,
            "cached_tokens": 1_000,
            "total_tokens": 120_000,
            "cost_usd": 0.12,
            "priced": true,
        },
        {
            "run_id": "run-02",
            "started_at": day1,
            "workflow_name": "nightly-health",
            "agent": "fixer",
            "provider": "anthropic",
            "model": "claude-opus-4-8",
            "workspace_id": "ws-1",
            "host_id": "local",
            "input_tokens": 50_000,
            "output_tokens": 10_000,
            "cached_tokens": 0,
            "total_tokens": 60_000,
            "cost_usd": 0.30,
            "priced": true,
        },
        {
            "run_id": "run-03",
            "started_at": day1,
            "workflow_name": "issue-triage",
            "agent": "rupuso",
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "workspace_id": "ws-2",
            "host_id": "local",
            "input_tokens": 20_000,
            "output_tokens": 4_000,
            "cached_tokens": 500,
            "total_tokens": 24_000,
            "cost_usd": 0.05,
            "priced": true,
        },
        {
            "run_id": "run-04",
            "started_at": day2,
            "workflow_name": "nightly-health",
            "agent": "rupuso",
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "workspace_id": "ws-1",
            "host_id": "local",
            "input_tokens": 150_000,
            "output_tokens": 30_000,
            "cached_tokens": 2_000,
            "total_tokens": 180_000,
            "cost_usd": 0.20,
            "priced": true,
        },
        {
            "run_id": "run-05",
            "started_at": day2,
            "workflow_name": "hotfix",
            "agent": "fixer",
            "provider": "anthropic",
            "model": "claude-opus-4-8",
            "workspace_id": "ws-2",
            "host_id": "local",
            "input_tokens": 80_000,
            "output_tokens": 16_000,
            "cached_tokens": 0,
            "total_tokens": 96_000,
            "cost_usd": 0.40,
            "priced": true,
        },
        {
            "run_id": "run-06",
            "started_at": day2,
            "workflow_name": "experimental",
            "agent": "explorer",
            "provider": "internal-vllm",
            "model": "llama-3-70b",
            "workspace_id": "ws-1",
            "host_id": "local",
            "input_tokens": 5_000,
            "output_tokens": 5_050,
            "cached_tokens": 0,
            "total_tokens": 10_050,
            "cost_usd": null,
            "priced": false,
        },
    ]);
    check_fixture("usage_runs.json", &rows);
}

#[test]
fn usage_outliers_fixture_is_current() {
    // `GET /api/usage/outliers` (api/usage_outliers.rs): `OutlierRun` is
    // `pub` — mirrored for real. 2 outliers across 2 workflows.
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();

    let outliers = vec![
        OutlierRun {
            run_id: "run-06".into(),
            workflow_name: "nightly-health".into(),
            cost_usd: 12.50,
            baseline_usd: 1.20,
            ratio: 12.50 / 1.20,
            started_at: t,
        },
        OutlierRun {
            run_id: "run-09".into(),
            workflow_name: "hotfix".into(),
            cost_usd: 8.0,
            baseline_usd: 2.0,
            ratio: 4.0,
            started_at: t + chrono::Duration::days(1),
        },
    ];
    check_fixture("usage_outliers.json", &outliers);
}

// ── Settings / config (Phase 6A) ────────────────────────────────────────────

#[test]
fn config_view_fixture_is_current() {
    // `GET /api/config` (`get_config`, api/config.rs): `ConfigView`/
    // `RuntimeStatus` were hoisted from private to `pub` (fields included) so
    // this integration test can construct the real types directly, rather
    // than hand-building a `json!` mirror as the private-DTO fixtures above
    // do. `pub(crate)` (as the visibility this arm's brief originally
    // specified) does NOT work here: `tests/macos_fixtures.rs` compiles as a
    // separate crate linking against `rupu-cp` as an external dependency, so
    // `pub(crate)` items stay invisible to it (confirmed by a compile
    // failure — `struct ConfigView is private`) — the same reason every
    // OTHER type imported directly in this file (`FindingOut`, `ProjectRow`,
    // `RunListRow`, `OutlierRun`, …) is `pub`, never `pub(crate)`.
    //
    // `effective` carries a dotted-name provider table key (`GLM-5.2-FP8`) to
    // exercise the canonical quoted-encoding stress case (see
    // `rupu_config::resolve::dotted`'s doc comment); `provenance` covers the
    // full source matrix (Global/Project/Default, one locked) plus that same
    // quoted-middle-segment key exactly as `dotted()` would emit it.
    //
    // `effective.policy.lock` is included because `resolve()` ALWAYS pins it
    // (`config.policy.lock = lock.clone()`, unconditional — see
    // `rupu_config::resolve::resolve`'s doc comment on that line) and
    // `PolicyConfig::lock` has no `skip_serializing_if`, so it is always on
    // the real wire — never omit it from this fixture. It deliberately
    // carries a SECOND entry, `"permission_mode"`, that names a key neither
    // layer's raw TOML actually sets: `resolve()` only inserts a
    // `provenance` entry for a key when some layer supplies a value for it
    // (`if let Some(v) = val { ... provenance.insert(...) }`), so a locked
    // key that's merely NAMED in `[policy].lock` without ever being set
    // anywhere never gets a `provenance` entry at all — the fixture's
    // `provenance` map below deliberately has no `"permission_mode"` key,
    // reproducing that exact real-world case (a lock list entry invisible
    // to anything that reads it off `provenance` instead of
    // `effective.policy.lock`).
    let effective = serde_json::json!({
        "default_model": "claude-sonnet-4-6",
        "providers": {
            "GLM-5.2-FP8": {
                "model": "GLM-5.2-FP8",
                "base_url": "https://oracle.internal/v1",
            },
        },
        "cp": {
            "bind": "127.0.0.1:7420",
            "max_workspace_bytes": 500_000_000i64,
        },
        "policy": {
            "lock": ["policy.lock", "permission_mode"],
        },
    });

    let mut provenance = BTreeMap::new();
    provenance.insert(
        "default_model".to_string(),
        KeyProvenance {
            source: KeySource::Global,
            locked: false,
        },
    );
    provenance.insert(
        "cp.max_workspace_bytes".to_string(),
        KeyProvenance {
            source: KeySource::Project,
            locked: false,
        },
    );
    provenance.insert(
        "log_level".to_string(),
        KeyProvenance {
            source: KeySource::Default,
            locked: false,
        },
    );
    provenance.insert(
        "policy.lock".to_string(),
        KeyProvenance {
            source: KeySource::Global,
            locked: true,
        },
    );
    // The quoted-middle-segment stress case: `providers."GLM-5.2-FP8".model`,
    // exactly as `rupu_config::resolve::dotted()` emits a segment containing
    // a `.` (see its doc comment — matches `config_write::split_dotted_key`
    // and the frontend's `quoteSegment`/`splitDottedKey`).
    provenance.insert(
        "providers.\"GLM-5.2-FP8\".model".to_string(),
        KeyProvenance {
            source: KeySource::Project,
            locked: false,
        },
    );

    let raw_global = "default_model = \"claude-sonnet-4-6\"\nlog_level = \"info\"\n\n\
                       [policy]\nlock = [\"policy.lock\", \"permission_mode\"]\n"
        .to_string();
    let raw_project = Some(
        "default_model = \"claude-opus-4-8\"\n\n\
         [cp]\nmax_workspace_bytes = 500000000\n"
            .to_string(),
    );

    let cp = serde_json::json!({
        "bind": "127.0.0.1:7420",
        "max_workspace_bytes": 500_000_000i64,
    });

    let view = ConfigView {
        effective,
        provenance,
        raw_global,
        raw_project,
        cp,
        status: RuntimeStatus {
            bind: "127.0.0.1:7420".into(),
            token_set: false,
            restart_required_keys: vec!["bind".into(), "token".into()],
        },
    };
    check_fixture("config_view.json", &view);
}

// ── Situation Room: code viewers + claims (Phase 6B) ────────────────────────

/// Shared skeleton for [`autoflow_claims_fixture_is_current`]'s rows — every
/// field defaulted to `None`/empty so each case below only sets what it cares
/// about, mirroring `sample_run_record`'s struct-update pattern above.
fn sample_claim(issue_ref: &str, status: ClaimStatus) -> AutoflowClaimRecord {
    AutoflowClaimRecord {
        issue_ref: issue_ref.into(),
        repo_ref: "github:Section9Labs/rupu".into(),
        source_ref: None,
        issue_display_ref: None,
        issue_title: None,
        issue_url: None,
        issue_state_name: None,
        issue_tracker: None,
        workflow: "issue-supervisor-dispatch".into(),
        status,
        worktree_path: None,
        branch: None,
        last_run_id: None,
        last_error: None,
        last_summary: None,
        pr_url: None,
        artifacts: None,
        artifact_manifest_path: None,
        next_retry_at: None,
        claim_owner: None,
        lease_expires_at: None,
        pending_dispatch: None,
        contenders: vec![],
        updated_at: "2026-08-25T12:00:00Z".into(),
    }
}

#[test]
fn autoflow_claims_fixture_is_current() {
    // `GET /api/autoflows/claims` (`list_claims`, api/autoflow_claims.rs)
    // returns `Vec<ClaimRow>` — `ClaimRow::from(AutoflowClaimRecord)` real
    // types, not hand-built, so drift on either shows up here. `status` is
    // asserted against `ClaimStatus`'s actual snake_case serialization (see
    // `claim_row_status_is_lowercase_snake` in api/autoflow_claims.rs's own
    // unit tests) rather than an invented string. Three rows:
    // 1. `await_human`, `last_error` + `pr_url` set (blocked on a human).
    // 2. `running`, `claim_owner` + `lease_expires_at` set (an active lease).
    // 3. `eligible`, every `ClaimRow` `Option` field `None` (honest `—`
    //    coverage for a freshly-discovered, unclaimed issue).
    let awaiting = AutoflowClaimRecord {
        issue_display_ref: Some("101".into()),
        issue_title: Some("Nightly workflow needs manual review".into()),
        issue_url: Some("https://github.com/Section9Labs/rupu/issues/101".into()),
        issue_state_name: Some("open".into()),
        issue_tracker: Some("github".into()),
        worktree_path: Some("/tmp/rupu-worktrees/101".into()),
        branch: Some("autoflow/issue-101".into()),
        last_run_id: Some("run_9k2f".into()),
        last_error: Some("panel review exceeded max_iterations".into()),
        last_summary: Some("blocked pending human review".into()),
        pr_url: Some("https://github.com/Section9Labs/rupu/pull/220".into()),
        ..sample_claim(
            "github:Section9Labs/rupu/issues/101",
            ClaimStatus::AwaitHuman,
        )
    };

    let active = AutoflowClaimRecord {
        issue_display_ref: Some("102".into()),
        issue_title: Some("Flaky test in rupu-orchestrator".into()),
        issue_url: Some("https://github.com/Section9Labs/rupu/issues/102".into()),
        issue_state_name: Some("open".into()),
        issue_tracker: Some("github".into()),
        worktree_path: Some("/tmp/rupu-worktrees/102".into()),
        branch: Some("autoflow/issue-102".into()),
        last_run_id: Some("run_am4d".into()),
        last_summary: Some("running the dispatched workflow".into()),
        claim_owner: Some("host:kuki:5521".into()),
        lease_expires_at: Some("2026-08-25T13:30:00Z".into()),
        updated_at: "2026-08-25T12:15:00Z".into(),
        ..sample_claim("github:Section9Labs/rupu/issues/102", ClaimStatus::Running)
    };

    let untouched = sample_claim("github:Section9Labs/rupu/issues/103", ClaimStatus::Eligible);

    let rows: Vec<ClaimRow> = vec![
        ClaimRow::from(awaiting),
        ClaimRow::from(active),
        ClaimRow::from(untouched),
    ];
    assert_eq!(rows[0].status, "await_human");
    assert_eq!(rows[1].status, "running");
    assert_eq!(rows[2].status, "eligible");
    check_fixture("autoflow_claims.json", &rows);
}

#[test]
fn code_tree_fixture_is_current() {
    // `GET /api/projects/:ws_id/tree` (`get_tree`, api/code.rs) returns
    // `TreeResult` directly — `TreeEntry`/`TreeResult` are `pub`, mirrored
    // for real. A non-root directory (`parent: Some("")`, matching
    // `list_tree`'s "trim to workspace root" case) with one dir entry sorted
    // before one file entry (dirs-first-then-files ordering `list_tree`
    // guarantees — see `lists_root_dirs_first_then_files_and_hides_git`).
    let tree = TreeResult {
        path: "src".into(),
        parent: Some(String::new()),
        entries: vec![
            TreeEntry {
                name: "auth".into(),
                path: "src/auth".into(),
                kind: "dir".into(),
            },
            TreeEntry {
                name: "main.rs".into(),
                path: "src/main.rs".into(),
                kind: "file".into(),
            },
        ],
    };
    check_fixture("code_tree.json", &tree);
}

#[test]
fn code_file_fixture_is_current() {
    // `GET /api/projects/:ws_id/source` (`get_source`, api/code.rs) returns
    // `FileContent` directly. This covers the `available:true` case (with
    // `lines`/`language`/`total_lines` populated, per `read_whole_file`'s
    // success path); the `available:false, reason:Some(..)` case is covered
    // by `run_source.json`'s unavailable element instead, per the brief.
    let fc = FileContent {
        available: true,
        path: Some("src/main.rs".into()),
        language: Some("rust"),
        total_lines: Some(3),
        lines: Some(vec![
            SourceLine {
                n: 1,
                text: "fn main() {".into(),
            },
            SourceLine {
                n: 2,
                text: "    println!(\"hi\");".into(),
            },
            SourceLine {
                n: 3,
                text: "}".into(),
            },
        ]),
        reason: None,
    };
    check_fixture("code_file.json", &fc);
}

#[test]
fn code_files_fixture_is_current() {
    // `GET /api/projects/:ws_id/files` (`get_files`, api/code.rs) returns
    // `FileListResult` directly — `truncated: true` case (the `MAX_FILES`
    // cap tripped mid-walk, per `list_all_files_capped`'s doc comment).
    let res = FileListResult {
        files: vec![
            "README.md".into(),
            "src/auth/session.rs".into(),
            "src/main.rs".into(),
        ],
        truncated: true,
    };
    check_fixture("code_files.json", &res);
}

#[test]
fn run_source_fixture_is_current() {
    // `GET /api/runs/:id/source` (`get_source`, api/source.rs) returns
    // `SourceSlice` directly. Two-element array covering both cases this
    // endpoint soft-fails to HTTP 200 for, following the same
    // multiple-cases-in-one-fixture pattern `workers_fixture_is_current`
    // (busy vs. idle `WorkerRecord`) uses above:
    // 1. An available slice with `target_line` + numbered `lines`.
    // 2. An unavailable slice for a remote-host run — the exact message text
    //    of the private `REMOTE_NOT_SUPPORTED` const (api/source.rs), copied
    //    verbatim since the const itself isn't `pub` to import.
    let available = SourceSlice {
        available: true,
        path: Some("src/auth/session.rs".into()),
        language: Some("rust"),
        start_line: Some(40),
        end_line: Some(44),
        target_line: Some(42),
        total_lines: Some(120),
        lines: Some(vec![
            SourceLine {
                n: 40,
                text: "    pub fn verify(&self, token: &str) -> bool {".into(),
            },
            SourceLine {
                n: 41,
                text: "        let stored_token = self.stored_token();".into(),
            },
            SourceLine {
                n: 42,
                text: "        if token == stored_token {".into(),
            },
            SourceLine {
                n: 43,
                text: "            return true;".into(),
            },
            SourceLine {
                n: 44,
                text: "        }".into(),
            },
        ]),
        reason: None,
    };
    let unavailable = SourceSlice {
        reason: Some("Source preview is not available for remote-host runs yet.".into()),
        ..SourceSlice::default()
    };
    check_fixture("run_source.json", &vec![available, unavailable]);
}

#[test]
fn run_ast_fixture_is_current() {
    // `GET /api/runs/:id/ast` (`get_ast`, api/source.rs) returns
    // `AstResponse` directly — `root: Some(rupu_ast::AstNode)` is the real
    // type (fields per `rupu_ast::lib.rs`), hand-built as a small 3-level
    // tree (`source_file` → `function_item` → `identifier`) rather than a
    // live tree-sitter parse, so the fixture stays stable across grammar
    // version bumps. Only the deepest (target) node carries `matched: true`,
    // mirroring `parse_slice`'s "matched node id" flagging.
    let identifier = AstNode {
        kind: "identifier".into(),
        named: true,
        field: Some("name".into()),
        start_line: 2,
        start_col: 8,
        end_line: 2,
        end_col: 12,
        matched: true,
        children: vec![],
    };
    let function_item = AstNode {
        kind: "function_item".into(),
        named: true,
        field: None,
        start_line: 1,
        start_col: 1,
        end_line: 3,
        end_col: 2,
        matched: false,
        children: vec![identifier],
    };
    let source_file = AstNode {
        kind: "source_file".into(),
        named: true,
        field: None,
        start_line: 1,
        start_col: 1,
        end_line: 3,
        end_col: 2,
        matched: false,
        children: vec![function_item],
    };
    let response = AstResponse {
        available: true,
        language: Some("rust".into()),
        root: Some(source_file),
        truncated: Some(false),
        reason: None,
    };
    check_fixture("run_ast.json", &response);
}
