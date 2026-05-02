//! ModelPool — live model registry with status tracking.
//!
//! Aggregates models across all providers. Seeded from `cortex/model_catalog.toml`,
//! enriched by provider API discovery, and continuously updated by runtime events
//! (rate limits, errors, successes, quota headers from API responses).

use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::provider_id::ProviderId;

/// Model capability flags.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    ToolUse,
    Vision,
    Streaming,
    Reasoning,
    LongContext,
    StructuredOutput,
}

/// Per-million-token cost in USD.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCost {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

/// Live model state.
#[derive(Debug, Clone)]
pub enum ModelState {
    Available,
    RateLimited { retry_after: Option<Duration> },
    QuotaExhausted { recheck_after: Instant },
    Degraded,
    Unavailable { reason: String },
}

/// Live model status with utilization and failure tracking.
#[derive(Debug, Clone)]
pub struct ModelStatus {
    pub state: ModelState,
    pub utilization: Option<f64>,
    pub quota_reset: Option<Instant>,
    pub last_success: Option<Instant>,
    pub last_error: Option<Instant>,
    pub consecutive_failures: u32,
}

impl Default for ModelStatus {
    fn default() -> Self {
        Self {
            state: ModelState::Available,
            utilization: None,
            quota_reset: None,
            last_success: None,
            last_error: None,
            consecutive_failures: 0,
        }
    }
}

/// A model's capabilities, cost, and live status.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub provider: ProviderId,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub capabilities: Vec<ModelCapability>,
    pub cost: ModelCost,
    pub status: ModelStatus,
}

impl ModelInfo {
    pub fn is_available(&self) -> bool {
        matches!(self.status.state, ModelState::Available)
    }

    pub fn has_capability(&self, cap: &ModelCapability) -> bool {
        self.capabilities.contains(cap)
    }

    /// Composite key: "provider:model_id"
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.id)
    }
}

/// Parsed from provider API response headers to update model utilization.
#[derive(Debug, Clone, Default)]
pub struct ResponseHeaders {
    pub utilization: Option<f64>,
    pub quota_reset_epoch_secs: Option<u64>,
    pub rate_limit_status: Option<String>,
}

/// Consecutive failures before marking degraded.
const DEGRADED_THRESHOLD: u32 = 3;

/// Aggregates models across all providers with live status tracking.
pub struct ModelPool {
    models: RwLock<Vec<ModelInfo>>,
}

impl ModelPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            models: RwLock::new(Vec::new()),
        }
    }

    /// Load from a pre-parsed list of models (from catalog).
    pub fn from_models(models: Vec<ModelInfo>) -> Self {
        let count = models.len();
        let pool = Self {
            models: RwLock::new(models),
        };
        if count > 0 {
            info!(count, "model pool initialized");
        }
        pool
    }

    /// Merge discovered models from a provider into the pool.
    /// Updates existing models (by provider+id), adds new ones.
    pub fn merge_discovered(&self, provider: ProviderId, discovered: Vec<ModelInfo>) {
        let mut models = match self.models.write() {
            Ok(m) => m,
            Err(_) => return,
        };

        for new_model in discovered {
            if let Some(existing) = models
                .iter_mut()
                .find(|m| m.provider == provider && m.id == new_model.id)
            {
                // Update capabilities and cost from discovery, keep status
                existing.context_window = new_model.context_window;
                existing.max_output_tokens = new_model.max_output_tokens;
                if !new_model.capabilities.is_empty() {
                    existing.capabilities = new_model.capabilities;
                }
                if new_model.cost.input_per_million > 0.0 || new_model.cost.output_per_million > 0.0
                {
                    existing.cost = new_model.cost;
                }
            } else {
                // New model not in catalog
                info!(provider = %provider, model = new_model.id.as_str(), "new model discovered");
                models.push(new_model);
            }
        }
    }

    /// Get a specific model by provider and ID.
    pub fn get(&self, provider: ProviderId, model_id: &str) -> Option<ModelInfo> {
        self.models.read().ok().and_then(|m| {
            m.iter()
                .find(|m| m.provider == provider && m.id == model_id)
                .cloned()
        })
    }

    /// List all available models matching required capabilities.
    pub fn available(&self, required: &[ModelCapability]) -> Vec<ModelInfo> {
        let models = match self.models.read() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        models
            .iter()
            .filter(|m| m.is_available() && required.iter().all(|cap| m.has_capability(cap)))
            .cloned()
            .collect()
    }

    /// List all models for a specific provider (regardless of status).
    pub fn by_provider(&self, provider: ProviderId) -> Vec<ModelInfo> {
        self.models
            .read()
            .ok()
            .map(|m| {
                m.iter()
                    .filter(|m| m.provider == provider)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Total number of models in the pool.
    pub fn model_count(&self) -> usize {
        self.models.read().ok().map(|m| m.len()).unwrap_or(0)
    }

    /// Snapshot of all models (regardless of status).
    pub fn all_models(&self) -> Vec<ModelInfo> {
        self.models
            .read()
            .ok()
            .map(|m| m.to_vec())
            .unwrap_or_default()
    }

    /// Update status after a successful API call.
    pub fn record_success(&self, provider: ProviderId, model_id: &str, headers: &ResponseHeaders) {
        let mut models = match self.models.write() {
            Ok(m) => m,
            Err(_) => return,
        };

        if let Some(model) = models
            .iter_mut()
            .find(|m| m.provider == provider && m.id == model_id)
        {
            model.status.state = ModelState::Available;
            model.status.last_success = Some(Instant::now());
            model.status.consecutive_failures = 0;
            if let Some(util) = headers.utilization {
                model.status.utilization = Some(util);
            }
            if let Some(reset_secs) = headers.quota_reset_epoch_secs {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if reset_secs > now_secs {
                    model.status.quota_reset =
                        Some(Instant::now() + Duration::from_secs(reset_secs - now_secs));
                }
            }
        }
    }

    /// Update status after a 429 rate-limit response.
    pub fn record_rate_limit(
        &self,
        provider: ProviderId,
        model_id: &str,
        retry_after: Option<Duration>,
    ) {
        let mut models = match self.models.write() {
            Ok(m) => m,
            Err(_) => return,
        };

        if let Some(model) = models
            .iter_mut()
            .find(|m| m.provider == provider && m.id == model_id)
        {
            model.status.state = ModelState::RateLimited { retry_after };
            model.status.last_error = Some(Instant::now());
            debug!(provider = %provider, model = model_id, "model rate-limited");
        }
    }

    /// Update status after a non-429 error.
    pub fn record_error(&self, provider: ProviderId, model_id: &str, _reason: &str) {
        let mut models = match self.models.write() {
            Ok(m) => m,
            Err(_) => return,
        };

        if let Some(model) = models
            .iter_mut()
            .find(|m| m.provider == provider && m.id == model_id)
        {
            model.status.last_error = Some(Instant::now());
            model.status.consecutive_failures = model.status.consecutive_failures.saturating_add(1);

            if model.status.consecutive_failures >= DEGRADED_THRESHOLD {
                model.status.state = ModelState::Degraded;
                debug!(
                    provider = %provider,
                    model = model_id,
                    failures = model.status.consecutive_failures,
                    "model degraded"
                );
            }
        }
    }

    /// Auto-recover models whose rate limit / quota windows have passed.
    pub fn refresh_availability(&self) {
        let mut models = match self.models.write() {
            Ok(m) => m,
            Err(_) => return,
        };

        let now = Instant::now();
        for model in models.iter_mut() {
            match &model.status.state {
                ModelState::RateLimited { retry_after } => {
                    // Default to 60s if no retry_after header was provided
                    let effective = retry_after.unwrap_or(Duration::from_secs(60));
                    if let Some(last_err) = model.status.last_error {
                        if now.duration_since(last_err) >= effective {
                            model.status.state = ModelState::Available;
                            debug!(
                                provider = %model.provider,
                                model = model.id.as_str(),
                                "model recovered from rate limit"
                            );
                        }
                    }
                }
                ModelState::QuotaExhausted { recheck_after } => {
                    if now >= *recheck_after {
                        model.status.state = ModelState::Available;
                        debug!(
                            provider = %model.provider,
                            model = model.id.as_str(),
                            "model recovered from quota exhaustion"
                        );
                    }
                }
                ModelState::Degraded => {
                    // Auto-recover after 5 minutes
                    if let Some(last_err) = model.status.last_error {
                        if now.duration_since(last_err) >= Duration::from_secs(300) {
                            model.status.state = ModelState::Available;
                            model.status.consecutive_failures = 0;
                            debug!(
                                provider = %model.provider,
                                model = model.id.as_str(),
                                "model recovered from degraded state"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(id: &str, provider: ProviderId, caps: Vec<ModelCapability>) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            provider,
            context_window: 200_000,
            max_output_tokens: 16_000,
            capabilities: caps,
            cost: ModelCost {
                input_per_million: 3.0,
                output_per_million: 15.0,
            },
            status: ModelStatus::default(),
        }
    }

    #[test]
    fn test_new_pool_is_empty() {
        let pool = ModelPool::new();
        assert_eq!(pool.model_count(), 0);
    }

    #[test]
    fn test_from_models() {
        let models = vec![
            make_model(
                "sonnet",
                ProviderId::Anthropic,
                vec![ModelCapability::ToolUse],
            ),
            make_model(
                "haiku",
                ProviderId::Anthropic,
                vec![ModelCapability::Streaming],
            ),
        ];
        let pool = ModelPool::from_models(models);
        assert_eq!(pool.model_count(), 2);
    }

    #[test]
    fn test_get_model() {
        let pool =
            ModelPool::from_models(vec![make_model("sonnet", ProviderId::Anthropic, vec![])]);
        assert!(pool.get(ProviderId::Anthropic, "sonnet").is_some());
        assert!(pool.get(ProviderId::Anthropic, "opus").is_none());
        assert!(pool.get(ProviderId::OpenaiCodex, "sonnet").is_none());
    }

    #[test]
    fn test_available_filters_by_capability() {
        let pool = ModelPool::from_models(vec![
            make_model(
                "a",
                ProviderId::Anthropic,
                vec![ModelCapability::ToolUse, ModelCapability::Vision],
            ),
            make_model("b", ProviderId::Anthropic, vec![ModelCapability::ToolUse]),
            make_model("c", ProviderId::Anthropic, vec![ModelCapability::Vision]),
        ]);

        let with_tools = pool.available(&[ModelCapability::ToolUse]);
        assert_eq!(with_tools.len(), 2);

        let with_both = pool.available(&[ModelCapability::ToolUse, ModelCapability::Vision]);
        assert_eq!(with_both.len(), 1);
        assert_eq!(with_both[0].id, "a");

        let all = pool.available(&[]);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_available_excludes_non_available() {
        let pool = ModelPool::from_models(vec![
            make_model("a", ProviderId::Anthropic, vec![]),
            make_model("b", ProviderId::Anthropic, vec![]),
        ]);

        // Rate-limit model b
        pool.record_rate_limit(ProviderId::Anthropic, "b", Some(Duration::from_secs(60)));

        let avail = pool.available(&[]);
        assert_eq!(avail.len(), 1);
        assert_eq!(avail[0].id, "a");
    }

    #[test]
    fn test_record_success_resets_state() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);

        // Degrade via errors
        for _ in 0..5 {
            pool.record_error(ProviderId::Anthropic, "a", "test error");
        }
        assert!(!pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());

        // Success recovers
        pool.record_success(ProviderId::Anthropic, "a", &ResponseHeaders::default());
        assert!(pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());
    }

    #[test]
    fn test_degraded_after_consecutive_failures() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);

        // 2 failures — still available
        pool.record_error(ProviderId::Anthropic, "a", "err");
        pool.record_error(ProviderId::Anthropic, "a", "err");
        assert!(pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());

        // 3rd failure — degraded
        pool.record_error(ProviderId::Anthropic, "a", "err");
        let model = pool.get(ProviderId::Anthropic, "a").expect("exists");
        assert!(matches!(model.status.state, ModelState::Degraded));
    }

    #[test]
    fn test_refresh_availability_recovers_rate_limited() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);

        pool.record_rate_limit(ProviderId::Anthropic, "a", Some(Duration::from_millis(1)));
        std::thread::sleep(Duration::from_millis(10));
        pool.refresh_availability();

        assert!(pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());
    }

    #[test]
    fn test_merge_discovered_updates_existing() {
        let pool = ModelPool::from_models(vec![make_model(
            "sonnet",
            ProviderId::Anthropic,
            vec![ModelCapability::ToolUse],
        )]);

        // Discover same model with updated context window
        let discovered = vec![ModelInfo {
            id: "sonnet".into(),
            provider: ProviderId::Anthropic,
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            capabilities: vec![ModelCapability::ToolUse, ModelCapability::Vision],
            cost: ModelCost {
                input_per_million: 3.0,
                output_per_million: 15.0,
            },
            status: ModelStatus::default(),
        }];

        pool.merge_discovered(ProviderId::Anthropic, discovered);

        let model = pool.get(ProviderId::Anthropic, "sonnet").expect("exists");
        assert_eq!(model.context_window, 1_000_000);
        assert_eq!(model.capabilities.len(), 2);
    }

    #[test]
    fn test_merge_discovered_adds_new() {
        let pool =
            ModelPool::from_models(vec![make_model("sonnet", ProviderId::Anthropic, vec![])]);

        pool.merge_discovered(
            ProviderId::Anthropic,
            vec![make_model(
                "opus",
                ProviderId::Anthropic,
                vec![ModelCapability::Reasoning],
            )],
        );

        assert_eq!(pool.model_count(), 2);
        assert!(pool.get(ProviderId::Anthropic, "opus").is_some());
    }

    #[test]
    fn test_by_provider() {
        let pool = ModelPool::from_models(vec![
            make_model("sonnet", ProviderId::Anthropic, vec![]),
            make_model("gpt-5.4", ProviderId::OpenaiCodex, vec![]),
            make_model("haiku", ProviderId::Anthropic, vec![]),
        ]);

        let anthropic = pool.by_provider(ProviderId::Anthropic);
        assert_eq!(anthropic.len(), 2);

        let openai = pool.by_provider(ProviderId::OpenaiCodex);
        assert_eq!(openai.len(), 1);
    }

    #[test]
    fn test_record_success_updates_utilization() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);

        pool.record_success(
            ProviderId::Anthropic,
            "a",
            &ResponseHeaders {
                utilization: Some(0.45),
                ..Default::default()
            },
        );

        let model = pool.get(ProviderId::Anthropic, "a").expect("exists");
        assert_eq!(model.status.utilization, Some(0.45));
    }

    #[test]
    fn test_model_info_key_format() {
        let model = make_model("claude-sonnet-4-6", ProviderId::Anthropic, vec![]);
        assert_eq!(model.key(), "anthropic:claude-sonnet-4-6");
    }

    #[test]
    fn test_record_rate_limit_sets_state() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);
        pool.record_rate_limit(ProviderId::Anthropic, "a", Some(Duration::from_secs(30)));

        let model = pool.get(ProviderId::Anthropic, "a").expect("exists");
        assert!(!model.is_available());
        assert!(model.status.last_error.is_some());
    }

    #[test]
    fn test_rate_limited_none_auto_recovers_with_default_timeout() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);
        pool.record_rate_limit(ProviderId::Anthropic, "a", None);

        // With no retry_after, the default 60s timeout applies
        // Manually set last_error far enough in the past
        {
            let mut models = pool.models.write().expect("lock");
            if let Some(m) = models.iter_mut().find(|m| m.id == "a") {
                m.status.last_error = Some(Instant::now() - Duration::from_secs(61));
            }
        }

        pool.refresh_availability();
        assert!(pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());
    }

    #[test]
    fn test_degraded_auto_recovers_after_timeout() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);

        // Degrade via 3 errors
        for _ in 0..3 {
            pool.record_error(ProviderId::Anthropic, "a", "err");
        }
        assert!(!pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());

        // Manually set last_error far enough in the past (>5 min)
        {
            let mut models = pool.models.write().expect("lock");
            if let Some(m) = models.iter_mut().find(|m| m.id == "a") {
                m.status.last_error = Some(Instant::now() - Duration::from_secs(301));
            }
        }

        pool.refresh_availability();
        assert!(pool
            .get(ProviderId::Anthropic, "a")
            .expect("exists")
            .is_available());
    }

    #[test]
    fn test_record_error_on_nonexistent_model_is_noop() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);
        pool.record_error(ProviderId::Anthropic, "nonexistent", "err");
        pool.record_error(ProviderId::OpenaiCodex, "a", "err"); // wrong provider

        let model = pool.get(ProviderId::Anthropic, "a").expect("exists");
        assert_eq!(model.status.consecutive_failures, 0);
    }

    #[test]
    fn test_merge_preserves_live_status() {
        let pool = ModelPool::from_models(vec![make_model("a", ProviderId::Anthropic, vec![])]);
        for _ in 0..3 {
            pool.record_error(ProviderId::Anthropic, "a", "err");
        }

        pool.merge_discovered(
            ProviderId::Anthropic,
            vec![make_model(
                "a",
                ProviderId::Anthropic,
                vec![ModelCapability::Vision],
            )],
        );

        let model = pool.get(ProviderId::Anthropic, "a").expect("exists");
        assert!(matches!(model.status.state, ModelState::Degraded));
        assert_eq!(model.status.consecutive_failures, 3);
    }
}
