//! Provider router with automatic fallback on 429 rate-limit errors.
//!
//! Wraps multiple LLM providers and implements LlmProvider. If the primary
//! provider returns 429, tries the next available provider with its default model.
//! The agent loop uses this transparently via the LlmProvider trait.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::types::{LlmRequest, LlmResponse, StreamEvent};

/// Maximum rounds of trying all providers before giving up.
const MAX_ROUTER_ROUNDS: u32 = 3;
/// Backoff between full rounds when all providers are 429'd.
const ROUTER_ROUND_BACKOFF_MS: u64 = 3000;

/// Wraps multiple LLM providers with automatic fallback on errors.
///
/// On 429/401/403/TokenRefreshFailed from a provider, tries the next.
/// If all fail, calls `store.reload()` to check for new credentials on disk,
/// re-discovers providers, and retries. This enables live credential updates
/// without restarting the organism.
pub struct ProviderRouter {
    providers: Vec<Box<dyn LlmProvider>>,
    /// Credential store for live reload when all providers are exhausted.
    store: Option<std::sync::Arc<dyn crate::credential_source::CredentialSource>>,
    /// Auth.json path for re-creating providers after reload.
    auth_json_path: Option<std::path::PathBuf>,
}

impl ProviderRouter {
    pub fn new(providers: Vec<Box<dyn LlmProvider>>) -> Result<Self, ProviderError> {
        if providers.is_empty() {
            return Err(ProviderError::AuthConfig(
                "ProviderRouter requires at least one provider".into(),
            ));
        }
        Ok(Self {
            providers,
            store: None,
            auth_json_path: None,
        })
    }

    /// Create a router with live-reload support.
    /// When all providers are exhausted, the router calls store.reload() to
    /// pick up credential changes from disk, then re-discovers providers.
    pub fn with_store(
        providers: Vec<Box<dyn LlmProvider>>,
        store: std::sync::Arc<dyn crate::credential_source::CredentialSource>,
        auth_json_path: Option<std::path::PathBuf>,
    ) -> Result<Self, ProviderError> {
        if providers.is_empty() {
            return Err(ProviderError::AuthConfig(
                "ProviderRouter requires at least one provider".into(),
            ));
        }
        Ok(Self {
            providers,
            store: Some(store),
            auth_json_path,
        })
    }

    /// Attempt to reload credentials and re-discover providers.
    /// Returns true if new providers were found.
    fn try_reload(&mut self) -> bool {
        let Some(store) = &self.store else {
            return false;
        };

        if let Err(e) = store.reload() {
            warn!(error = %e, "credential store reload failed");
            return false;
        }

        let registry =
            crate::registry::ProviderRegistry::new(store.clone(), self.auth_json_path.clone());
        let new_providers = registry.discover_all();
        if new_providers.is_empty() {
            return false;
        }

        // Check if we found any providers we didn't have before
        let old_ids: std::collections::HashSet<_> =
            self.providers.iter().map(|p| p.provider_id()).collect();
        let has_new = new_providers
            .iter()
            .any(|p| !old_ids.contains(&p.provider_id()));

        if has_new || new_providers.len() > self.providers.len() {
            info!(
                old = self.providers.len(),
                new = new_providers.len(),
                "providers refreshed after credential reload"
            );
            self.providers = new_providers;
            true
        } else {
            false
        }
    }

    /// If the request model belongs to the given provider, use it as-is.
    /// Otherwise use the provider's default model.
    ///
    /// For OpenAI Codex with ChatGPT OAuth, only Codex-compatible models
    /// (gpt-5.x, codex) are passed through. Other GPT models (gpt-4.1, o4, o3)
    /// are NOT supported on the chatgpt.com/backend-api/codex endpoint and
    /// would return 400. In those cases, use the provider default (gpt-5.3-codex).
    fn select_model(request: &LlmRequest, provider: &dyn LlmProvider) -> String {
        let matches = match provider.provider_id() {
            ProviderId::Anthropic => request.model.starts_with("claude"),
            ProviderId::OpenaiCodex => {
                // Only pass through models compatible with the Codex backend
                request.model.starts_with("gpt-5") || request.model.contains("codex")
            }
            ProviderId::GoogleGeminiCli | ProviderId::GoogleAntigravity => {
                request.model.starts_with("gemini")
            }
            ProviderId::GithubCopilot => true,
        };

        if matches {
            request.model.clone()
        } else {
            provider.default_model().to_string()
        }
    }
}

#[async_trait]
impl LlmProvider for ProviderRouter {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        for round in 0..MAX_ROUTER_ROUNDS {
            if round > 0 {
                let backoff = ROUTER_ROUND_BACKOFF_MS * 2u64.pow(round - 1);
                warn!(
                    round,
                    backoff_ms = backoff,
                    "all providers 429'd, retrying round"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }

            for provider in &mut self.providers {
                let model = Self::select_model(request, provider.as_ref());
                let mut req = request.clone();
                req.model = model;

                match provider.send(&req).await {
                    Ok(resp) => return Ok(resp),
                    Err(ProviderError::Api { status: 429, .. }) => {
                        warn!(
                            provider = %provider.provider_id(),
                            "rate-limited (429), trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::Api {
                        status: status @ (401 | 403),
                        ref message,
                    }) => {
                        warn!(
                            provider = %provider.provider_id(),
                            status,
                            message,
                            "auth failure, trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::TokenRefreshFailed(ref msg)) => {
                        warn!(
                            provider = %provider.provider_id(),
                            error = msg.as_str(),
                            "token refresh failed, trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::Http(ref msg)) => {
                        warn!(
                            provider = %provider.provider_id(),
                            error = msg.as_str(),
                            "network error, trying next provider"
                        );
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Last resort: reload credentials from disk and try once more
        if self.try_reload() {
            for provider in &mut self.providers {
                let model = Self::select_model(request, provider.as_ref());
                let mut req = request.clone();
                req.model = model;
                match provider.send(&req).await {
                    Ok(resp) => return Ok(resp),
                    Err(_) => continue,
                }
            }
        }

        Err(ProviderError::Api {
            status: 503,
            message: "all providers exhausted after all retry rounds".into(),
        })
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        for round in 0..MAX_ROUTER_ROUNDS {
            if round > 0 {
                let backoff = ROUTER_ROUND_BACKOFF_MS * 2u64.pow(round - 1);
                warn!(
                    round,
                    backoff_ms = backoff,
                    "all providers failed, retrying round"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }

            for provider in &mut self.providers {
                let model = Self::select_model(request, provider.as_ref());
                let mut req = request.clone();
                req.model = model;

                match provider.stream(&req, on_event).await {
                    Ok(resp) => return Ok(resp),
                    Err(ProviderError::Api { status: 429, .. }) => {
                        warn!(
                            provider = %provider.provider_id(),
                            "rate-limited (429), trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::Api {
                        status: status @ (401 | 403),
                        ref message,
                    }) => {
                        warn!(
                            provider = %provider.provider_id(),
                            status,
                            message,
                            "auth failure, trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::TokenRefreshFailed(ref msg)) => {
                        warn!(
                            provider = %provider.provider_id(),
                            error = msg.as_str(),
                            "token refresh failed, trying next provider"
                        );
                        continue;
                    }
                    Err(ProviderError::Http(ref msg)) => {
                        warn!(
                            provider = %provider.provider_id(),
                            error = msg.as_str(),
                            "network error, trying next provider"
                        );
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        // Last resort: reload credentials from disk and try once more
        if self.try_reload() {
            for provider in &mut self.providers {
                let model = Self::select_model(request, provider.as_ref());
                let mut req = request.clone();
                req.model = model;
                match provider.stream(&req, on_event).await {
                    Ok(resp) => return Ok(resp),
                    Err(_) => continue,
                }
            }
        }

        Err(ProviderError::Api {
            status: 503,
            message: "all providers exhausted after all retry rounds".into(),
        })
    }

    fn default_model(&self) -> &str {
        self.providers[0].default_model()
    }

    fn provider_id(&self) -> ProviderId {
        self.providers[0].provider_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    struct OkProvider {
        model: String,
        id: ProviderId,
    }

    #[async_trait]
    impl LlmProvider for OkProvider {
        async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Ok(LlmResponse {
                id: "ok".into(),
                model: request.model.clone(),
                content: vec![ContentBlock::Text { text: "ok".into() }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..Default::default()
                },
            })
        }
        async fn stream(
            &mut self,
            req: &LlmRequest,
            _: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            self.send(req).await
        }
        fn default_model(&self) -> &str {
            &self.model
        }
        fn provider_id(&self) -> ProviderId {
            self.id
        }
    }

    struct RateLimitedProvider {
        id: ProviderId,
    }

    #[async_trait]
    impl LlmProvider for RateLimitedProvider {
        async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Err(ProviderError::Api {
                status: 429,
                message: "rate limited".into(),
            })
        }
        async fn stream(
            &mut self,
            _: &LlmRequest,
            _: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            Err(ProviderError::Api {
                status: 429,
                message: "rate limited".into(),
            })
        }
        fn default_model(&self) -> &str {
            "limited"
        }
        fn provider_id(&self) -> ProviderId {
            self.id
        }
    }

    fn test_request() -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("test")],
            max_tokens: 10,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
        }
    }

    #[tokio::test]
    async fn routes_to_primary_when_healthy() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![Box::new(OkProvider {
            model: "claude-sonnet-4-6".into(),
            id: ProviderId::Anthropic,
        })];
        let mut router = ProviderRouter::new(providers).unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert_eq!(resp.model, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn falls_back_on_429() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(RateLimitedProvider {
                id: ProviderId::Anthropic,
            }),
            Box::new(OkProvider {
                model: "o4-mini".into(),
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert_eq!(resp.model, "o4-mini");
    }

    #[tokio::test]
    async fn all_rate_limited_returns_error() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(RateLimitedProvider {
                id: ProviderId::Anthropic,
            }),
            Box::new(RateLimitedProvider {
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let err = router.send(&test_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Api { status: 503, .. }));
    }

    #[tokio::test]
    async fn single_provider_delegates() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![Box::new(OkProvider {
            model: "claude-sonnet-4-6".into(),
            id: ProviderId::Anthropic,
        })];
        let router = ProviderRouter::new(providers).unwrap();
        assert_eq!(router.default_model(), "claude-sonnet-4-6");
        assert_eq!(router.provider_id(), ProviderId::Anthropic);
    }

    #[tokio::test]
    async fn stream_falls_back_on_429() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(RateLimitedProvider {
                id: ProviderId::Anthropic,
            }),
            Box::new(OkProvider {
                model: "o4-mini".into(),
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let resp = router.stream(&test_request(), &mut |_| {}).await.unwrap();
        assert_eq!(resp.model, "o4-mini");
    }

    #[test]
    fn select_model_keeps_matching_model() {
        let provider = OkProvider {
            model: "claude-sonnet-4-6".into(),
            id: ProviderId::Anthropic,
        };
        let req = test_request(); // model = "claude-sonnet-4-6"
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn select_model_uses_default_for_wrong_provider() {
        let provider = OkProvider {
            model: "o4-mini".into(),
            id: ProviderId::OpenaiCodex,
        };
        let req = test_request(); // model = "claude-sonnet-4-6" (Anthropic model)
        assert_eq!(ProviderRouter::select_model(&req, &provider), "o4-mini");
    }

    #[test]
    fn select_model_gemini_keeps_gemini_prefix() {
        let provider = OkProvider {
            model: "gemini-2.5-pro".into(),
            id: ProviderId::GoogleGeminiCli,
        };
        let mut req = test_request();
        req.model = "gemini-1.5-pro".into();
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "gemini-1.5-pro"
        );
    }

    #[test]
    fn select_model_gemini_uses_default_for_non_gemini() {
        let provider = OkProvider {
            model: "gemini-2.5-pro".into(),
            id: ProviderId::GoogleGeminiCli,
        };
        let req = test_request(); // "claude-sonnet-4-6"
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "gemini-2.5-pro"
        );
    }

    #[test]
    fn select_model_antigravity_keeps_gemini_prefix() {
        let provider = OkProvider {
            model: "gemini-pro".into(),
            id: ProviderId::GoogleAntigravity,
        };
        let mut req = test_request();
        req.model = "gemini-ultra".into();
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "gemini-ultra"
        );
    }

    #[test]
    fn select_model_codex_passes_gpt5_models() {
        let provider = OkProvider {
            model: "gpt-5.3-codex".into(),
            id: ProviderId::OpenaiCodex,
        };
        let mut req = test_request();
        req.model = "gpt-5.4".into();
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "gpt-5.4",
            "gpt-5.x should be passed through to Codex backend"
        );
    }

    #[test]
    fn select_model_codex_rejects_gpt4_models() {
        let provider = OkProvider {
            model: "gpt-5.3-codex".into(),
            id: ProviderId::OpenaiCodex,
        };
        let mut req = test_request();
        req.model = "gpt-4.1".into();
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "gpt-5.3-codex",
            "gpt-4.1 not supported on Codex backend — should use provider default"
        );
    }

    #[test]
    fn select_model_copilot_always_keeps_original() {
        let provider = OkProvider {
            model: "gpt-4o".into(),
            id: ProviderId::GithubCopilot,
        };
        let req = test_request(); // "claude-sonnet-4-6"
        assert_eq!(
            ProviderRouter::select_model(&req, &provider),
            "claude-sonnet-4-6"
        );
    }

    #[tokio::test]
    async fn non_429_error_propagates_without_fallback() {
        struct ErrorProvider {
            status: u16,
        }
        #[async_trait]
        impl LlmProvider for ErrorProvider {
            async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::Api {
                    status: self.status,
                    message: "server error".into(),
                })
            }
            async fn stream(
                &mut self,
                _: &LlmRequest,
                _: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::Api {
                    status: self.status,
                    message: "server error".into(),
                })
            }
            fn default_model(&self) -> &str {
                "err"
            }
            fn provider_id(&self) -> ProviderId {
                ProviderId::Anthropic
            }
        }

        // Primary returns 500; secondary would succeed if reached
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(ErrorProvider { status: 500 }),
            Box::new(OkProvider {
                model: "o4-mini".into(),
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let err = router.send(&test_request()).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Api { status: 500, .. }),
            "non-429 error must propagate immediately"
        );
    }

    #[test]
    fn new_with_empty_providers_returns_error() {
        let result = ProviderRouter::new(vec![]);
        assert!(result.is_err());
    }

    struct AuthFailProvider;

    #[async_trait]
    impl LlmProvider for AuthFailProvider {
        async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Err(ProviderError::Api {
                status: 401,
                message: "invalid_grant".into(),
            })
        }
        async fn stream(
            &mut self,
            _: &LlmRequest,
            _: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            Err(ProviderError::Api {
                status: 401,
                message: "invalid_grant".into(),
            })
        }
        fn default_model(&self) -> &str {
            "broken"
        }
        fn provider_id(&self) -> ProviderId {
            ProviderId::Anthropic
        }
    }

    #[tokio::test]
    async fn falls_back_on_401_auth_failure() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(AuthFailProvider),
            Box::new(OkProvider {
                model: "o4-mini".into(),
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert_eq!(resp.model, "o4-mini", "should fall back to OpenAI on 401");
    }

    #[tokio::test]
    async fn falls_back_on_token_refresh_failure() {
        struct RefreshFailProvider;

        #[async_trait]
        impl LlmProvider for RefreshFailProvider {
            async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::TokenRefreshFailed("invalid_grant".into()))
            }
            async fn stream(
                &mut self,
                _: &LlmRequest,
                _: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, ProviderError> {
                Err(ProviderError::TokenRefreshFailed("invalid_grant".into()))
            }
            fn default_model(&self) -> &str {
                "broken"
            }
            fn provider_id(&self) -> ProviderId {
                ProviderId::Anthropic
            }
        }

        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(RefreshFailProvider),
            Box::new(OkProvider {
                model: "o4-mini".into(),
                id: ProviderId::OpenaiCodex,
            }),
        ];
        let mut router = ProviderRouter::new(providers).unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert_eq!(
            resp.model, "o4-mini",
            "should fall back on token refresh failure"
        );
    }
}
