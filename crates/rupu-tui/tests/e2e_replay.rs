use std::time::Duration;

use chrono::Utc;
use rupu_transcript::{Event, RunMode};
use rupu_tui::source::{EventSource, ReplaySource, SourceEvent};

#[test]
fn replay_drains_scripted_events_in_order() {
    let scripted = vec![
        (
            "step-1".to_string(),
            Event::RunStart {
                run_id: "run_test".into(),
                workspace_id: "ws".into(),
                agent: "a".into(),
                provider: "anthropic".into(),
                model: "m".into(),
                started_at: Utc::now(),
                mode: RunMode::Ask,
            },
        ),
        ("step-1".to_string(), Event::TurnStart { turn_idx: 0 }),
    ];

    let mut src = ReplaySource::new(scripted, 1_000);
    let drained = src.poll();
    assert!(
        drained.is_empty(),
        "first poll yields no events without time progression"
    );

    let first = src.wait(Duration::from_millis(1));
    assert!(matches!(first, Some(SourceEvent::StepEvent { .. })));
}
