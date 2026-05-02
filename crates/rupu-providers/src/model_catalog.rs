//! TOML-based model catalog loader.
//!
//! Loads model definitions from `cortex/model_catalog.toml` at boot.
//! The catalog provides the baseline; provider API discovery enriches it.

use std::path::Path;

use serde::Deserialize;
use tracing::{info, warn};

use crate::model_pool::{ModelCapability, ModelCost, ModelInfo, ModelStatus};
use crate::provider_id::ProviderId;

/// Raw TOML entry for a model in the catalog.
#[derive(Debug, Deserialize)]
struct CatalogEntry {
    id: String,
    provider: String,
    #[serde(default)]
    context_window: u32,
    #[serde(default)]
    max_output_tokens: u32,
    #[serde(default)]
    capabilities: Vec<ModelCapability>,
    #[serde(default)]
    input_cost_per_million: f64,
    #[serde(default)]
    output_cost_per_million: f64,
}

/// Root TOML structure.
#[derive(Debug, Deserialize)]
struct Catalog {
    #[serde(default)]
    models: Vec<CatalogEntry>,
}

/// Load all models from a catalog TOML file.
/// Returns an empty vec if the file doesn't exist or can't be parsed.
pub fn load_catalog(path: &Path) -> Vec<ModelInfo> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            if path.exists() {
                warn!(path = %path.display(), "failed to read model catalog");
            }
            return Vec::new();
        }
    };

    let catalog: Catalog = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to parse model catalog");
            return Vec::new();
        }
    };

    let mut models = Vec::new();
    for entry in catalog.models {
        let provider = match entry.provider.parse::<ProviderId>() {
            Ok(p) => p,
            Err(_) => {
                warn!(
                    provider = entry.provider.as_str(),
                    model = entry.id.as_str(),
                    "unknown provider in catalog — skipping"
                );
                continue;
            }
        };

        // Validate costs — reject NaN, Infinity, negative
        let input_cost = entry.input_cost_per_million;
        let output_cost = entry.output_cost_per_million;
        if input_cost.is_nan()
            || output_cost.is_nan()
            || input_cost.is_infinite()
            || output_cost.is_infinite()
            || input_cost < 0.0
            || output_cost < 0.0
        {
            warn!(
                model = entry.id.as_str(),
                "invalid cost in catalog — skipping"
            );
            continue;
        }

        models.push(ModelInfo {
            id: entry.id,
            provider,
            context_window: entry.context_window,
            max_output_tokens: entry.max_output_tokens,
            capabilities: entry.capabilities,
            cost: ModelCost {
                input_per_million: input_cost,
                output_per_million: output_cost,
            },
            status: ModelStatus::default(),
        });
    }

    info!(count = models.len(), path = %path.display(), "model catalog loaded");
    models
}

/// Get models from the catalog for a specific provider.
/// Convenience for provider `list_models()` implementations.
pub fn known_models_for_provider(catalog_path: &Path, provider: ProviderId) -> Vec<ModelInfo> {
    load_catalog(catalog_path)
        .into_iter()
        .filter(|m| m.provider == provider)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model_catalog.toml");
        fs::write(
            &path,
            r#"
[[models]]
id = "test-model"
provider = "anthropic"
context_window = 200000
max_output_tokens = 16000
capabilities = ["tool_use", "streaming"]
input_cost_per_million = 3.0
output_cost_per_million = 15.0
"#,
        )
        .unwrap();

        let models = load_catalog(&path);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");
        assert_eq!(models[0].provider, ProviderId::Anthropic);
        assert_eq!(models[0].context_window, 200000);
        assert!(models[0].has_capability(&ModelCapability::ToolUse));
        assert!(models[0].has_capability(&ModelCapability::Streaming));
    }

    #[test]
    fn test_load_catalog_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let models = load_catalog(&dir.path().join("nonexistent.toml"));
        assert!(models.is_empty());
    }

    #[test]
    fn test_load_catalog_unknown_provider_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.toml");
        fs::write(
            &path,
            r#"
[[models]]
id = "good"
provider = "anthropic"
context_window = 100000

[[models]]
id = "bad"
provider = "unknown-provider"
context_window = 50000
"#,
        )
        .unwrap();

        let models = load_catalog(&path);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "good");
    }

    #[test]
    fn test_load_catalog_multiple_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.toml");
        fs::write(
            &path,
            r#"
[[models]]
id = "claude-sonnet-4-6"
provider = "anthropic"
context_window = 200000

[[models]]
id = "gpt-5.4"
provider = "openai-codex"
context_window = 1050000
"#,
        )
        .unwrap();

        let models = load_catalog(&path);
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_known_models_for_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.toml");
        fs::write(
            &path,
            r#"
[[models]]
id = "sonnet"
provider = "anthropic"

[[models]]
id = "haiku"
provider = "anthropic"

[[models]]
id = "gpt"
provider = "openai-codex"
"#,
        )
        .unwrap();

        let anthropic = known_models_for_provider(&path, ProviderId::Anthropic);
        assert_eq!(anthropic.len(), 2);

        let openai = known_models_for_provider(&path, ProviderId::OpenaiCodex);
        assert_eq!(openai.len(), 1);
    }

    #[test]
    fn test_load_real_catalog() {
        let catalog_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("cortex/model_catalog.toml");
        if catalog_path.exists() {
            let models = load_catalog(&catalog_path);
            assert!(
                models.len() >= 10,
                "real catalog should have 10+ models, got {}",
                models.len()
            );
        }
    }
}
