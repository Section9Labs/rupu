#![deny(clippy::all)]
//! Golden fixtures for the macOS app (apps/rupu-macos/Fixtures/).
//! `cargo test -p rupu-cp --test macos_fixtures` asserts no drift;
//! `REGEN_FIXTURES=1` rewrites. Swift decodes these in RupuAPITests.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStatus, StepKind};

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
