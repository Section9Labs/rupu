//! Once SIGTERM has arrived (`credential_writes::terminating()`), the
//! compaction summariser call is not started — in `compact_messages`
//! itself, which `rupu session compact` and the session worker's compaction
//! turn call directly (the agent loop guards its own two call sites
//! before reaching it).
//!
//! Its own test binary: the flag is process-wide and never cleared.

use async_trait::async_trait;
use rupu_agent::compact_messages;
use rupu_providers::credential_writes;
use rupu_providers::types::{ContentBlock, LlmRequest, LlmResponse, Message, Role};
use rupu_providers::{LlmProvider, ProviderError, StreamEvent};

/// A provider the summariser must never reach: any call fails the test.
struct NeverProvider;

#[async_trait]
impl LlmProvider for NeverProvider {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        panic!("the summariser was called while the process is terminating")
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        rupu_providers::ProviderId::Anthropic
    }
}

/// Enough history that there is a middle to summarise (the partition does
/// not return `None` before the summariser would be called).
fn history() -> Vec<Message> {
    (0..6)
        .flat_map(|i| {
            [
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!("question {i} {}", "x".repeat(400)),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: format!("answer {i} {}", "y".repeat(400)),
                    }],
                },
            ]
        })
        .collect()
}

#[tokio::test]
async fn a_terminating_process_starts_no_summariser_call_in_compact_messages() {
    assert!(!credential_writes::terminating(), "fresh process");
    credential_writes::request_termination();

    let mut provider = NeverProvider;
    let err = match compact_messages(&history(), &mut provider, "mock-1", 1_000, 900).await {
        Err(e) => e,
        Ok(None) => panic!("no summariser call: the process is terminating; got Ok(None)"),
        Ok(Some(outcome)) => panic!(
            "no summariser call: the process is terminating; got a compaction of {} messages",
            outcome.summarized_messages
        ),
    };
    assert!(
        err.to_string().contains("terminating"),
        "says why nothing was compacted: {err}"
    );
}
