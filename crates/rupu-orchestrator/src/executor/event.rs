//! Step-level workflow event. Serialized as one JSON object per line
//! into `events.jsonl`. Same enum round-trips through the in-process
//! broadcast channel and the on-disk log — `Deserialize` + `Serialize`
//! both required.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runs::{RunStatus, StepKind};

/// How an interrupted attempt was picked back up (see
/// [`Event::AttemptResumed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptResumeMode {
    /// The interrupted agent run's transcript was replayed and the new run
    /// carries on from where it stopped.
    Continued,
    /// The interrupted attempt's transcript turned out to be a finished run:
    /// its output was recovered from that transcript without re-running the
    /// agent.
    Recovered,
    /// The attempt was started again from scratch (nothing usable to
    /// continue from).
    Restarted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codename: Option<String>,
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
    /// An agent instance is about to run: the first moment its provider and
    /// model are known. One per agent instance (linear step, fan-out unit,
    /// parallel sub-step, panelist, fixer, on_reject cleanup). Placed units
    /// run remotely, so `provider`/`model` are `None` for them.
    AgentStarted {
        run_id: String,
        step_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codename: Option<String>,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        agent_run_id: String,
        transcript_path: PathBuf,
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
        /// The typed cause when the step failed on a classified response
        /// outcome (a refusal, a provider error the recovery ladder could
        /// not route around, ...). `None` for every other failure and in
        /// older event logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<rupu_transcript::OutcomeRecord>,
    },
    StepSkipped {
        run_id: String,
        step_id: String,
        reason: String,
    },
    /// Something about a step the operator should see that did not fail it —
    /// e.g. a remote unit whose coverage could not be collected.
    StepWarning {
        run_id: String,
        step_id: String,
        /// The fan-out unit, when the warning is about one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        message: String,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codename: Option<String>,
    },
    /// One fan-out unit finished. `tokens_in` / `tokens_out` are the unit's
    /// real totals — from its usage hook for local units, from its mirrored
    /// transcript for remote units (`0` only for in-memory runs with no
    /// store, and for a remote unit whose host exposes no coordinator-side
    /// mirror path — `UnitDispatcher::unit_transcript_path` = `None`).
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
        /// The typed cause of a failed unit, when it failed on a classified
        /// response outcome. `None` on success, for an unclassified failure,
        /// for a remote unit (its error crosses the host boundary as text),
        /// and in older event logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<rupu_transcript::OutcomeRecord>,
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
    /// A `dispatch_agent` tool call spawned a child agent run. Emitted
    /// immediately after the child's sub-run directory is allocated (so
    /// `sub_run_id` + `transcript_path` are already known) and before
    /// the child's agent loop starts. Correlates to a later
    /// `DispatchCompleted` by `sub_run_id` — dispatch carries no
    /// `step_id`; the live view attaches the child to whichever step is
    /// currently active (dispatch happens inside a running step's tool
    /// loop).
    DispatchStarted {
        run_id: String,
        sub_run_id: String,
        agent: Option<String>,
        transcript_path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A dispatched child agent run finished. `tokens_in` / `tokens_out`
    /// are the child run's totals (best-effort `0` if the child errored
    /// before any usage was recorded).
    DispatchCompleted {
        run_id: String,
        sub_run_id: String,
        success: bool,
        tokens_in: u64,
        tokens_out: u64,
        /// The typed cause of a failed child run, when it failed on a
        /// classified response outcome. Absent in older event logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<rupu_transcript::OutcomeRecord>,
    },
    /// A resumed run picked an interrupted attempt back up. Emitted once per
    /// interrupted attempt (a linear step, or one fan-out unit when
    /// `unit_index` is `Some`) before its agent is re-dispatched, so the live
    /// view can show that the attempt is a resumption rather than a fresh
    /// start (a `Recovered` attempt is folded in without a new dispatch).
    /// `from_agent_run_id` is the interrupted agent run being
    /// continued/recovered (`None` for a plain restart); `reason` is a short
    /// operator-facing note on why this mode was chosen.
    AttemptResumed {
        run_id: String,
        step_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_index: Option<usize>,
        mode: AttemptResumeMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_agent_run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// An event type this build does not know — written by a newer rupu.
    /// Readers skip it so an older reader can still replay a newer run's
    /// `events.jsonl`. Never emitted.
    #[serde(other)]
    Unknown,
}

impl Event {
    pub fn run_id(&self) -> &str {
        match self {
            Event::RunStarted { run_id, .. }
            | Event::StepStarted { run_id, .. }
            | Event::StepWorking { run_id, .. }
            | Event::AgentStarted { run_id, .. }
            | Event::StepAwaitingApproval { run_id, .. }
            | Event::StepCompleted { run_id, .. }
            | Event::StepFailed { run_id, .. }
            | Event::StepSkipped { run_id, .. }
            | Event::StepWarning { run_id, .. }
            | Event::UnitStarted { run_id, .. }
            | Event::UnitCompleted { run_id, .. }
            | Event::PanelRound { run_id, .. }
            | Event::RunCompleted { run_id, .. }
            | Event::RunFailed { run_id, .. }
            | Event::RunPaused { run_id, .. }
            | Event::RunResumed { run_id, .. }
            | Event::StepPaused { run_id, .. }
            | Event::StepResumed { run_id, .. }
            | Event::DispatchStarted { run_id, .. }
            | Event::DispatchCompleted { run_id, .. }
            | Event::AttemptResumed { run_id, .. } => run_id,
            Event::Unknown => "",
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
            codename: None,
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
    fn dispatch_started_round_trips() {
        let ev = Event::DispatchStarted {
            run_id: "run_parent".into(),
            sub_run_id: "sub_child".into(),
            agent: Some("security-reviewer".into()),
            transcript_path: PathBuf::from("/runs/run_parent/sub_child.jsonl"),
            codename: None,
            model: None,
            provider: None,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(
            json.contains(r#""type":"dispatch_started""#),
            "json: {json}"
        );
        assert!(json.contains(r#""sub_run_id":"sub_child""#), "json: {json}");

        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.run_id(), "run_parent");
        match back {
            Event::DispatchStarted {
                run_id,
                sub_run_id,
                agent,
                transcript_path,
                ..
            } => {
                assert_eq!(run_id, "run_parent");
                assert_eq!(sub_run_id, "sub_child");
                assert_eq!(agent.as_deref(), Some("security-reviewer"));
                assert_eq!(
                    transcript_path,
                    PathBuf::from("/runs/run_parent/sub_child.jsonl")
                );
            }
            other => panic!("expected DispatchStarted, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_completed_round_trips() {
        let ev = Event::DispatchCompleted {
            run_id: "run_parent".into(),
            sub_run_id: "sub_child".into(),
            success: true,
            tokens_in: 12,
            tokens_out: 34,
            cause: None,
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(
            json.contains(r#""type":"dispatch_completed""#),
            "json: {json}"
        );
        assert!(json.contains(r#""sub_run_id":"sub_child""#), "json: {json}");

        let back: Event = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.run_id(), "run_parent");
        match back {
            Event::DispatchCompleted {
                run_id,
                sub_run_id,
                success,
                tokens_in,
                tokens_out,
                ..
            } => {
                assert_eq!(run_id, "run_parent");
                assert_eq!(sub_run_id, "sub_child");
                assert!(success);
                assert_eq!(tokens_in, 12);
                assert_eq!(tokens_out, 34);
            }
            other => panic!("expected DispatchCompleted, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_type_is_skipped_as_unknown() {
        // A newer writer's event type must not break an older reader's
        // `events.jsonl` replay: it parses as `Unknown` and is skipped.
        let newer = r#"{"type":"step_warped","run_id":"r","step_id":"s"}"#;
        let ev: Event = serde_json::from_str(newer).expect("unknown type parses");
        assert!(matches!(ev, Event::Unknown), "got {ev:?}");
        assert_eq!(ev.run_id(), "");
    }

    fn refusal_cause() -> rupu_transcript::OutcomeRecord {
        rupu_transcript::OutcomeRecord {
            id: "o1".into(),
            class: "refusal".into(),
            severity: rupu_transcript::Severity::Error,
            title: "refused".into(),
            detail: None,
            error_class: None,
            wire: serde_json::Value::Null,
        }
    }

    #[test]
    fn step_failed_cause_round_trips_and_legacy_lines_parse() {
        let ev = Event::StepFailed {
            run_id: "run_A".into(),
            step_id: "review".into(),
            error: "agent failure in step review: refused".into(),
            cause: Some(refusal_cause()),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        assert!(json.contains(r#""class":"refusal""#), "json: {json}");
        match serde_json::from_str::<Event>(&json).expect("deserialize") {
            Event::StepFailed { cause, .. } => assert_eq!(cause, Some(refusal_cause())),
            other => panic!("expected StepFailed, got {other:?}"),
        }

        let legacy = r#"{"type":"step_failed","run_id":"r","step_id":"s","error":"boom"}"#;
        match serde_json::from_str::<Event>(legacy).expect("legacy parses") {
            Event::StepFailed { cause, error, .. } => {
                assert_eq!(cause, None);
                assert_eq!(error, "boom");
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }
        let none = Event::StepFailed {
            run_id: "r".into(),
            step_id: "s".into(),
            error: "boom".into(),
            cause: None,
        };
        assert!(!serde_json::to_string(&none).unwrap().contains("cause"));
    }

    #[test]
    fn unit_and_dispatch_completed_carry_an_optional_cause() {
        let unit = Event::UnitCompleted {
            run_id: "r".into(),
            step_id: "s".into(),
            index: 1,
            unit_key: "k".into(),
            success: false,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
            cause: Some(refusal_cause()),
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&unit).unwrap()).unwrap();
        assert!(
            matches!(back, Event::UnitCompleted { cause: Some(c), .. } if c.class == "refusal")
        );
        let dispatch = Event::DispatchCompleted {
            run_id: "r".into(),
            sub_run_id: "sub".into(),
            success: false,
            tokens_in: 0,
            tokens_out: 0,
            cause: Some(refusal_cause()),
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&dispatch).unwrap()).unwrap();
        assert!(
            matches!(back, Event::DispatchCompleted { cause: Some(c), .. } if c.class == "refusal")
        );
    }

    #[test]
    fn agent_started_serde_and_run_id() {
        let ev = Event::AgentStarted {
            run_id: "run_W".into(),
            step_id: "review".into(),
            unit_index: Some(3),
            codename: Some("jade-reef/heron#4".into()),
            agent: "security-reviewer".into(),
            provider: Some("anthropic".into()),
            model: Some("claude-opus-5-5".into()),
            agent_run_id: "run_U".into(),
            transcript_path: "/t.jsonl".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "agent_started");
        assert_eq!(v["codename"], "jade-reef/heron#4");
        assert_eq!(ev.run_id(), "run_W");
        let legacy = r#"{"type":"unit_started","run_id":"r","step_id":"s","index":0,"unit_key":"k","agent":null,"transcript_path":"/t"}"#;
        assert!(serde_json::from_str::<Event>(legacy).is_ok());
    }

    #[test]
    fn attempt_resumed_round_trips() {
        let e = Event::AttemptResumed {
            run_id: "r".into(),
            step_id: "s".into(),
            unit_index: Some(3),
            mode: AttemptResumeMode::Continued,
            from_agent_run_id: Some("run_prev".into()),
            reason: None,
        };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["type"], "attempt_resumed");
        assert_eq!(j["mode"], "continued");
        assert!(j.get("reason").is_none(), "None reason is skipped");
        assert_eq!(e.run_id(), "r");
        assert_eq!(serde_json::from_value::<Event>(j).unwrap(), e);
    }

    #[test]
    fn attempt_resumed_modes_serialize_snake_case_and_optionals_default() {
        for (mode, wire) in [
            (AttemptResumeMode::Continued, "continued"),
            (AttemptResumeMode::Recovered, "recovered"),
            (AttemptResumeMode::Restarted, "restarted"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), wire);
        }
        // A minimal line (no unit_index / from_agent_run_id / reason)
        // still deserializes: the optionals are `serde(default)`.
        let minimal = r#"{"type":"attempt_resumed","run_id":"r","step_id":"s","mode":"restarted"}"#;
        let ev: Event = serde_json::from_str(minimal).unwrap();
        assert_eq!(
            ev,
            Event::AttemptResumed {
                run_id: "r".into(),
                step_id: "s".into(),
                unit_index: None,
                mode: AttemptResumeMode::Restarted,
                from_agent_run_id: None,
                reason: None,
            }
        );
    }
}
