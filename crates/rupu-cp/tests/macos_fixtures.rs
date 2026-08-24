#![deny(clippy::all)]
//! Golden fixtures for the macOS app (apps/rupu-macos/Fixtures/).
//! `cargo test -p rupu-cp --test macos_fixtures` asserts no drift;
//! `REGEN_FIXTURES=1` rewrites. Swift decodes these in RupuAPITests.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use rupu_cp::api::graph::{ApprovalGateDto, GateDto, StepDag, StepNodeDto, SubStepDto};
use rupu_cp::api::projects::ProjectRow;
use rupu_cp::api::runs::RunListRow;
use rupu_cp::host::dashboard_summary::{
    ActiveCounts, ActiveLongest, CycleCounts, DashboardSummary, FleetCounts, TerminalBucket,
    ThroughputBucket,
};
use rupu_cp::usage::UsageSummary;
use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, Origin, Outcome};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{AwaitingGate, RunStatus, StepKind};
use rupu_orchestrator::{FindingRecord, RunRecord, StepResultRecord, UnitCheckpoint};

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
        },
        rupu_transcript::Event::TurnEnd {
            turn_idx: 1,
            tokens_in: None,
            tokens_out: None,
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
