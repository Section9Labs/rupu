//! Model resolution aggregator. Sources, in order:
//! 1. Custom (~/.rupu/config.toml [[providers.X.models]])
//! 2. Live cache (~/.rupu/cache/models/<provider>.json, TTL 1h)
//! 3. Baked-in fallback (Copilot only)
//!
//! Spec §6a-c.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::model_pool::ModelInfo;

const CACHE_TTL_SECS: i64 = 60 * 60; // 1h

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    Custom,
    Live,
    BakedIn,
}

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub entry: ModelInfo,
    pub source: ModelSource,
}

#[derive(Default)]
struct State {
    custom: HashMap<String, Vec<ModelInfo>>,
    live: HashMap<String, (DateTime<Utc>, Vec<ModelInfo>)>,
    baked: HashMap<String, Vec<ModelInfo>>,
}

pub struct ModelRegistry {
    state: Arc<RwLock<State>>,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at: DateTime<Utc>,
    models: Vec<ModelEntry>,
}

impl ModelRegistry {
    pub fn with_cache_dir(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            state: Arc::new(RwLock::new(State::default())),
            cache_dir: cache_dir.into(),
        }
    }

    pub async fn set_custom(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state
            .write()
            .await
            .custom
            .insert(provider.to_string(), entries);
    }

    pub async fn set_live_cache(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state
            .write()
            .await
            .live
            .insert(provider.to_string(), (Utc::now(), entries));
    }

    pub async fn set_baked_in(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state
            .write()
            .await
            .baked
            .insert(provider.to_string(), entries);
    }

    pub async fn list(&self, provider: &str) -> Vec<ResolvedModel> {
        let s = self.state.read().await;
        let mut out: HashMap<String, ResolvedModel> = HashMap::new();
        if let Some(entries) = s.live.get(provider) {
            for e in &entries.1 {
                out.insert(
                    e.id.clone(),
                    ResolvedModel {
                        entry: e.clone(),
                        source: ModelSource::Live,
                    },
                );
            }
        }
        if let Some(entries) = s.baked.get(provider) {
            for e in entries {
                out.entry(e.id.clone()).or_insert(ResolvedModel {
                    entry: e.clone(),
                    source: ModelSource::BakedIn,
                });
            }
        }
        if let Some(entries) = s.custom.get(provider) {
            for e in entries {
                // Custom always wins.
                out.insert(
                    e.id.clone(),
                    ResolvedModel {
                        entry: e.clone(),
                        source: ModelSource::Custom,
                    },
                );
            }
        }
        let mut v: Vec<ResolvedModel> = out.into_values().collect();
        v.sort_by(|a, b| a.entry.id.cmp(&b.entry.id));
        v
    }

    pub async fn resolve(&self, provider: &str, model: &str) -> Result<ResolvedModel> {
        let list = self.list(provider).await;
        list.into_iter()
            .find(|m| m.entry.id == model)
            .ok_or_else(|| {
                anyhow!(
                    "model '{model}' not found for provider '{provider}'. \
                     Run 'rupu models list --provider {provider}' to see available models, \
                     or add a custom entry to ~/.rupu/config.toml."
                )
            })
    }

    pub async fn cache_is_stale(&self, provider: &str) -> bool {
        let s = self.state.read().await;
        match s.live.get(provider) {
            Some((ts, _)) => (Utc::now() - *ts).num_seconds() >= CACHE_TTL_SECS,
            None => true,
        }
    }

    pub async fn save_cache(&self, provider: &str) -> Result<()> {
        let s = self.state.read().await;
        if let Some((ts, entries)) = s.live.get(provider) {
            std::fs::create_dir_all(&self.cache_dir)?;
            let path = self.cache_dir.join(format!("{provider}.json"));
            let body = serde_json::to_string(&CacheFile {
                fetched_at: *ts,
                models: entries
                    .iter()
                    .map(|e| ModelEntry { id: e.id.clone() })
                    .collect(),
            })?;
            std::fs::write(&path, body)?;
        }
        Ok(())
    }

    pub async fn load_cache(&self, provider: &str) -> Result<()> {
        let path = self.cache_dir.join(format!("{provider}.json"));
        if !path.exists() {
            return Ok(());
        }
        let body = std::fs::read_to_string(&path)?;
        let cache: CacheFile = serde_json::from_str(&body)?;
        let entries: Vec<ModelInfo> = cache
            .models
            .into_iter()
            .map(|m| make_model_info(m.id, provider))
            .collect();
        let mut s = self.state.write().await;
        s.live
            .insert(provider.to_string(), (cache.fetched_at, entries));
        Ok(())
    }
}

fn make_model_info(id: String, provider_name: &str) -> ModelInfo {
    let pid = match provider_name {
        "anthropic" => crate::provider_id::ProviderId::Anthropic,
        "openai" | "openai-codex" => crate::provider_id::ProviderId::OpenaiCodex,
        "gemini" | "google-gemini-cli" => crate::provider_id::ProviderId::GoogleGeminiCli,
        "copilot" | "github-copilot" => crate::provider_id::ProviderId::GithubCopilot,
        _ => crate::provider_id::ProviderId::Anthropic,
    };
    ModelInfo {
        id,
        provider: pid,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: crate::model_pool::ModelCost::default(),
        status: crate::model_pool::ModelStatus::default(),
    }
}
