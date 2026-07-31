//! LLM provider trait for polymorphic LLM access.
//!
//! Both `AnthropicClient` (direct API) and `BrokerClient` (credential-brokered)
//! implement this trait. `CellRuntime` holds `Box<dyn LlmProvider>` for clean swap.
//! Spec Phase 3B item 6.

use async_trait::async_trait;

use crate::error::ProviderError;
use crate::provider_id::ProviderId;
use crate::types::{LlmRequest, LlmResponse, StreamEvent};

/// Unified interface for LLM providers.
///
/// Object-safe via `async_trait`. Implementations include:
/// - `AnthropicClient`: direct Anthropic Messages API
/// - `OpenAiCodexClient`, `GoogleGeminiClient`, `GithubCopilotClient`
/// - `BrokerClient`: credential-brokered proxy (Phase 3B)
///
/// The `on_event` parameter uses `&mut dyn FnMut` (trait object) rather than
/// `impl FnMut` (generic) to maintain object safety for `Box<dyn LlmProvider>`.
///
/// `Sync` (not just `Send`) is a supertrait (I-76): `async_trait` boxes
/// `&self` methods as `Pin<Box<dyn Future<Output = _> + Send>>`, and a
/// future that holds `&self` alive across an `.await` is `Send` only if
/// `Self: Sync`. Without this bound, `dyn LlmProvider` (the erased type
/// behind every `Box<dyn LlmProvider>`) can't satisfy that requirement, so
/// a decorator holding `inner: Box<dyn LlmProvider>` cannot forward
/// `list_models` — the only method with a default body — through the
/// trait object at all; the empty-vec default is the only thing callable.
/// Adding `Sync` here is what makes forwarding possible.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a request and get the complete response (non-streaming).
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError>;

    /// Send a request with SSE streaming. Calls `on_event` for each stream event.
    /// Returns the complete response after the stream ends.
    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError>;

    /// The provider's default model name (e.g., "claude-sonnet-4-6", "o4-mini").
    fn default_model(&self) -> &str;

    /// The provider's identity.
    fn provider_id(&self) -> ProviderId;

    /// Discover available models from this provider.
    /// Default returns empty vec — providers override to return known models.
    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        Vec::new()
    }

    /// Liveness + authorization probe: does this provider answer an
    /// authenticated request right now?
    ///
    /// `Ok(())` means an authenticated call succeeded. Errors carry WHY, which
    /// is the point — an operator needs to tell "the key is wrong" from "the
    /// host is unreachable".
    ///
    /// The default is `NotImplemented`, NOT a [`list_models`](Self::list_models)-derived
    /// guess. `list_models` returns a bare `Vec` and folds every failure into
    /// an empty result, so a 401 would be indistinguishable from a provider
    /// with no models — reporting that as healthy is precisely the false green
    /// light this exists to avoid. A provider without a real probe reports
    /// "never probed" instead, which callers must count as neither healthy nor
    /// unhealthy.
    async fn probe(&self) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented {
            provider: self.provider_id().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Message, StopReason, Usage};

    struct MockProvider {
        response: LlmResponse,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn send(&mut self, _request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Ok(self.response.clone())
        }

        async fn stream(
            &mut self,
            _request: &LlmRequest,
            on_event: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            on_event(StreamEvent::TextDelta("Hello".into()));
            Ok(self.response.clone())
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        fn provider_id(&self) -> ProviderId {
            ProviderId::Anthropic
        }
    }

    /// A provider that has not implemented `probe` must report
    /// `NotImplemented` — never `Ok`. Callers map that to "never probed" and
    /// count it as neither healthy nor unhealthy; an `Ok` default would put a
    /// green light on something nobody checked.
    #[tokio::test]
    async fn probe_default_is_not_implemented_never_ok() {
        let p = MockProvider {
            response: mock_response(),
        };
        let err = p
            .probe()
            .await
            .expect_err("the default must not report success");
        assert!(
            matches!(err, ProviderError::NotImplemented { .. }),
            "expected NotImplemented, got {err:?}"
        );
    }

    fn mock_response() -> LlmResponse {
        LlmResponse {
            id: "msg_mock".into(),
            model: "mock".into(),
            content: vec![ContentBlock::Text {
                text: "Hello".into(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn test_trait_send_works() {
        let mut provider: Box<dyn LlmProvider> = Box::new(MockProvider {
            response: mock_response(),
        });
        let request = LlmRequest {
            model: "mock".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let response = provider.send(&request).await.unwrap();
        assert_eq!(response.text(), Some("Hello"));
    }

    #[tokio::test]
    async fn test_trait_stream_with_callback() {
        let mut provider: Box<dyn LlmProvider> = Box::new(MockProvider {
            response: mock_response(),
        });
        let request = LlmRequest {
            model: "mock".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let mut events = Vec::new();
        let response = provider
            .stream(&request, &mut |event| {
                events.push(format!("{:?}", event));
            })
            .await
            .unwrap();
        assert_eq!(response.id, "msg_mock");
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn test_trait_error_path_via_trait_object() {
        struct FailingProvider;
        #[async_trait]
        impl LlmProvider for FailingProvider {
            async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::Api {
                    status: 500,
                    message: "upstream failure".into(),
                })
            }
            async fn stream(
                &mut self,
                _: &LlmRequest,
                _: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::Api {
                    status: 503,
                    message: "unavailable".into(),
                })
            }
            fn default_model(&self) -> &str {
                "failing-model"
            }
            fn provider_id(&self) -> ProviderId {
                ProviderId::Anthropic
            }
        }

        let mut provider: Box<dyn LlmProvider> = Box::new(FailingProvider);
        let request = LlmRequest {
            model: "x".into(),
            system: None,
            messages: vec![],
            max_tokens: 1,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let result = provider.send(&request).await;
        assert!(matches!(
            result,
            Err(ProviderError::Api { status: 500, .. })
        ));
    }
}
