use rupu_transcript::outcome::{outcome_line, recovery_line, unknown_line};
use rupu_transcript::{Event, OutcomeRecord, RecoveryAction, Severity, StopRecord};
use serde_json::json;

fn record() -> OutcomeRecord {
    OutcomeRecord {
        id: "oc_01".into(),
        class: "refusal".into(),
        severity: Severity::Error,
        title: "refused · cyber".into(),
        detail: Some("Declined for this example.".into()),
        error_class: None,
        wire: json!({"provider": "anthropic", "value": "refusal"}),
    }
}

fn roundtrip(e: &Event) {
    let line = serde_json::to_string(e).unwrap();
    let back: Event = serde_json::from_str(&line).unwrap();
    assert_eq!(&back, e, "{line}");
}

#[test]
fn outcome_and_recovery_round_trip() {
    roundtrip(&Event::Outcome {
        turn_idx: 3,
        outcome: record(),
    });
    roundtrip(&Event::Recovery {
        outcome_id: "oc_01".into(),
        rung: 1,
        action: RecoveryAction::FellBack,
        attempt: Some(1),
        budget: None,
        provider: Some("anthropic".into()),
        model: Some("claude-opus-4-8".into()),
        reason: None,
        merge_into_previous: false,
        continues_output: false,
    });
}

#[test]
fn turn_end_stop_and_discarded_round_trip_and_default() {
    let stop = StopRecord {
        reason: "refusal".into(),
        wire: json!({"provider": "anthropic", "value": "refusal"}),
        refusal: Some(
            json!({"category": "cyber", "explanation": null, "recommended_model": null, "source": "classifier"}),
        ),
        served_by: None,
    };
    roundtrip(&Event::TurnEnd {
        turn_idx: 1,
        tokens_in: Some(10),
        tokens_out: Some(2),
        stop_reason: Some("refusal".into()),
        response_id: None,
        stop: Some(stop),
        discarded: true,
    });
    // Old lines (no stop/discarded) still parse with the defaults.
    let old: Event = serde_json::from_str(r#"{"type":"turn_end","data":{"turn_idx":0}}"#).unwrap();
    assert!(matches!(
        old,
        Event::TurnEnd {
            stop: None,
            discarded: false,
            ..
        }
    ));
}

#[test]
fn stop_record_reads_a_serialized_provider_stop() {
    // The exact JSON shape rupu_providers::Stop serializes to.
    let v = json!({"reason": "max_tokens", "wire": {"provider": "openai-codex", "value": "max_output_tokens"}});
    let s: StopRecord = serde_json::from_value(v).unwrap();
    assert_eq!(s.reason, "max_tokens");
    assert_eq!(s.wire["value"], "max_output_tokens");
}

#[test]
fn run_complete_outcome_round_trips_and_defaults() {
    roundtrip(&Event::RunComplete {
        run_id: "r".into(),
        status: rupu_transcript::RunStatus::Error,
        total_tokens: 5,
        duration_ms: 1,
        error: Some("refused · cyber".into()),
        outcome: Some(record()),
    });
    let old: Event = serde_json::from_str(
        r#"{"type":"run_complete","data":{"run_id":"r","status":"ok","total_tokens":0,"duration_ms":0}}"#,
    )
    .unwrap();
    assert!(matches!(old, Event::RunComplete { outcome: None, .. }));
}

#[test]
fn unknown_event_keeps_and_re_serializes_its_payload() {
    let line = r#"{"type":"brand_new_event","data":{"x":1,"nested":{"y":[1,2]}}}"#;
    let e: Event = serde_json::from_str(line).unwrap();
    assert_eq!(
        e,
        Event::Unknown {
            tag: "brand_new_event".into(),
            data: json!({"x":1,"nested":{"y":[1,2]}})
        }
    );
    let back = serde_json::to_value(&e).unwrap();
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(line).unwrap()
    );
    // No data key → Null data, still re-serialized without inventing one.
    let e: Event = serde_json::from_str(r#"{"type":"bare_new_event"}"#).unwrap();
    assert_eq!(
        e,
        Event::Unknown {
            tag: "bare_new_event".into(),
            data: serde_json::Value::Null
        }
    );
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"type": "bare_new_event"})
    );
}

#[test]
fn unknown_recovery_action_is_other() {
    let a: RecoveryAction = serde_json::from_value(json!("teleported")).unwrap();
    assert_eq!(a, RecoveryAction::Other);
}

fn outcome_with(severity: Severity, title: &str, detail: Option<&str>) -> OutcomeRecord {
    OutcomeRecord {
        severity,
        title: title.into(),
        detail: detail.map(str::to_string),
        ..record()
    }
}

#[test]
fn outcome_line_glyph_follows_severity() {
    assert_eq!(
        outcome_line(&outcome_with(
            Severity::Error,
            "refused · cyber",
            Some("Declined for this example.")
        )),
        "✗ refused · cyber — Declined for this example."
    );
    assert_eq!(
        outcome_line(&outcome_with(Severity::Warning, "cut off", None)),
        "! cut off"
    );
    assert_eq!(
        outcome_line(&outcome_with(Severity::Info, "paused", Some("resuming"))),
        "· paused — resuming"
    );
}

#[test]
fn recovery_line_phrases_every_action() {
    use RecoveryAction::*;
    let line = |action, provider, model, attempt, budget, reason| {
        recovery_line(action, 1, provider, model, attempt, budget, reason)
    };
    assert_eq!(
        line(Continued, None, None, Some(2), Some(3), None),
        "↺ rung 1 · continued 2/3"
    );
    assert_eq!(
        line(Retried, None, None, None, None, None),
        "↺ rung 1 · retried"
    );
    assert_eq!(
        line(Compacted, None, None, None, None, None),
        "↺ rung 1 · compacted"
    );
    assert_eq!(
        line(
            FellBack,
            Some("anthropic"),
            Some("claude-opus-4-8"),
            None,
            None,
            None
        ),
        "↺ rung 1 · fell back to anthropic/claude-opus-4-8"
    );
    assert_eq!(
        line(
            ServedByFallback,
            None,
            Some("claude-opus-5"),
            None,
            None,
            None
        ),
        "↺ rung 1 · served by claude-opus-5"
    );
    assert_eq!(
        line(
            Skipped,
            Some("openai"),
            Some("m1"),
            None,
            None,
            Some("no credentials")
        ),
        "↺ rung 1 · skipped openai/m1: no credentials"
    );
    assert_eq!(
        line(Failed, None, None, None, None, None),
        "↺ rung 1 · no recovery left"
    );
    assert_eq!(
        line(Asked, None, None, None, None, None),
        "↺ rung 1 · asked"
    );
    assert_eq!(
        line(Parked, None, None, None, None, None),
        "↺ rung 1 · parked"
    );
    assert_eq!(
        line(Other, None, None, None, None, None),
        "↺ rung 1 · other"
    );
    // Missing optional pieces degrade without panicking.
    assert_eq!(
        line(Continued, None, None, None, None, None),
        "↺ rung 1 · continued"
    );
}

#[test]
fn unknown_line_names_the_tag_and_truncates_data() {
    assert_eq!(
        unknown_line("brand_new_event", &serde_json::Value::Null),
        "unrecognized event · brand_new_event"
    );
    assert_eq!(
        unknown_line("brand_new_event", &json!({"x": 1})),
        "unrecognized event · brand_new_event {\"x\":1}"
    );
    let big = json!({"x": "y".repeat(500)});
    let s = unknown_line("t", &big);
    let data = s.strip_prefix("unrecognized event · t ").unwrap();
    assert_eq!(data.chars().count(), 200);
    assert!(data.ends_with('…'));
}
