//! Step-level workflow event. Serialized as one JSON object per line
//! into `events.jsonl`. Same enum round-trips through the in-process
//! broadcast channel and the on-disk log — `Deserialize` + `Serialize`
//! both required.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runs::{RunStatus, StepKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        event_version: u32,
        run_id: String,
        workflow_path: PathBuf,
        started_at: DateTime<Utc>,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        kind: StepKind,
        agent: Option<String>,
    },
    StepWorking {
        run_id: String,
        step_id: String,
        note: Option<String>,
    },
    StepAwaitingApproval {
        run_id: String,
        step_id: String,
        reason: String,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
        success: bool,
        duration_ms: u64,
    },
    StepFailed {
        run_id: String,
        step_id: String,
        error: String,
    },
    StepSkipped {
        run_id: String,
        step_id: String,
        reason: String,
    },
    /// One fan-out (`for_each` / `parallel`) unit began its agent run.
    /// Emitted immediately before the unit is dispatched so the live
    /// view can mark that unit working and re-point the focus feed at
    /// the unit's transcript.
    UnitStarted {
        run_id: String,
        step_id: String,
        index: usize,
        /// The `for_each` item rendered to a short string (e.g. the path).
        unit_key: String,
        agent: Option<String>,
        transcript_path: PathBuf,
    },
    /// One fan-out unit finished. `tokens_in` / `tokens_out` are
    /// best-effort: the runner's per-unit dispatch result does not carry
    /// token counts, so they are emitted as `0` (tokens still flow to the
    /// live view via the per-unit transcript tail).
    UnitCompleted {
        run_id: String,
        step_id: String,
        index: usize,
        unit_key: String,
        success: bool,
        tokens_in: u64,
        tokens_out: u64,
    },
    /// Emitted at the start of each gate-loop iteration in a panel step.
    /// Allows the live view to display a live round counter (e.g. "Round 2 / 5").
    PanelRound {
        run_id: String,
        step_id: String,
        /// 1-based iteration counter.
        round: u32,
        max_iterations: u32,
        /// Highest finding severity remaining at the top of this round,
        /// if already known (always `None` on round 1 before any results).
        max_severity_remaining: Option<String>,
    },
    RunCompleted {
        run_id: String,
        status: RunStatus,
        finished_at: DateTime<Utc>,
    },
    RunFailed {
        run_id: String,
        error: String,
        finished_at: DateTime<Utc>,
    },
}

impl Event {
    pub fn run_id(&self) -> &str {
        match self {
            Event::RunStarted { run_id, .. }
            | Event::StepStarted { run_id, .. }
            | Event::StepWorking { run_id, .. }
            | Event::StepAwaitingApproval { run_id, .. }
            | Event::StepCompleted { run_id, .. }
            | Event::StepFailed { run_id, .. }
            | Event::StepSkipped { run_id, .. }
            | Event::UnitStarted { run_id, .. }
            | Event::UnitCompleted { run_id, .. }
            | Event::PanelRound { run_id, .. }
            | Event::RunCompleted { run_id, .. }
            | Event::RunFailed { run_id, .. } => run_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    #[test]
    fn run_started_round_trips_through_json() {
        let ev = Event::RunStarted {
            event_version: 1,
            run_id: "run_01J0".into(),
            workflow_path: PathBuf::from("/wf/foo.yaml"),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 5, 12, 0, 0, 0).unwrap(),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        match back {
            Event::RunStarted {
                event_version,
                run_id,
                ..
            } => {
                assert_eq!(event_version, 1);
                assert_eq!(run_id, "run_01J0");
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[test]
    fn step_completed_serializes_as_tagged_json() {
        let ev = Event::StepCompleted {
            run_id: "run_x".into(),
            step_id: "classify_input".into(),
            success: true,
            duration_ms: 312,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""type":"step_completed""#));
        assert!(json.contains(r#""step_id":"classify_input""#));
    }

    #[test]
    fn panel_round_round_trips_through_json() {
        let ev = Event::PanelRound {
            run_id: "run-abc".into(),
            step_id: "security-panel".into(),
            round: 2,
            max_iterations: 5,
            max_severity_remaining: Some("high".into()),
        };
        let val = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(val["type"], "panel_round");
        assert_eq!(val["run_id"], "run-abc");
        assert_eq!(val["step_id"], "security-panel");
        assert_eq!(val["round"], 2);
        assert_eq!(val["max_iterations"], 5);
        assert_eq!(val["max_severity_remaining"], "high");

        let back: Event = serde_json::from_value(val).expect("deserialize");
        match back {
            Event::PanelRound {
                run_id,
                step_id,
                round,
                max_iterations,
                max_severity_remaining,
            } => {
                assert_eq!(run_id, "run-abc");
                assert_eq!(step_id, "security-panel");
                assert_eq!(round, 2);
                assert_eq!(max_iterations, 5);
                assert_eq!(max_severity_remaining.as_deref(), Some("high"));
            }
            other => panic!("expected PanelRound, got {other:?}"),
        }
    }

    #[test]
    fn panel_round_none_severity_round_trips() {
        let ev = Event::PanelRound {
            run_id: "r".into(),
            step_id: "p".into(),
            round: 1,
            max_iterations: 3,
            max_severity_remaining: None,
        };
        let val = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(val["type"], "panel_round");
        assert!(val["max_severity_remaining"].is_null());
        let back: Event = serde_json::from_value(val).expect("deserialize");
        assert_eq!(back.run_id(), "r");
    }

    #[test]
    fn unknown_event_type_errors() {
        let bad = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let res: Result<Event, _> = serde_json::from_str(bad);
        assert!(res.is_err(), "unknown variant should fail to deserialize");
    }
}
