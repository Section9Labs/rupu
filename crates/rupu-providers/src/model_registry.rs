//! Model resolution aggregator. Sources, in order:
//! 1. Custom (~/.rupu/config.toml [[providers.X.models]])
//! 2. Live cache (~/.rupu/cache/models/<provider>.json, TTL 1h)
//! 3. Baked-in id fallback (Copilot, and Gemini's Code Assist path, which has no
//!    listing). Ids only: it never supplies a limit.
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
/// Cache file schema. v1 had no `schema` field and stored ids only; any other
/// schema (older or newer) is treated as stale.
const CACHE_SCHEMA: u32 = 2;

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
    #[serde(default)]
    pub context_window: u32,
    #[serde(default)]
    pub max_output_tokens: u32,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    schema: u32,
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

    /// When `provider`'s live list was fetched, if it is loaded.
    pub async fn fetched_at(&self, provider: &str) -> Option<DateTime<Utc>> {
        self.state
            .read()
            .await
            .live
            .get(provider)
            .map(|(ts, _)| *ts)
    }

    /// The live entry for `model` (exact id, else newest dated snapshot).
    pub async fn find_live(&self, provider: &str, model: &str) -> Option<ModelInfo> {
        let s = self.state.read().await;
        let (_, entries) = s.live.get(provider)?;
        let id = match_model_id(entries.iter().map(|m| m.id.as_str()), model)?;
        entries.iter().find(|m| m.id == id).cloned()
    }

    pub async fn save_cache(&self, provider: &str) -> Result<()> {
        let s = self.state.read().await;
        if let Some((ts, entries)) = s.live.get(provider) {
            std::fs::create_dir_all(&self.cache_dir)?;
            let path = self.cache_dir.join(format!("{provider}.json"));
            let body = serde_json::to_string(&CacheFile {
                schema: CACHE_SCHEMA,
                fetched_at: *ts,
                models: entries
                    .iter()
                    .map(|e| ModelEntry {
                        id: e.id.clone(),
                        context_window: e.context_window,
                        max_output_tokens: e.max_output_tokens,
                    })
                    .collect(),
            })?;
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let tmp = self.cache_dir.join(format!(
                ".{provider}.json.{}.{}.tmp",
                std::process::id(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            // Atomic replace: write a sibling temp file, then rename over the
            // target. If either step fails, don't strand the temp file.
            if let Err(e) = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &path)) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
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
        if cache.schema != CACHE_SCHEMA {
            tracing::debug!(
                provider,
                path = %path.display(),
                found = cache.schema,
                expected = CACHE_SCHEMA,
                "ignoring model cache with an unsupported schema"
            );
            return Ok(());
        }
        let entries: Vec<ModelInfo> = cache
            .models
            .into_iter()
            .map(|e| {
                let mut m = make_model_info(e.id, provider);
                m.context_window = e.context_window;
                m.max_output_tokens = e.max_output_tokens;
                m
            })
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

/// Strip a trailing `[1m]` opt-in suffix (case-insensitive). Never panics on multi-byte UTF-8.
pub fn strip_1m(model: &str) -> &str {
    let b = model.as_bytes();
    if b.len() >= 4 && b[b.len() - 4..].eq_ignore_ascii_case(b"[1m]") {
        &model[..model.len() - 4]
    } else {
        model
    }
}

/// Exact id, else the newest dated snapshot `<model>-YYYYMMDD` (spec §5).
pub fn match_model_id<'a, I: IntoIterator<Item = &'a str>>(ids: I, model: &str) -> Option<&'a str> {
    let want = strip_1m(model);
    let mut best: Option<&'a str> = None;
    for id in ids {
        if id == want {
            return Some(id);
        }
        let dated = id
            .strip_prefix(want)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()));
        if dated && best.is_none_or(|b| id > b) {
            best = Some(id);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mi(id: &str, cw: u32, mo: u32) -> ModelInfo {
        let mut m = make_model_info(id.to_string(), "anthropic");
        m.context_window = cw;
        m.max_output_tokens = mo;
        m
    }

    #[tokio::test]
    async fn v2_cache_round_trips_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.set_live_cache("anthropic", vec![mi("claude-a", 1_000_000, 128_000)])
            .await;
        r.save_cache("anthropic").await.unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write leaves no temp files");
        let r2 = ModelRegistry::with_cache_dir(tmp.path());
        r2.load_cache("anthropic").await.unwrap();
        let m = r2.find_live("anthropic", "claude-a").await.unwrap();
        assert_eq!(
            (m.context_window, m.max_output_tokens),
            (1_000_000, 128_000)
        );
        assert!(r2.fetched_at("anthropic").await.is_some());
        assert!(!r2.cache_is_stale("anthropic").await);
    }

    #[tokio::test]
    async fn v1_cache_is_treated_as_stale() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("anthropic.json"),
            r#"{"fetched_at":"2026-09-30T00:00:00Z","models":[{"id":"claude-a"}]}"#,
        )
        .unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.load_cache("anthropic").await.unwrap();
        assert!(r.cache_is_stale("anthropic").await);
        assert!(r.find_live("anthropic", "claude-a").await.is_none());
    }

    /// Any schema other than the current one is stale — a file written by a
    /// NEWER rupu (`schema: 3`) has a shape this build cannot vouch for, so it
    /// is ignored just like an older one.
    #[tokio::test]
    async fn a_newer_schema_cache_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("anthropic.json"),
            r#"{"schema":3,"fetched_at":"2026-09-30T00:00:00Z","models":[{"id":"claude-a","context_window":1000,"max_output_tokens":10}]}"#,
        )
        .unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.load_cache("anthropic").await.unwrap();
        assert!(r.cache_is_stale("anthropic").await);
        assert!(r.find_live("anthropic", "claude-a").await.is_none());
    }

    /// The current schema is honoured (guards the `!=` comparison against an
    /// over-eager "ignore everything").
    #[tokio::test]
    async fn a_current_schema_cache_is_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("anthropic.json"),
            format!(
                r#"{{"schema":{CACHE_SCHEMA},"fetched_at":"2026-09-30T00:00:00Z","models":[{{"id":"claude-a","context_window":1000,"max_output_tokens":10}}]}}"#
            ),
        )
        .unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.load_cache("anthropic").await.unwrap();
        let m = r.find_live("anthropic", "claude-a").await.unwrap();
        assert_eq!((m.context_window, m.max_output_tokens), (1000, 10));
    }

    /// A failed write/rename must not strand its temp file in the cache dir.
    #[tokio::test]
    async fn save_cache_failure_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory where the cache file belongs: the final rename fails.
        std::fs::create_dir(tmp.path().join("anthropic.json")).unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.set_live_cache("anthropic", vec![mi("claude-a", 1000, 10)])
            .await;
        assert!(r.save_cache("anthropic").await.is_err());
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "stranded temp files: {leftovers:?}");
    }

    #[tokio::test]
    async fn find_live_strips_the_1m_suffix() {
        let r = ModelRegistry::with_cache_dir("unused");
        r.set_live_cache(
            "anthropic",
            vec![mi("claude-sonnet-4-6", 1_000_000, 64_000)],
        )
        .await;
        for model in ["claude-sonnet-4-6[1m]", "claude-sonnet-4-6[1M]"] {
            let m = r.find_live("anthropic", model).await.unwrap();
            assert_eq!(m.id, "claude-sonnet-4-6", "{model}");
        }
    }

    #[tokio::test]
    async fn find_live_falls_back_to_the_newest_dated_snapshot() {
        let r = ModelRegistry::with_cache_dir("unused");
        r.set_live_cache(
            "anthropic",
            vec![
                mi("claude-haiku-4-5-20250101", 100, 10),
                mi("claude-haiku-4-5-20251001", 200, 20),
                mi("claude-haiku-4-5-extra", 300, 30),
            ],
        )
        .await;
        let m = r.find_live("anthropic", "claude-haiku-4-5").await.unwrap();
        assert_eq!(m.id, "claude-haiku-4-5-20251001");
        // The suffix and the snapshot fallback compose.
        let m = r
            .find_live("anthropic", "claude-haiku-4-5[1m]")
            .await
            .unwrap();
        assert_eq!(m.id, "claude-haiku-4-5-20251001");
        assert!(r.find_live("anthropic", "claude-missing").await.is_none());
    }

    #[test]
    fn match_prefers_exact_then_newest_snapshot() {
        let ids = [
            "claude-haiku-4-5-20250101",
            "claude-haiku-4-5-20251001",
            "claude-haiku-4-5-extra",
        ];
        assert_eq!(
            match_model_id(ids, "claude-haiku-4-5"),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(
            match_model_id(["claude-x", "claude-x-20250101"], "claude-x"),
            Some("claude-x")
        );
        assert_eq!(match_model_id(["claude-y"], "claude-z"), None);
    }

    #[test]
    fn strip_1m_is_case_insensitive() {
        assert_eq!(strip_1m("claude-sonnet-4-6[1m]"), "claude-sonnet-4-6");
        assert_eq!(strip_1m("claude-sonnet-4-6[1M]"), "claude-sonnet-4-6");
        assert_eq!(strip_1m("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(
            match_model_id(["claude-s"], "claude-s[1m]"),
            Some("claude-s")
        );
    }

    #[test]
    fn strip_1m_never_panics_on_multibyte() {
        assert_eq!(strip_1m("a€bc"), "a€bc");
        assert_eq!(strip_1m("gpt-é"), "gpt-é");
        assert_eq!(strip_1m("modèle[1m]"), "modèle");
        assert_eq!(match_model_id(["x"], "é"), None);
    }
}
