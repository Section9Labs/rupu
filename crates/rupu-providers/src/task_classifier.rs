//! Task-type classification for routing decisions.
//!
//! Determines intent of a request so the router can apply
//! task-type-specific scoring. Classification uses a fast LLM call
//! with heuristic fallback.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::provider::LlmProvider;
use crate::types::{LlmRequest, Message};

/// Preferred classifier models, tried in order.
const CLASSIFIER_MODELS: &[&str] = &[
    "claude-haiku-4-5-20251001",
    "gpt-4.1-nano",
    "gemini-2.0-flash",
];

/// Maximum time to wait for the classifier LLM call.
const CLASSIFIER_TIMEOUT: Duration = Duration::from_secs(2);

/// The intent category of a request, used for affinity scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Plan,
    Execute,
    Research,
    Synthesize,
    Review,
    Chat,
}

impl TaskType {
    /// Parse from a string (case-insensitive). Returns None for unknown.
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "plan" => Some(Self::Plan),
            "execute" => Some(Self::Execute),
            "research" => Some(Self::Research),
            "synthesize" => Some(Self::Synthesize),
            "review" => Some(Self::Review),
            "chat" => Some(Self::Chat),
            _ => None,
        }
    }

    /// All variants for iteration.
    pub const ALL: &[TaskType] = &[
        TaskType::Plan,
        TaskType::Execute,
        TaskType::Research,
        TaskType::Synthesize,
        TaskType::Review,
        TaskType::Chat,
    ];
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan => write!(f, "plan"),
            Self::Execute => write!(f, "execute"),
            Self::Research => write!(f, "research"),
            Self::Synthesize => write!(f, "synthesize"),
            Self::Review => write!(f, "review"),
            Self::Chat => write!(f, "chat"),
        }
    }
}

/// Classifies request intent for routing decisions.
///
/// Uses a three-tier strategy:
/// 1. Explicit `task_type` on the request (skip classification)
/// 2. Deterministic heuristics from contract action context
/// 3. Fast LLM call using a cheap model (with timeout + fallback)
pub struct TaskClassifier {
    /// A cheap, fast provider for classification calls.
    /// None when only one provider exists (classification skipped).
    classifier_provider: Option<Arc<Mutex<Box<dyn LlmProvider>>>>,
    /// The model to use for classification.
    classifier_model: String,
}

impl TaskClassifier {
    /// Create a classifier with a dedicated cheap provider.
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        let model = Self::pick_classifier_model(provider.default_model());
        Self {
            classifier_provider: Some(Arc::new(Mutex::new(provider))),
            classifier_model: model,
        }
    }

    /// Create a classifier that only uses heuristics (no LLM call).
    /// Used when only one provider exists — no routing decision to make.
    pub fn heuristic_only() -> Self {
        Self {
            classifier_provider: None,
            classifier_model: String::new(),
        }
    }

    /// Pick the best classifier model. Prefers known cheap/fast models.
    fn pick_classifier_model(provider_default: &str) -> String {
        // If the provider default is already a cheap model, use it
        if provider_default.contains("haiku")
            || provider_default.contains("nano")
            || provider_default.contains("flash")
        {
            return provider_default.to_string();
        }
        // Otherwise use our first preference
        CLASSIFIER_MODELS[0].to_string()
    }

    /// Classify a request's task type.
    ///
    /// Checks explicit field, then heuristics, then fast LLM call.
    /// Never blocks — falls back to Chat on any failure.
    pub async fn classify(&self, request: &LlmRequest, contract_action: Option<&str>) -> TaskType {
        // 1. Explicit task_type on request
        if let Some(tt) = request.task_type {
            debug!(task_type = %tt, "using explicit task_type from request");
            return tt;
        }

        // 2. Heuristic from contract action
        if let Some(tt) = Self::heuristic_classify(Some(&request.messages), contract_action) {
            debug!(task_type = %tt, "classified via heuristic");
            return tt;
        }

        // 3. Fast LLM call
        if let Some(ref provider) = self.classifier_provider {
            match self.llm_classify(provider, request).await {
                Some(tt) => {
                    debug!(task_type = %tt, "classified via LLM");
                    return tt;
                }
                None => {
                    debug!("LLM classification failed, defaulting to Chat");
                }
            }
        }

        TaskType::Chat
    }

    /// Heuristic classification from contract actions.
    /// Returns None if no heuristic matches.
    pub fn heuristic_classify(
        _messages: Option<&Vec<Message>>,
        contract_action: Option<&str>,
    ) -> Option<TaskType> {
        match contract_action? {
            "WorkspacePublish" | "SpawnCell" | "KillCell" => Some(TaskType::Execute),
            "GovernanceProposal" | "GovernanceVote" => Some(TaskType::Review),
            "NotifyOperator" => Some(TaskType::Chat),
            _ => None,
        }
    }

    /// Classify via fast LLM call with timeout.
    async fn llm_classify(
        &self,
        provider: &Arc<Mutex<Box<dyn LlmProvider>>>,
        request: &LlmRequest,
    ) -> Option<TaskType> {
        let prompt_summary = request
            .messages
            .last()
            .and_then(|m| m.content.first())
            .and_then(|c| match c {
                crate::types::ContentBlock::Text { text } => Some(if text.len() > 500 {
                    let mut end = 500;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    &text[..end]
                } else {
                    text.as_str()
                }),
                _ => None,
            })
            .unwrap_or("(no text content)");

        let classify_request = LlmRequest {
            model: self.classifier_model.clone(),
            system: Some(
                "Classify the following prompt into exactly one category: \
                 Plan, Execute, Research, Synthesize, Review, Chat. \
                 Respond with a single word."
                    .to_string(),
            ),
            messages: vec![Message::user(prompt_summary)],
            max_tokens: 10,
            tools: vec![],
            cell_id: request.cell_id.clone(),
            trace_id: request.trace_id.clone(),
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };

        let result = tokio::time::timeout(CLASSIFIER_TIMEOUT, async {
            let mut provider = provider.lock().await;
            provider.send(&classify_request).await
        })
        .await;

        match result {
            Ok(Ok(response)) => {
                let text = response.text()?;
                TaskType::from_label(text)
            }
            Ok(Err(e)) => {
                warn!(error = %e, "classifier LLM call failed");
                None
            }
            Err(_) => {
                warn!("classifier LLM call timed out");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_type_serde_roundtrip() {
        for tt in TaskType::ALL {
            let json = serde_json::to_string(tt).unwrap();
            let parsed: TaskType = serde_json::from_str(&json).unwrap();
            assert_eq!(*tt, parsed);
        }
    }

    #[test]
    fn test_task_type_from_label() {
        assert_eq!(TaskType::from_label("Plan"), Some(TaskType::Plan));
        assert_eq!(TaskType::from_label("EXECUTE"), Some(TaskType::Execute));
        assert_eq!(TaskType::from_label("  chat  "), Some(TaskType::Chat));
        assert_eq!(TaskType::from_label("unknown"), None);
        assert_eq!(TaskType::from_label(""), None);
    }

    #[test]
    fn test_task_type_display() {
        assert_eq!(TaskType::Plan.to_string(), "plan");
        assert_eq!(TaskType::Execute.to_string(), "execute");
        assert_eq!(TaskType::Chat.to_string(), "chat");
    }

    #[test]
    fn test_heuristic_classify_contract_actions() {
        assert_eq!(
            TaskClassifier::heuristic_classify(None, Some("WorkspacePublish")),
            Some(TaskType::Execute),
        );
        assert_eq!(
            TaskClassifier::heuristic_classify(None, Some("GovernanceProposal")),
            Some(TaskType::Review),
        );
        assert_eq!(
            TaskClassifier::heuristic_classify(None, Some("NotifyOperator")),
            Some(TaskType::Chat),
        );
        assert_eq!(
            TaskClassifier::heuristic_classify(None, Some("SpawnCell")),
            Some(TaskType::Execute),
        );
    }

    #[test]
    fn test_heuristic_classify_unknown_action_returns_none() {
        assert_eq!(
            TaskClassifier::heuristic_classify(None, Some("UnknownAction")),
            None,
        );
    }

    #[test]
    fn test_heuristic_classify_no_context_returns_none() {
        assert_eq!(TaskClassifier::heuristic_classify(None, None), None);
    }

    #[tokio::test]
    async fn test_classify_with_mock_llm() {
        use crate::types::*;
        use async_trait::async_trait;

        struct ClassifierMock;
        #[async_trait]
        impl crate::provider::LlmProvider for ClassifierMock {
            async fn send(
                &mut self,
                _: &LlmRequest,
            ) -> Result<LlmResponse, crate::error::ProviderError> {
                Ok(LlmResponse {
                    id: "clf".into(),
                    model: "mock".into(),
                    content: vec![ContentBlock::Text {
                        text: "Research".into(),
                    }],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage {
                        input_tokens: 5,
                        output_tokens: 1,
                        ..Default::default()
                    },
                })
            }
            async fn stream(
                &mut self,
                req: &LlmRequest,
                _: &mut (dyn FnMut(StreamEvent) + Send),
            ) -> Result<LlmResponse, crate::error::ProviderError> {
                self.send(req).await
            }
            fn default_model(&self) -> &str {
                "haiku"
            }
            fn provider_id(&self) -> crate::provider_id::ProviderId {
                crate::provider_id::ProviderId::Anthropic
            }
        }

        let classifier = TaskClassifier::new(Box::new(ClassifierMock));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("What is the architecture of this system?")],
            max_tokens: 1000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let result = classifier.classify(&request, None).await;
        assert_eq!(result, TaskType::Research);
    }

    #[tokio::test]
    async fn test_classify_explicit_task_type_skips_llm() {
        let classifier = TaskClassifier::heuristic_only();
        let request = LlmRequest {
            model: "test".into(),
            system: None,
            messages: vec![Message::user("anything")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: Some(TaskType::Plan),
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let result = classifier.classify(&request, None).await;
        assert_eq!(result, TaskType::Plan);
    }

    #[tokio::test]
    async fn test_classify_heuristic_only_defaults_to_chat() {
        let classifier = TaskClassifier::heuristic_only();
        let request = LlmRequest {
            model: "test".into(),
            system: None,
            messages: vec![Message::user("hello")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        };
        let result = classifier.classify(&request, None).await;
        assert_eq!(result, TaskType::Chat);
    }
}
