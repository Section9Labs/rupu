//! Model scoring engine for smart routing.
//!
//! Pure function: takes models, task type, budget state, and history,
//! returns a ranked list of scored models. No state, no side effects.

use crate::model_pool::{ModelCapability, ModelInfo, ModelState};
use crate::routing_history::RoutingHistory;
use crate::task_classifier::TaskType;

/// Budget enforcement mode for the smart router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMode {
    Unlimited,
    CostAware,
    BudgetStrict,
}

impl Default for BudgetMode {
    fn default() -> Self {
        Self::Unlimited
    }
}

/// Budget state snapshot passed from CostTracker.
#[derive(Debug, Clone)]
pub struct BudgetState {
    pub usd_remaining: Option<f64>,
    pub tokens_remaining: Option<u64>,
    pub budget_mode: BudgetMode,
}

impl Default for BudgetState {
    fn default() -> Self {
        Self {
            usd_remaining: None,
            tokens_remaining: None,
            budget_mode: BudgetMode::Unlimited,
        }
    }
}

/// Score breakdown for debug logging.
#[derive(Debug, Clone)]
pub struct ScoreBreakdown {
    pub affinity: f64,
    pub cost: f64,
    pub health: f64,
    pub capability: f64,
    pub context: f64,
    pub history: f64,
}

/// A model with its computed score.
#[derive(Debug, Clone)]
pub struct ScoredModel {
    pub model: ModelInfo,
    pub score: f64,
    pub breakdown: ScoreBreakdown,
}

/// Rank models for a given task type and budget state.
///
/// Returns models sorted by score descending. Models missing required
/// capabilities are filtered out (score 0.0, not included).
pub fn rank(
    task_type: TaskType,
    models: &[ModelInfo],
    budget: &BudgetState,
    history: &RoutingHistory,
) -> Vec<ScoredModel> {
    let mut scored: Vec<ScoredModel> = models
        .iter()
        .filter_map(|model| {
            let breakdown = score_model(model, task_type, budget, history);

            // Normalized weights: variable dimensions + remaining distributed to fixed
            let w_affinity = weight_affinity(task_type);
            let w_cost = weight_cost(budget.budget_mode);
            let w_context = weight_context(task_type);
            let variable_sum = w_affinity + w_cost + w_context;
            // Remaining weight distributed to health, capability, history
            let remaining = (1.0 - variable_sum).max(0.0);
            let w_health = remaining * 0.45;
            let w_capability = remaining * 0.30;
            let w_history = remaining * 0.25;

            let total = breakdown.affinity * w_affinity
                + breakdown.cost * w_cost
                + breakdown.health * w_health
                + breakdown.capability * w_capability
                + breakdown.context * w_context
                + breakdown.history * w_history;

            // Filter out models with zero capability (missing required caps)
            if breakdown.capability < 0.01 {
                return None;
            }

            // In budget_strict mode, filter out models with zero cost score
            if budget.budget_mode == BudgetMode::BudgetStrict && breakdown.cost < 0.01 {
                return None;
            }

            Some(ScoredModel {
                model: model.clone(),
                score: total,
                breakdown,
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

fn score_model(
    model: &ModelInfo,
    task_type: TaskType,
    budget: &BudgetState,
    history: &RoutingHistory,
) -> ScoreBreakdown {
    ScoreBreakdown {
        affinity: score_affinity(model, task_type),
        cost: score_cost(model, budget),
        health: score_health(model),
        capability: score_capability(model, task_type),
        context: score_context(model),
        history: history.success_rate(model.provider, &model.id, task_type),
    }
}

/// Task-type affinity: how well this model fits the task.
fn score_affinity(model: &ModelInfo, task_type: TaskType) -> f64 {
    let has_reasoning = model.has_capability(&ModelCapability::Reasoning);
    let has_long_ctx = model.context_window >= 100_000;
    let has_streaming = model.has_capability(&ModelCapability::Streaming);
    let has_tool_use = model.has_capability(&ModelCapability::ToolUse);
    let has_structured = model.has_capability(&ModelCapability::StructuredOutput);

    match task_type {
        TaskType::Plan => {
            let mut s: f64 = 0.3;
            if has_reasoning {
                s += 0.4;
            }
            if has_long_ctx {
                s += 0.2;
            }
            f64::min(s, 1.0)
        }
        TaskType::Execute => {
            let mut s: f64 = 0.3;
            if has_tool_use {
                s += 0.4;
            }
            if has_structured {
                s += 0.2;
            }
            f64::min(s, 1.0)
        }
        TaskType::Research => {
            let mut s: f64 = 0.2;
            if has_reasoning {
                s += 0.3;
            }
            if has_long_ctx {
                s += 0.4;
            }
            f64::min(s, 1.0)
        }
        TaskType::Synthesize => {
            let mut s: f64 = 0.3;
            if has_long_ctx {
                s += 0.4;
            }
            if has_streaming {
                s += 0.2;
            }
            f64::min(s, 1.0)
        }
        TaskType::Review => {
            let mut s: f64 = 0.3;
            if has_reasoning {
                s += 0.4;
            }
            if has_structured {
                s += 0.2;
            }
            f64::min(s, 1.0)
        }
        TaskType::Chat => {
            let mut s: f64 = 0.4;
            if has_streaming {
                s += 0.3;
            }
            f64::min(s, 1.0)
        }
    }
}

/// Cost score: cheaper = higher score.
fn score_cost(model: &ModelInfo, budget: &BudgetState) -> f64 {
    let cost_per_1k = (model.cost.input_per_million + model.cost.output_per_million) / 1000.0;

    if cost_per_1k <= 0.0 {
        return 1.0; // Free model
    }

    // In budget_strict, check if model is affordable
    if budget.budget_mode == BudgetMode::BudgetStrict {
        if let Some(usd) = budget.usd_remaining {
            if cost_per_1k > usd {
                return 0.0;
            }
        }
    }

    // Normalize: ~$60/M combined max → $0.06 per 1k tokens
    let max_cost = 60.0 / 1000.0;
    let ratio = (cost_per_1k / max_cost).min(1.0);
    1.0 - ratio
}

/// Health score from live model status.
fn score_health(model: &ModelInfo) -> f64 {
    match &model.status.state {
        ModelState::Available => {
            let penalty = (model.status.consecutive_failures as f64 * 0.1).min(0.3);
            1.0 - penalty
        }
        ModelState::Degraded => 0.3,
        ModelState::RateLimited { .. } => 0.1,
        ModelState::QuotaExhausted { .. } => 0.05,
        ModelState::Unavailable { .. } => 0.0,
    }
}

/// Capability check: 1.0 if model has all required caps, 0.0 if missing any.
fn score_capability(model: &ModelInfo, task_type: TaskType) -> f64 {
    let required = required_capabilities(task_type);
    if required.iter().all(|cap| model.has_capability(cap)) {
        1.0
    } else {
        0.0
    }
}

/// Required capabilities per task type (hard filter — models without these are excluded).
fn required_capabilities(task_type: TaskType) -> Vec<ModelCapability> {
    match task_type {
        TaskType::Plan => vec![ModelCapability::Reasoning],
        TaskType::Execute => vec![ModelCapability::ToolUse],
        TaskType::Research => vec![ModelCapability::LongContext],
        TaskType::Review => vec![ModelCapability::Reasoning],
        TaskType::Chat | TaskType::Synthesize => vec![ModelCapability::Streaming],
    }
}

/// Context window score: larger = higher, normalized.
fn score_context(model: &ModelInfo) -> f64 {
    let ctx = model.context_window as f64;
    ((ctx - 8_000.0) / (1_000_000.0 - 8_000.0)).clamp(0.0, 1.0)
}

/// Weight for affinity dimension, varies by task type.
fn weight_affinity(task_type: TaskType) -> f64 {
    match task_type {
        TaskType::Plan | TaskType::Research | TaskType::Review => 0.25,
        TaskType::Execute | TaskType::Synthesize => 0.20,
        TaskType::Chat => 0.15,
    }
}

/// Weight for cost dimension, varies by budget mode.
fn weight_cost(mode: BudgetMode) -> f64 {
    match mode {
        BudgetMode::Unlimited => 0.10,
        BudgetMode::CostAware => 0.40,
        BudgetMode::BudgetStrict => 0.80,
    }
}

/// Weight for context dimension, varies by task type.
fn weight_context(task_type: TaskType) -> f64 {
    match task_type {
        TaskType::Research | TaskType::Synthesize => 0.15,
        TaskType::Plan => 0.10,
        _ => 0.05,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_pool::{ModelCost, ModelStatus};
    use crate::provider_id::ProviderId;

    fn make_model(
        id: &str,
        provider: ProviderId,
        caps: Vec<ModelCapability>,
        ctx: u32,
        input_cost: f64,
        output_cost: f64,
    ) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            provider,
            context_window: ctx,
            max_output_tokens: ctx / 4,
            capabilities: caps,
            cost: ModelCost {
                input_per_million: input_cost,
                output_per_million: output_cost,
            },
            status: ModelStatus::default(),
        }
    }

    fn cheap_chat_model() -> ModelInfo {
        make_model(
            "haiku",
            ProviderId::Anthropic,
            vec![ModelCapability::Streaming, ModelCapability::ToolUse],
            200_000,
            0.80,
            4.0,
        )
    }

    fn expensive_reasoning_model() -> ModelInfo {
        make_model(
            "opus",
            ProviderId::Anthropic,
            vec![
                ModelCapability::Streaming,
                ModelCapability::ToolUse,
                ModelCapability::Reasoning,
                ModelCapability::LongContext,
            ],
            200_000,
            15.0,
            75.0,
        )
    }

    fn mid_tier_model() -> ModelInfo {
        make_model(
            "sonnet",
            ProviderId::Anthropic,
            vec![
                ModelCapability::Streaming,
                ModelCapability::ToolUse,
                ModelCapability::Reasoning,
            ],
            200_000,
            3.0,
            15.0,
        )
    }

    fn openai_model() -> ModelInfo {
        make_model(
            "gpt-5.4",
            ProviderId::OpenaiCodex,
            vec![
                ModelCapability::Streaming,
                ModelCapability::ToolUse,
                ModelCapability::Reasoning,
                ModelCapability::LongContext,
            ],
            1_050_000,
            2.0,
            8.0,
        )
    }

    #[test]
    fn test_chat_prefers_cheap_streaming() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let models = vec![cheap_chat_model(), expensive_reasoning_model()];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Chat, &models, &budget, &history);
        assert!(!ranked.is_empty());
        assert_eq!(
            ranked[0].model.id, "haiku",
            "chat should prefer cheap model"
        );
    }

    #[test]
    fn test_plan_prefers_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let models = vec![
            cheap_chat_model(),
            expensive_reasoning_model(),
            mid_tier_model(),
        ];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Plan, &models, &budget, &history);
        assert!(ranked[0].model.has_capability(&ModelCapability::Reasoning));
    }

    #[test]
    fn test_budget_strict_filters_expensive() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let models = vec![cheap_chat_model(), expensive_reasoning_model()];
        let budget = BudgetState {
            usd_remaining: Some(0.001),
            tokens_remaining: None,
            budget_mode: BudgetMode::BudgetStrict,
        };

        let ranked = rank(TaskType::Plan, &models, &budget, &history);
        assert!(
            ranked.iter().all(|m| m.model.id != "opus"),
            "opus should be filtered out in strict mode with low budget"
        );
    }

    #[test]
    fn test_cost_aware_prefers_cheaper() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let models = vec![mid_tier_model(), expensive_reasoning_model()];
        let budget = BudgetState {
            usd_remaining: None,
            tokens_remaining: None,
            budget_mode: BudgetMode::CostAware,
        };

        let ranked = rank(TaskType::Chat, &models, &budget, &history);
        assert_eq!(
            ranked[0].model.id, "sonnet",
            "cost_aware chat should prefer cheaper model"
        );
    }

    #[test]
    fn test_execute_requires_tool_use() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let no_tools = make_model(
            "no-tools",
            ProviderId::Anthropic,
            vec![ModelCapability::Streaming],
            100_000,
            1.0,
            5.0,
        );
        let with_tools = cheap_chat_model();
        let models = vec![no_tools, with_tools];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Execute, &models, &budget, &history);
        assert!(ranked
            .iter()
            .all(|m| m.model.has_capability(&ModelCapability::ToolUse)));
    }

    #[test]
    fn test_unavailable_model_scores_zero_health() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let mut model = cheap_chat_model();
        model.status.state = ModelState::Unavailable {
            reason: "down".into(),
        };
        let healthy = mid_tier_model();
        let models = vec![model, healthy];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Chat, &models, &budget, &history);
        assert_eq!(
            ranked[0].model.id, "sonnet",
            "healthy model should rank first"
        );
    }

    #[test]
    fn test_history_influences_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.json");
        let mut history = RoutingHistory::load(&path);

        for _ in 0..20 {
            history.record(ProviderId::Anthropic, "sonnet", TaskType::Chat, false, 5000);
        }
        for _ in 0..20 {
            history.record(ProviderId::Anthropic, "haiku", TaskType::Chat, true, 200);
        }

        let models = vec![mid_tier_model(), cheap_chat_model()];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Chat, &models, &budget, &history);
        assert_eq!(
            ranked[0].model.id, "haiku",
            "model with better history should rank higher"
        );
    }

    #[test]
    fn test_empty_models_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let ranked = rank(TaskType::Chat, &[], &BudgetState::default(), &history);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_cross_provider_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let history = RoutingHistory::load(&dir.path().join("h.json"));
        let models = vec![mid_tier_model(), openai_model(), cheap_chat_model()];
        let budget = BudgetState::default();

        let ranked = rank(TaskType::Research, &models, &budget, &history);
        // Only models with LongContext qualify for Research; gpt-5.4 has it
        assert!(!ranked.is_empty());
        assert_eq!(
            ranked[0].model.id, "gpt-5.4",
            "large context + reasoning should win for research"
        );
    }
}
