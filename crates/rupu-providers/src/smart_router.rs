//! Smart provider router with health-aware, cost-aware, task-type-aware routing.
//!
//! Replaces ProviderRouter. Routes at the model level across all providers.
//! Scoring uses task-type affinity, cost, health status, capability match,
//! context window, and historical success rate.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::credential_source::CredentialSource;
use crate::error::ProviderError;
use crate::model_pool::ModelPool;
use crate::model_scorer::{self, BudgetMode, BudgetState};
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::routing_history::RoutingHistory;
use crate::task_classifier::{TaskClassifier, TaskType};
use crate::types::{LlmRequest, LlmResponse, StreamEvent};

/// Smart router with model-level scoring and fallback.
pub struct SmartRouter {
    providers: HashMap<ProviderId, Box<dyn LlmProvider>>,
    classifier: TaskClassifier,
    model_pool: Arc<ModelPool>,
    history: Arc<RwLock<RoutingHistory>>,
    budget_mode: BudgetMode,
    store: Option<Arc<dyn CredentialSource>>,
    auth_json_path: Option<PathBuf>,
}

impl SmartRouter {
    pub fn new(
        providers: HashMap<ProviderId, Box<dyn LlmProvider>>,
        classifier: TaskClassifier,
        model_pool: Arc<ModelPool>,
        history: Arc<RwLock<RoutingHistory>>,
        budget_mode: BudgetMode,
    ) -> Result<Self, ProviderError> {
        if providers.is_empty() {
            return Err(ProviderError::AuthConfig(
                "SmartRouter requires at least one provider".into(),
            ));
        }
        Ok(Self {
            providers,
            classifier,
            model_pool,
            history,
            budget_mode,
            store: None,
            auth_json_path: None,
        })
    }

    /// Set credential store for live reload support.
    pub fn with_store(
        mut self,
        store: Arc<dyn CredentialSource>,
        auth_json_path: Option<PathBuf>,
    ) -> Self {
        self.store = Some(store);
        self.auth_json_path = auth_json_path;
        self
    }

    /// Attempt credential reload.
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
        let old_count = self.providers.len();
        for provider in new_providers {
            let id = provider.provider_id();
            self.providers.entry(id).or_insert(provider);
        }
        if self.providers.len() > old_count {
            info!(
                old = old_count,
                new = self.providers.len(),
                "providers refreshed"
            );
            true
        } else {
            false
        }
    }
}

/// Check if an error is retriable (should try next model).
fn is_retriable(e: &ProviderError) -> bool {
    matches!(
        e,
        ProviderError::Api {
            status: 429 | 401 | 403,
            ..
        } | ProviderError::TokenRefreshFailed(_)
            | ProviderError::Http(_)
    )
}

/// Record a success outcome against pool + history.
async fn record_ok(
    pool: &ModelPool,
    history: &RwLock<RoutingHistory>,
    provider: ProviderId,
    model_id: &str,
    task_type: TaskType,
    latency_ms: u64,
) {
    pool.record_success(provider, model_id, &Default::default());
    let mut h = history.write().await;
    h.record(provider, model_id, task_type, true, latency_ms);
}

/// Record a failure outcome against pool + history.
async fn record_err(
    pool: &ModelPool,
    history: &RwLock<RoutingHistory>,
    provider: ProviderId,
    model_id: &str,
    task_type: TaskType,
    latency_ms: u64,
    error: &ProviderError,
) {
    match error {
        ProviderError::Api { status: 429, .. } => {
            pool.record_rate_limit(provider, model_id, None);
        }
        _ => {
            pool.record_error(provider, model_id, &error.to_string());
        }
    }
    let mut h = history.write().await;
    h.record(provider, model_id, task_type, false, latency_ms);
}

#[async_trait]
impl LlmProvider for SmartRouter {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        // 0. Refresh model availability (recover from expired rate limits / degraded)
        self.model_pool.refresh_availability();

        // 1. Classify
        let task_type = self.classifier.classify(request, None).await;

        // 2. Score
        let budget = BudgetState {
            usd_remaining: None,
            tokens_remaining: None,
            budget_mode: self.budget_mode,
        };
        let models = self.model_pool.all_models();
        let ranked = {
            let h = self.history.read().await;
            model_scorer::rank(task_type, &models, &budget, &h)
        };

        if ranked.is_empty() {
            warn!("no models after scoring, trying raw providers");
            for provider in self.providers.values_mut() {
                match provider.send(request).await {
                    Ok(resp) => return Ok(resp),
                    Err(e) if is_retriable(&e) => continue,
                    Err(e) => return Err(e),
                }
            }
            return Err(ProviderError::Api {
                status: 503,
                message: "all models exhausted (empty scoring)".into(),
            });
        }

        debug!(
            task_type = %task_type,
            top = ranked[0].model.id.as_str(),
            score = ranked[0].score,
            n = ranked.len(),
            "routing"
        );

        // 3. Try ranked models
        // Clone Arcs so recording doesn't borrow self
        let pool = self.model_pool.clone();
        let history = self.history.clone();

        for scored in &ranked {
            let provider = match self.providers.get_mut(&scored.model.provider) {
                Some(p) => p,
                None => continue,
            };

            let mut req = request.clone();
            req.model = scored.model.id.clone();
            req.task_type = Some(task_type);

            let start = Instant::now();
            match provider.send(&req).await {
                Ok(resp) => {
                    record_ok(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Ok(resp);
                }
                Err(ref e) if is_retriable(e) => {
                    warn!(provider = %scored.model.provider, model = scored.model.id.as_str(), error = %e, "retriable, next model");
                    record_err(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                        e,
                    )
                    .await;
                    continue;
                }
                Err(e) => {
                    record_err(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                        &e,
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        // 4. Last resort: reload and re-score
        if self.try_reload() {
            self.model_pool.refresh_availability();
            let models = self.model_pool.all_models();
            let ranked = {
                let h = self.history.read().await;
                model_scorer::rank(task_type, &models, &budget, &h)
            };
            for scored in &ranked {
                let provider = match self.providers.get_mut(&scored.model.provider) {
                    Some(p) => p,
                    None => continue,
                };
                let mut req = request.clone();
                req.model = scored.model.id.clone();
                match provider.send(&req).await {
                    Ok(resp) => return Ok(resp),
                    Err(_) => continue,
                }
            }
        }

        Err(ProviderError::Api {
            status: 503,
            message: "all models exhausted after smart routing".into(),
        })
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.model_pool.refresh_availability();
        let task_type = self.classifier.classify(request, None).await;

        let budget = BudgetState {
            usd_remaining: None,
            tokens_remaining: None,
            budget_mode: self.budget_mode,
        };
        let models = self.model_pool.all_models();
        let ranked = {
            let h = self.history.read().await;
            model_scorer::rank(task_type, &models, &budget, &h)
        };

        if ranked.is_empty() {
            warn!("no models after scoring, trying raw providers");
            for provider in self.providers.values_mut() {
                match provider.stream(request, on_event).await {
                    Ok(resp) => return Ok(resp),
                    Err(e) if is_retriable(&e) => continue,
                    Err(e) => return Err(e),
                }
            }
            return Err(ProviderError::Api {
                status: 503,
                message: "all models exhausted (empty scoring)".into(),
            });
        }

        debug!(task_type = %task_type, top = ranked[0].model.id.as_str(), score = ranked[0].score, n = ranked.len(), "routing (stream)");

        let pool = self.model_pool.clone();
        let history = self.history.clone();

        for scored in &ranked {
            let provider = match self.providers.get_mut(&scored.model.provider) {
                Some(p) => p,
                None => continue,
            };

            let mut req = request.clone();
            req.model = scored.model.id.clone();
            req.task_type = Some(task_type);

            let start = Instant::now();
            match provider.stream(&req, on_event).await {
                Ok(resp) => {
                    record_ok(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Ok(resp);
                }
                Err(ref e) if is_retriable(e) => {
                    warn!(provider = %scored.model.provider, model = scored.model.id.as_str(), error = %e, "retriable, next model");
                    record_err(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                        e,
                    )
                    .await;
                    continue;
                }
                Err(e) => {
                    record_err(
                        &pool,
                        &history,
                        scored.model.provider,
                        &scored.model.id,
                        task_type,
                        start.elapsed().as_millis() as u64,
                        &e,
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        if self.try_reload() {
            self.model_pool.refresh_availability();
            let models = self.model_pool.all_models();
            let ranked = {
                let h = self.history.read().await;
                model_scorer::rank(task_type, &models, &budget, &h)
            };
            for scored in &ranked {
                let provider = match self.providers.get_mut(&scored.model.provider) {
                    Some(p) => p,
                    None => continue,
                };
                let mut req = request.clone();
                req.model = scored.model.id.clone();
                match provider.stream(&req, on_event).await {
                    Ok(resp) => return Ok(resp),
                    Err(_) => continue,
                }
            }
        }

        Err(ProviderError::Api {
            status: 503,
            message: "all models exhausted after smart routing".into(),
        })
    }

    fn default_model(&self) -> &str {
        self.providers
            .values()
            .next()
            .map(|p| p.default_model())
            .unwrap_or("claude-sonnet-4-6")
    }

    fn provider_id(&self) -> ProviderId {
        self.providers.keys().next().copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_pool::{ModelCapability, ModelCost, ModelInfo, ModelStatus};
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
                    input_tokens: 10,
                    output_tokens: 5,
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

    struct FailProvider {
        id: ProviderId,
        status: u16,
    }

    #[async_trait]
    impl LlmProvider for FailProvider {
        async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Err(ProviderError::Api {
                status: self.status,
                message: "error".into(),
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
            "fail"
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
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: Some(TaskType::Chat),
        }
    }

    fn setup_pool() -> Arc<ModelPool> {
        Arc::new(ModelPool::from_models(vec![
            ModelInfo {
                id: "claude-sonnet-4-6".into(),
                provider: ProviderId::Anthropic,
                context_window: 200_000,
                max_output_tokens: 16_000,
                capabilities: vec![
                    ModelCapability::Streaming,
                    ModelCapability::ToolUse,
                    ModelCapability::Reasoning,
                ],
                cost: ModelCost {
                    input_per_million: 3.0,
                    output_per_million: 15.0,
                },
                status: ModelStatus::default(),
            },
            ModelInfo {
                id: "gpt-5.4".into(),
                provider: ProviderId::OpenaiCodex,
                context_window: 1_050_000,
                max_output_tokens: 100_000,
                capabilities: vec![
                    ModelCapability::Streaming,
                    ModelCapability::ToolUse,
                    ModelCapability::Reasoning,
                ],
                cost: ModelCost {
                    input_per_million: 2.0,
                    output_per_million: 8.0,
                },
                status: ModelStatus::default(),
            },
        ]))
    }

    fn history(dir: &std::path::Path) -> Arc<RwLock<RoutingHistory>> {
        Arc::new(RwLock::new(RoutingHistory::load(&dir.join("h.json"))))
    }

    #[tokio::test]
    async fn test_routes_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut providers: HashMap<ProviderId, Box<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            ProviderId::Anthropic,
            Box::new(OkProvider {
                model: "claude-sonnet-4-6".into(),
                id: ProviderId::Anthropic,
            }),
        );
        providers.insert(
            ProviderId::OpenaiCodex,
            Box::new(OkProvider {
                model: "gpt-5.4".into(),
                id: ProviderId::OpenaiCodex,
            }),
        );

        let mut router = SmartRouter::new(
            providers,
            TaskClassifier::heuristic_only(),
            setup_pool(),
            history(dir.path()),
            BudgetMode::Unlimited,
        )
        .unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert!(resp.model == "claude-sonnet-4-6" || resp.model == "gpt-5.4");
    }

    #[tokio::test]
    async fn test_falls_back_on_429() {
        let dir = tempfile::tempdir().unwrap();
        let mut providers: HashMap<ProviderId, Box<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            ProviderId::Anthropic,
            Box::new(FailProvider {
                id: ProviderId::Anthropic,
                status: 429,
            }),
        );
        providers.insert(
            ProviderId::OpenaiCodex,
            Box::new(OkProvider {
                model: "gpt-5.4".into(),
                id: ProviderId::OpenaiCodex,
            }),
        );

        let mut router = SmartRouter::new(
            providers,
            TaskClassifier::heuristic_only(),
            setup_pool(),
            history(dir.path()),
            BudgetMode::Unlimited,
        )
        .unwrap();
        let resp = router.send(&test_request()).await.unwrap();
        assert_eq!(resp.model, "gpt-5.4");
    }

    #[tokio::test]
    async fn test_all_fail_returns_503() {
        let dir = tempfile::tempdir().unwrap();
        let mut providers: HashMap<ProviderId, Box<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            ProviderId::Anthropic,
            Box::new(FailProvider {
                id: ProviderId::Anthropic,
                status: 429,
            }),
        );
        providers.insert(
            ProviderId::OpenaiCodex,
            Box::new(FailProvider {
                id: ProviderId::OpenaiCodex,
                status: 401,
            }),
        );

        let mut router = SmartRouter::new(
            providers,
            TaskClassifier::heuristic_only(),
            setup_pool(),
            history(dir.path()),
            BudgetMode::Unlimited,
        )
        .unwrap();
        let err = router.send(&test_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Api { status: 503, .. }));
    }

    #[tokio::test]
    async fn test_empty_providers_errors() {
        let result = SmartRouter::new(
            HashMap::new(),
            TaskClassifier::heuristic_only(),
            Arc::new(ModelPool::new()),
            Arc::new(RwLock::new(RoutingHistory::load(std::path::Path::new(
                "/tmp/x.json",
            )))),
            BudgetMode::Unlimited,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_is_retriable() {
        assert!(is_retriable(&ProviderError::Api {
            status: 429,
            message: "".into()
        }));
        assert!(is_retriable(&ProviderError::Api {
            status: 401,
            message: "".into()
        }));
        assert!(is_retriable(&ProviderError::Api {
            status: 403,
            message: "".into()
        }));
        assert!(is_retriable(&ProviderError::TokenRefreshFailed(
            "expired".into()
        )));
        assert!(is_retriable(&ProviderError::Http("".into())));
        assert!(!is_retriable(&ProviderError::Api {
            status: 500,
            message: "".into()
        }));
        assert!(!is_retriable(&ProviderError::Json("".into())));
    }

    #[tokio::test]
    async fn test_stream_falls_back_on_429() {
        let dir = tempfile::tempdir().unwrap();
        let mut providers: HashMap<ProviderId, Box<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            ProviderId::Anthropic,
            Box::new(FailProvider {
                id: ProviderId::Anthropic,
                status: 429,
            }),
        );
        providers.insert(
            ProviderId::OpenaiCodex,
            Box::new(OkProvider {
                model: "gpt-5.4".into(),
                id: ProviderId::OpenaiCodex,
            }),
        );

        let mut router = SmartRouter::new(
            providers,
            TaskClassifier::heuristic_only(),
            setup_pool(),
            history(dir.path()),
            BudgetMode::Unlimited,
        )
        .unwrap();
        let resp = router.stream(&test_request(), &mut |_| {}).await.unwrap();
        assert_eq!(resp.model, "gpt-5.4");
    }

    #[tokio::test]
    async fn test_stream_all_fail_returns_503() {
        let dir = tempfile::tempdir().unwrap();
        let mut providers: HashMap<ProviderId, Box<dyn LlmProvider>> = HashMap::new();
        providers.insert(
            ProviderId::Anthropic,
            Box::new(FailProvider {
                id: ProviderId::Anthropic,
                status: 429,
            }),
        );
        providers.insert(
            ProviderId::OpenaiCodex,
            Box::new(FailProvider {
                id: ProviderId::OpenaiCodex,
                status: 401,
            }),
        );

        let mut router = SmartRouter::new(
            providers,
            TaskClassifier::heuristic_only(),
            setup_pool(),
            history(dir.path()),
            BudgetMode::Unlimited,
        )
        .unwrap();
        let err = router
            .stream(&test_request(), &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Api { status: 503, .. }));
    }
}
