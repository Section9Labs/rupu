use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_providers::provider::LlmProvider;
use rupu_providers::reply_error::{parse_error_body, ErrorClass, ErrorOrigin};
use rupu_providers::types::{
    ContentBlock, LlmRequest, RefusalDetail, RefusalSource, Stop, StopReason, Usage,
};
use rupu_providers::{ProviderError, ProviderId};

#[tokio::test]
async fn scripted_reply_carries_a_full_stop() {
    let mut stop = Stop::from_wire(StopReason::Refusal, "anthropic", Some("refusal"));
    stop.refusal = Some(RefusalDetail {
        category: Some("cyber".into()),
        explanation: Some("Declined for this example.".into()),
        recommended_model: None,
        source: RefusalSource::Classifier,
    });
    let mut p = MockProvider::new(vec![ScriptedTurn::Reply {
        content: vec![ContentBlock::Text {
            text: "partial".into(),
        }],
        stop: stop.clone(),
        usage: Usage::default(),
    }]);
    let r = p.send(&LlmRequest::default()).await.unwrap();
    assert_eq!(r.stop, stop);
}

#[tokio::test]
async fn scripted_reply_error_is_a_structured_error() {
    let body = parse_error_body(
        "anthropic",
        ErrorOrigin::Http { status: 529 },
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        None,
        None,
    );
    let mut p = MockProvider::new(vec![ScriptedTurn::ReplyError { body }]);
    let e = p.send(&LlmRequest::default()).await.unwrap_err();
    assert_eq!(e.class(), ErrorClass::Overloaded);
    assert!(matches!(e, ProviderError::Reply(_)));
}

#[tokio::test]
async fn provider_id_is_configurable_and_defaults_to_anthropic() {
    let p = MockProvider::new(vec![]);
    assert_eq!(p.provider_id(), ProviderId::Anthropic);
    let p = MockProvider::new(vec![]).with_provider_id(ProviderId::OpenaiCodex);
    assert_eq!(p.provider_id(), ProviderId::OpenaiCodex);
}

#[test]
fn reply_scripts_deserialize_from_json() {
    // RUPU_MOCK_PROVIDER_SCRIPT scripts are JSON: the new variants must parse there too.
    let json = r#"[{"Reply":{"content":[{"type":"text","text":"hi"}],
        "stop":{"reason":"pause_turn","wire":{"provider":"anthropic","value":"pause_turn"}}}}]"#;
    let turns: Vec<ScriptedTurn> = serde_json::from_str(json).unwrap();
    assert!(
        matches!(&turns[0], ScriptedTurn::Reply { stop, .. } if stop.reason == StopReason::PauseTurn)
    );
}
