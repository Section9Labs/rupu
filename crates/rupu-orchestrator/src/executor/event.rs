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
    fn unknown_event_type_errors() {
        let bad = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let res: Result<Event, _> = serde_json::from_str(bad);
        assert!(res.is_err(), "unknown variant should fail to deserialize");
    }
}
