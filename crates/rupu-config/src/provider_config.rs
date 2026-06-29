//! Per-provider runtime knobs. All fields optional — vendor defaults
//! apply when absent, per Slice B-1 spec §9a.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Discriminates a generic OpenAI-compatible endpoint
    /// (`"openai-compatible"`) from per-provider knob overrides (`None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Request SSE streaming. `None`/`Some(true)` → stream; `Some(false)`
    /// forces non-streaming for servers without SSE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<CustomModel>,
}

/// A user-registered model entry for private, internal, or fine-tuned models
/// that are not listed in the provider's public /models API. Spec §6a.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_kind_and_stream() {
        let toml = r#"
kind = "openai-compatible"
base_url = "http://192.29.35.246:8080"
default_model = "/raid/models/zai-org/GLM-5.2-FP8"
stream = true

[[models]]
id = "/raid/models/zai-org/GLM-5.2-FP8"
context_window = 131072
max_output = 8192
"#;
        let cfg: ProviderConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.kind.as_deref(), Some("openai-compatible"));
        assert_eq!(cfg.stream, Some(true));
        assert_eq!(cfg.base_url.as_deref(), Some("http://192.29.35.246:8080"));
        assert_eq!(cfg.models.len(), 1);
    }
}
