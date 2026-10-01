use rupu_transcript::Event;

#[test]
fn usage_event_serde_roundtrip() {
    let e = Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        served_model: None,
        input_tokens: 1234,
        output_tokens: 567,
        cached_tokens: 890,
        cache_write_tokens: 0,
        purpose: None,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"type\":\"usage\""));
    assert!(json.contains("\"input_tokens\":1234"));
    // `served_model` is None → skipped from the wire form.
    assert!(!json.contains("served_model"));
    let back: Event = serde_json::from_str(&json).unwrap();
    match back {
        Event::Usage {
            provider,
            model,
            served_model,
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_write_tokens,
            purpose,
        } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model, "claude-sonnet-4-6");
            assert_eq!(served_model, None);
            assert_eq!(input_tokens, 1234);
            assert_eq!(output_tokens, 567);
            assert_eq!(cached_tokens, 890);
            assert_eq!(cache_write_tokens, 0);
            assert_eq!(purpose, None);
        }
        _ => panic!("expected Event::Usage"),
    }
}

#[test]
fn usage_event_with_served_model_roundtrips() {
    let e = Event::Usage {
        provider: "anthropic".into(),
        model: "claude-opus-4-8".into(),
        served_model: Some("claude-mythos-preview".into()),
        input_tokens: 10,
        output_tokens: 20,
        cached_tokens: 0,
        cache_write_tokens: 0,
        purpose: None,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"served_model\":\"claude-mythos-preview\""));
    let back: Event = serde_json::from_str(&json).unwrap();
    match back {
        Event::Usage {
            model,
            served_model,
            ..
        } => {
            assert_eq!(model, "claude-opus-4-8");
            assert_eq!(served_model.as_deref(), Some("claude-mythos-preview"));
        }
        _ => panic!("expected Event::Usage"),
    }
}

#[test]
fn old_usage_json_without_served_model_deserializes() {
    // An old-style transcript line that predates `served_model`.
    let line = r#"{"type":"usage","data":{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":5,"output_tokens":7,"cached_tokens":1}}"#;
    let ev: Event = serde_json::from_str(line).unwrap();
    match ev {
        Event::Usage {
            served_model,
            input_tokens,
            output_tokens,
            cached_tokens,
            ..
        } => {
            assert_eq!(served_model, None, "missing served_model defaults to None");
            assert_eq!(input_tokens, 5);
            assert_eq!(output_tokens, 7);
            assert_eq!(cached_tokens, 1);
        }
        _ => panic!("expected Event::Usage"),
    }
}

#[test]
fn usage_purpose_round_trips_and_defaults_to_none() {
    let with = rupu_transcript::Event::Usage {
        provider: "anthropic".into(),
        model: "m".into(),
        served_model: None,
        input_tokens: 10,
        output_tokens: 2,
        cached_tokens: 0,
        cache_write_tokens: 0,
        purpose: Some("compaction".into()),
    };
    let json = serde_json::to_string(&with).unwrap();
    assert!(json.contains("\"purpose\":\"compaction\""));
    let back: rupu_transcript::Event = serde_json::from_str(&json).unwrap();
    assert_eq!(back, with);

    // Older lines (no purpose) still parse, as None, and don't serialize it.
    let old = r#"{"type":"usage","data":{"provider":"p","model":"m","input_tokens":1,"output_tokens":1,"cached_tokens":0}}"#;
    let ev: rupu_transcript::Event = serde_json::from_str(old).unwrap();
    match &ev {
        rupu_transcript::Event::Usage { purpose, .. } => assert!(purpose.is_none()),
        _ => panic!("expected usage"),
    }
    assert!(!serde_json::to_string(&ev).unwrap().contains("purpose"));
}

#[test]
fn usage_cache_write_tokens_round_trips() {
    let e = Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        served_model: None,
        input_tokens: 1000,
        output_tokens: 40,
        cached_tokens: 600,
        cache_write_tokens: 30,
        purpose: None,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"cache_write_tokens\":30"), "{json}");
    let back: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(back, e);
    match back {
        Event::Usage {
            cache_write_tokens, ..
        } => assert_eq!(cache_write_tokens, 30),
        _ => panic!("expected Event::Usage"),
    }
}

#[test]
fn old_usage_line_without_cache_write_tokens_defaults_to_zero_and_omits_it() {
    // A transcript line written before `cache_write_tokens` existed.
    let old = r#"{"type":"usage","data":{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":5,"output_tokens":7,"cached_tokens":1}}"#;
    let ev: Event = serde_json::from_str(old).unwrap();
    match &ev {
        Event::Usage {
            cache_write_tokens,
            cached_tokens,
            ..
        } => {
            assert_eq!(*cache_write_tokens, 0, "missing field defaults to 0");
            assert_eq!(*cached_tokens, 1);
        }
        _ => panic!("expected Event::Usage"),
    }
    // Zero is never written, so non-caching providers' lines are unchanged.
    let json = serde_json::to_string(&ev).unwrap();
    assert!(!json.contains("cache_write_tokens"), "{json}");
}
