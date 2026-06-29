//! Live smoke for an OpenAI-compatible endpoint. Ignored by default; run with:
//!   RUPU_LIVE_OAI_BASE_URL=http://host:8080 \
//!   RUPU_LIVE_OAI_KEY=sk-... \
//!   RUPU_LIVE_OAI_MODEL=/raid/models/zai-org/GLM-5.2-FP8 \
//!   cargo test -p rupu-providers --test openai_compatible_live -- --ignored --nocapture

use rupu_providers::provider::LlmProvider;
use rupu_providers::types::{LlmRequest, Message};

#[tokio::test]
#[ignore]
async fn live_non_streaming_completion() {
    let base = std::env::var("RUPU_LIVE_OAI_BASE_URL").expect("RUPU_LIVE_OAI_BASE_URL");
    let key = std::env::var("RUPU_LIVE_OAI_KEY").expect("RUPU_LIVE_OAI_KEY");
    let model = std::env::var("RUPU_LIVE_OAI_MODEL").expect("RUPU_LIVE_OAI_MODEL");

    let mut client =
        rupu_providers::OpenAiCompatibleClient::new(&base, &key, &model, vec![], false);
    let req = LlmRequest {
        model: model.clone(),
        messages: vec![Message::user("What is 17 * 23? Reply with just the number.")],
        max_tokens: 64,
        ..Default::default()
    };
    let resp = client.send(&req).await.expect("send");
    let text = resp.text().unwrap_or("");
    println!("model={} reply={text}", resp.model);
    assert!(text.contains("391"), "expected 391 in reply, got: {text}");
}
