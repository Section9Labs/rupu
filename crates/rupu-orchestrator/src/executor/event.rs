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
        /// Host that ran this step. `None` = local (same host as the
        /// orchestrator). `Some(name)` = a remote fleet host (multi-host
        /// `host:` placement). Absent in older event logs; serde default
        /// restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    StepWorking {
        run_id: String,
        step_id: String,
        note: Option<String>,
        /// Transcript file for this running step, emitted once its sub-run
        /// path is known (a linear step generates it lazily, after
        /// `StepStarted`). Lets the live UI select and tail the file before
        /// any persisted `step_result` exists. `None` on tool-call pings;
        /// absent in older event logs (serde default restores `None`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript_path: Option<PathBuf>,
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
        /// Host that ran this step. `None` = local. `Some(name)` = remote.
        /// Absent in older event logs; serde default restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
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
        /// Host that ran this unit. `None` = local (same host as the
        /// orchestrator). `Some(name)` = a remote fleet host (multi-host
        /// `distribute:` placement). Absent in older event logs; serde
        /// default restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
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
        /// Host that ran this unit. `None` = local. `Some(name)` = remote.
        /// Absent in older event logs; serde default restores `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
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
    /// The run was paused by an operator (distinct from `RunCompleted`
    /// with a `Cancelled` status — a paused run expects a later
    /// `RunResumed`).
    RunPaused { run_id: String },
    /// A previously paused run resumed execution.
    RunResumed { run_id: String },
    /// The step in flight when a pause was requested stopped
    /// cooperatively at a checkpoint boundary.
    StepPaused { run_id: String, step_id: String },
    /// A step resumed after a prior `StepPaused`.
    StepResumed { run_id: String, step_id: String },
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
            | Event::RunFailed { run_id, .. }
            | Event::RunPaused { run_id, .. }
            | Event::RunResumed { run_id, .. }
            | Event::StepPaused { run_id, .. }
            | Event::StepResumed { run_id, .. } => run_id,
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
            host: None,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""type":"step_completed""#));
        assert!(json.contains(r#""step_id":"classify_input""#));
    }

    #[test]
    fn step_started_host_round_trips() {
        let ev = Event::StepStarted {
            run_id: "r1".into(),
            step_id: "build".into(),
            kind: StepKind::Linear,
            agent: Some("builder".into()),
            host: Some("worker-1".into()),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains("\"host\":\"worker-1\""), "json: {json}");
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back,
            Event::StepStarted { host: Some(ref h), .. } if h == "worker-1"
        ));
    }

    #[test]
    fn step_working_transcript_path_round_trips() {
        let ev = Event::StepWorking {
            run_id: "r1".into(),
            step_id: "build".into(),
            note: None,
            transcript_path: Some(PathBuf::from("/t/run_X.jsonl")),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(
            json.contains(r#""transcript_path":"/t/run_X.jsonl""#),
            "json: {json}"
        );
        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            back,
            Event::StepWorking { transcript_path: Some(ref p), .. } if p == &PathBuf::from("/t/run_X.jsonl")
        ));
    }

    #[test]
    fn step_working_transcript_path_defaults_to_none_when_absent() {
        // Older event logs / tool-call pings without the field still deserialize.
        let json = r#"{"type":"step_working","run_id":"r1","step_id":"build","note":null}"#;
        let back: Event = serde_json::from_str(json).expect("deserialize legacy");
        assert!(matches!(
            back,
            Event::StepWorking {
                transcript_path: None,
                ..
            }
        ));
    }

    #[test]
    fn step_completed_host_defaults_to_none_when_absent() {
        // Older event logs without `host` must still deserialize.
        let json = r#"{"type":"step_completed","run_id":"r1","step_id":"build","success":true,"duration_ms":5}"#;
        let back: Event = serde_json::from_str(json).expect("deserialize legacy");
        assert!(matches!(back, Event::StepCompleted { host: None, .. }));
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
    fn run_paused_resumed_round_trip() {
        let ev = Event::RunPaused {
            run_id: "r1".into(),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("run_paused") || j.contains("RunPaused"));
        let back: Event = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, Event::RunPaused { .. }));

        let ev = Event::RunResumed {
            run_id: "r1".into(),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("run_resumed") || j.contains("RunResumed"));
        let back: Event = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, Event::RunResumed { .. }));
    }

    #[test]
    fn step_paused_resumed_round_trip() {
        let ev = Event::StepPaused {
            run_id: "r1".into(),
            step_id: "s1".into(),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("step_paused") || j.contains("StepPaused"));
        let back: Event = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, Event::StepPaused { .. }));

        let ev = Event::StepResumed {
            run_id: "r1".into(),
            step_id: "s1".into(),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("step_resumed") || j.contains("StepResumed"));
        let back: Event = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, Event::StepResumed { .. }));
    }

    #[test]
    fn unknown_event_type_errors() {
        let bad = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let res: Result<Event, _> = serde_json::from_str(bad);
        assert!(res.is_err(), "unknown variant should fail to deserialize");
    }
}
