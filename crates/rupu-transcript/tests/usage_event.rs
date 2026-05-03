use rupu_transcript::Event;

#[test]
fn usage_event_serde_roundtrip() {
    let e = Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        input_tokens: 1234,
        output_tokens: 567,
        cached_tokens: 890,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"type\":\"usage\""));
    assert!(json.contains("\"input_tokens\":1234"));
    let back: Event = serde_json::from_str(&json).unwrap();
    match back {
        Event::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model, "claude-sonnet-4-6");
            assert_eq!(input_tokens, 1234);
            assert_eq!(output_tokens, 567);
            assert_eq!(cached_tokens, 890);
        }
        _ => panic!("expected Event::Usage"),
    }
}
