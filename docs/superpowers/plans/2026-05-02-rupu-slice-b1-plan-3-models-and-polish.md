# rupu Slice B-1 — Plan 3: Model resolution & polish

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add live model discovery (`/models` fetch + cache), custom model registration in `~/.rupu/config.toml`, the `rupu models list` and `rupu models refresh` subcommands, the `--no-stream` flag on `rupu run`, full provider documentation (`README` + `docs/providers.md` + four `docs/providers/<name>.md`), Anthropic `cached_tokens` integration, and the gated nightly live-integration test suite. After this plan, Slice B-1 is feature-complete per the design spec.

**Architecture:** A new `model_registry` module in `rupu-providers` aggregates three sources — custom config entries, live-fetched cache, baked-in fallback. Adapters that already have inherent `list_models()` (Anthropic does; the others gain it) feed the live cache. The CLI adds two new subcommands and one new flag; documentation goes under `docs/providers/`. Nightly CI gets a separate workflow gated on `RUPU_LIVE_TESTS=1`.

**Tech Stack:** Rust 2021 (MSRV 1.88), no new workspace deps. Reuses the resolver + provider trait from Plan 2.

**Spec:** `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`

**Prereqs:** Plan 1 and Plan 2 must be merged.

---

## File Structure

```
crates/
  rupu-providers/
    src/
      model_registry.rs           # NEW: aggregator (custom + live + baked-in)
      anthropic.rs                # MODIFY: populate Usage.cached_tokens
      openai_codex.rs             # MODIFY: list_models() impl
      google_gemini.rs            # MODIFY: list_models() impl
      github_copilot.rs           # MODIFY: list_models() returns baked-in list
      lib.rs                      # MODIFY: re-export ModelRegistry
    tests/
      registry_resolution.rs      # NEW: source-precedence + cache-staleness
  rupu-config/
    src/
      provider_config.rs          # MODIFY: add `models: Vec<CustomModel>`
  rupu-transcript/
    src/
      event.rs                    # MODIFY (verify): cached_tokens already on Event::Usage
  rupu-cli/
    src/
      cmd/
        models.rs                 # NEW: `rupu models list | refresh`
        run.rs                    # MODIFY: --no-stream flag
        mod.rs                    # MODIFY: register new subcommand
      main.rs                     # MODIFY: wire models subcommand
    tests/
      models_subcommand.rs        # NEW: list / refresh end-to-end via mocks
      no_stream_flag.rs           # NEW: --no-stream skips streaming events
docs/
  providers.md                    # NEW: canonical multi-provider reference
  providers/
    anthropic.md                  # NEW: API-key + SSO walkthrough
    openai.md                     # NEW
    gemini.md                     # NEW
    copilot.md                    # NEW
  ../README.md                    # MODIFY: add "Configuring providers" quick-start
.github/workflows/
  nightly-live-tests.yml          # NEW: gated by RUPU_LIVE_TESTS=1
CHANGELOG.md                      # MODIFY: Slice B-1 release entry
```

## Important pre-existing state

- After Plan 2, `rupu-providers::AuthCredentials` is the on-the-wire credential shape; provider adapters take it via constructor.
- After Plan 2, `KeychainResolver` and `InMemoryResolver` implement `CredentialResolver`.
- After Plan 1, `Event::Usage { provider, model, input_tokens, output_tokens, cached_tokens }` exists in the transcript schema. Anthropic's `LlmResponse.usage` does NOT currently expose cache hit/miss separately — Plan 3 Task 6 plumbs it through the existing Anthropic SDK response shape.
- The provider trait already has `async fn list_models(&self) -> Vec<ModelInfo>` with a default that returns empty.

---

## Phase 0 — Custom model TOML schema

### Task 1: Add `CustomModel` to `ProviderConfig`

**Files:**
- Modify: `crates/rupu-config/src/provider_config.rs`
- Modify: `crates/rupu-config/tests/provider_config.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-config/tests/provider_config.rs`:

```rust
#[test]
fn provider_config_parses_custom_models() {
    let toml = r#"
[providers.openai]
default_model = "gpt-5"

[[providers.openai.models]]
id = "gpt-5-internal-finetune"
context_window = 200000
max_output = 16000

[[providers.openai.models]]
id = "gpt-5-org-private"
context_window = 128000
"#;
    let cfg: rupu_config::Config = toml::from_str(toml).expect("parse");
    let openai = cfg.providers.get("openai").expect("openai block");
    assert_eq!(openai.models.len(), 2);
    assert_eq!(openai.models[0].id, "gpt-5-internal-finetune");
    assert_eq!(openai.models[0].context_window, Some(200000));
    assert_eq!(openai.models[0].max_output, Some(16000));
    assert_eq!(openai.models[1].id, "gpt-5-org-private");
    assert_eq!(openai.models[1].max_output, None);
}
```

- [ ] **Step 2: Add the type**

Modify `crates/rupu-config/src/provider_config.rs`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
}
```

Add to `ProviderConfig`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub models: Vec<CustomModel>,
```

Re-export in `lib.rs`:

```rust
pub use provider_config::{CustomModel, ProviderConfig};
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-config
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-config/src/provider_config.rs crates/rupu-config/src/lib.rs crates/rupu-config/tests/provider_config.rs
git commit -m "$(cat <<'EOF'
rupu-config: add CustomModel entries to ProviderConfig

[[providers.openai.models]] arrays let users register
private/internal/fine-tuned models that aren't in the public
/models listing. Spec §6a.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Adapter `list_models()`

### Task 2: OpenAI `list_models`

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs`

- [ ] **Step 1: Write the failing test**

Append a unit test to `openai_codex.rs`:

```rust
#[cfg(test)]
mod list_models_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;
    use httpmock::prelude::*;

    #[tokio::test]
    async fn list_models_parses_response() {
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "data": [
                        { "id": "gpt-5", "object": "model" },
                        { "id": "gpt-4o", "object": "model" }
                    ]
                }));
        });
        let creds = AuthCredentials::ApiKey { key: "sk-test".into() };
        let mut client = OpenAiCodexClient::new(creds, None).unwrap();
        client.set_base_url(server.url(""));
        let models = client.list_models().await;
        assert!(models.iter().any(|m| m.id == "gpt-5"));
        assert!(models.iter().any(|m| m.id == "gpt-4o"));
    }
}
```

(`set_base_url` is the test seam — add a public `pub fn set_base_url(&mut self, url: String)` if absent.)

- [ ] **Step 2: Implement `list_models`**

In the `impl LlmProvider for OpenAiCodexClient` block in `openai_codex.rs`, replace the default `list_models` (or add one):

```rust
async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
    let url = format!(
        "{}/v1/models",
        self.api_url.trim_end_matches("/v1/responses").trim_end_matches('/')
    );
    let resp = self
        .client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.access_token))
        .send()
        .await;
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    #[derive(serde::Deserialize)]
    struct ListResp {
        data: Vec<ModelEntry>,
    }
    #[derive(serde::Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let body: ListResp = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    body.data
        .into_iter()
        .map(|e| crate::model_pool::ModelInfo {
            id: e.id,
            ..Default::default()
        })
        .collect()
}
```

(`ModelInfo::Default::default()` already exists in the lifted `model_pool.rs` — confirm by reading.)

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-providers --lib openai_codex::list_models_tests
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/src/openai_codex.rs
git commit -m "$(cat <<'EOF'
rupu-providers: openai list_models() fetches /v1/models

Returns ModelInfo entries for each model in the response. On any
error (401, network, parse), returns empty — let the registry's
caching + fallback handle the user-facing message.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Gemini `list_models`

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs`

- [ ] **Step 1: Write the failing test**

Add a similar httpmock-backed test to `google_gemini.rs` exercising Gemini's `models.list` endpoint shape:

```json
{
  "models": [
    { "name": "models/gemini-2.5-pro", "supportedGenerationMethods": ["generateContent"] },
    { "name": "models/gemini-2.5-flash" }
  ]
}
```

The test asserts `client.list_models()` includes `"gemini-2.5-pro"` (without the `models/` prefix).

- [ ] **Step 2: Implement**

```rust
async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
    let base = self
        .api_url
        .as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com");
    let url = format!("{base}/v1beta/models?key={}", self.api_key);
    let resp = self.client.get(&url).send().await;
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    #[derive(serde::Deserialize)]
    struct ListResp {
        models: Vec<ModelEntry>,
    }
    #[derive(serde::Deserialize)]
    struct ModelEntry {
        name: String,
    }
    let body: ListResp = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    body.models
        .into_iter()
        .map(|e| crate::model_pool::ModelInfo {
            id: e.name.trim_start_matches("models/").to_string(),
            ..Default::default()
        })
        .collect()
}
```

(Field names depend on `GoogleGeminiClient`'s struct — adjust `self.api_key` / `self.api_url` to match. Read the struct first.)

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-providers --lib google_gemini::list_models_tests
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/src/google_gemini.rs
git commit -m "$(cat <<'EOF'
rupu-providers: gemini list_models() via /v1beta/models

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Copilot baked-in model list

**Files:**
- Modify: `crates/rupu-providers/src/github_copilot.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod baked_in_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;

    #[tokio::test]
    async fn list_models_returns_baked_in_when_offline() {
        let creds = AuthCredentials::ApiKey { key: "ghp-test".into() };
        let client = GithubCopilotClient::new(creds, None).unwrap();
        let models = client.list_models().await;
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id == "gpt-4o"));
    }
}
```

- [ ] **Step 2: Implement**

Add to `impl LlmProvider for GithubCopilotClient`:

```rust
async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
    // GitHub Copilot doesn't expose a public /models endpoint.
    // Slice B-1 spec §6a: ship a baked-in list. Users who have access
    // to additional models register them as custom entries in
    // ~/.rupu/config.toml.
    ["gpt-4o", "gpt-4o-mini", "claude-sonnet-4", "o4-mini"]
        .into_iter()
        .map(|id| crate::model_pool::ModelInfo {
            id: id.to_string(),
            ..Default::default()
        })
        .collect()
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-providers --lib github_copilot
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/src/github_copilot.rs
git commit -m "$(cat <<'EOF'
rupu-providers: copilot baked-in model list

Copilot has no public /models endpoint; ship the curated list and
let users add custom entries via [providers.copilot.models] when
their org grants access to additional models.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Model registry

### Task 5: `ModelRegistry` aggregator with cache

**Files:**
- Create: `crates/rupu-providers/src/model_registry.rs`
- Modify: `crates/rupu-providers/src/lib.rs`
- Create: `crates/rupu-providers/tests/registry_resolution.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-providers/tests/registry_resolution.rs`:

```rust
use rupu_providers::model_pool::ModelInfo;
use rupu_providers::model_registry::{ModelEntry, ModelRegistry, ModelSource};

fn mi(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn custom_models_take_precedence_over_live() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_custom("openai", vec![mi("gpt-5-internal-finetune")]).await;
    reg.set_live_cache("openai", vec![mi("gpt-5"), mi("gpt-5-internal-finetune")]).await;
    let models = reg.list("openai").await;
    let custom = models.iter().find(|m| m.entry.id == "gpt-5-internal-finetune").unwrap();
    assert_eq!(custom.source, ModelSource::Custom);
}

#[tokio::test]
async fn unknown_model_resolution_errors_with_actionable_message() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_live_cache("openai", vec![mi("gpt-5")]).await;
    let res = reg.resolve("openai", "gpt-9000").await;
    let err = res.unwrap_err().to_string();
    assert!(err.contains("not found"));
    assert!(err.contains("rupu models list"));
}

#[tokio::test]
async fn known_model_resolves_from_live_cache() {
    let dir = tempfile::tempdir().unwrap();
    let reg = ModelRegistry::with_cache_dir(dir.path());
    reg.set_live_cache("openai", vec![mi("gpt-5")]).await;
    let entry = reg.resolve("openai", "gpt-5").await.unwrap();
    assert_eq!(entry.entry.id, "gpt-5");
    assert_eq!(entry.source, ModelSource::Live);
}
```

- [ ] **Step 2: Implement the registry**

Create `crates/rupu-providers/src/model_registry.rs`:

```rust
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

impl ModelRegistry {
    pub fn with_cache_dir(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            state: Arc::new(RwLock::new(State::default())),
            cache_dir: cache_dir.into(),
        }
    }

    pub async fn set_custom(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state.write().await.custom.insert(provider.to_string(), entries);
    }

    pub async fn set_live_cache(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state
            .write()
            .await
            .live
            .insert(provider.to_string(), (Utc::now(), entries));
    }

    pub async fn set_baked_in(&self, provider: &str, entries: Vec<ModelInfo>) {
        self.state.write().await.baked.insert(provider.to_string(), entries);
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
                models: entries.iter().map(|e| ModelEntry { id: e.id.clone() }).collect(),
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
            .map(|m| ModelInfo {
                id: m.id,
                ..Default::default()
            })
            .collect();
        let mut s = self.state.write().await;
        s.live.insert(provider.to_string(), (cache.fetched_at, entries));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at: DateTime<Utc>,
    models: Vec<ModelEntry>,
}
```

In `lib.rs`:

```rust
pub mod model_registry;
pub use model_registry::{ModelRegistry, ModelSource, ResolvedModel};
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-providers --test registry_resolution
```

Expected: 3 tests green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/src/model_registry.rs crates/rupu-providers/src/lib.rs crates/rupu-providers/tests/registry_resolution.rs
git commit -m "$(cat <<'EOF'
rupu-providers: ModelRegistry aggregator

Spec §6. Custom > Live (TTL 1h) > BakedIn precedence; resolution
errors surface the actionable hint pointing at `rupu models list`
and the config TOML.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Anthropic prompt-cache integration

### Task 6: Populate `cached_tokens` for Anthropic

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs`
- Modify: `crates/rupu-providers/src/types.rs` (add `cached_tokens` field to `Usage`)

- [ ] **Step 1: Write the failing test**

In `crates/rupu-providers/src/anthropic.rs` add a test under the existing test module that decodes a sample Anthropic response with `cache_read_input_tokens` and asserts `Usage.cached_tokens > 0`.

```rust
#[test]
fn decode_response_populates_cached_tokens() {
    let body = r#"{
        "id": "msg_x",
        "model": "claude-sonnet-4-6",
        "content": [{"type":"text","text":"hi"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 200
        }
    }"#;
    let parsed: AnthropicResponse = serde_json::from_str(body).unwrap();
    let resp: LlmResponse = parsed.into();
    assert_eq!(resp.usage.cached_tokens, 200);
}
```

(`AnthropicResponse` is the SDK-shaped struct; `into()` converts to the neutral `LlmResponse`. The exact name lives in `anthropic.rs:829` per Plan 1's exploration.)

- [ ] **Step 2: Add `cached_tokens` field to `Usage`**

Modify `crates/rupu-providers/src/types.rs` `Usage`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cached_tokens: u32,
}
```

- [ ] **Step 3: Map Anthropic's cache fields**

In `anthropic.rs` where `AnthropicResponse` converts to `LlmResponse`, add `cached_tokens: anth.usage.cache_read_input_tokens.unwrap_or(0)` (field name confirmed via the existing struct).

- [ ] **Step 4: Run tests**

```
cargo test -p rupu-providers --lib anthropic
```

Expected: green. The transcript test `usage_event_serde_roundtrip` from Plan 1 still passes (cached_tokens already on the event variant).

- [ ] **Step 5: Wire to the runner**

In `crates/rupu-agent/src/runner.rs`, change the `Event::Usage` emission to use `response.usage.cached_tokens` instead of the hard-coded `0` Plan 1 had.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-providers/src/types.rs crates/rupu-providers/src/anthropic.rs crates/rupu-agent/src/runner.rs
git commit -m "$(cat <<'EOF'
rupu-providers: populate cached_tokens for Anthropic prompt-cache

Reads cache_read_input_tokens from the Anthropic response and
surfaces it through Usage.cached_tokens. The runner now writes a
non-zero cached_tokens to the JSONL transcript when prompt caching
is in play, and the run footer shows '(N cached)' for runs that
benefited.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — `rupu models` subcommand

### Task 7: Implement `rupu models list | refresh`

**Files:**
- Create: `crates/rupu-cli/src/cmd/models.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`
- Modify: `crates/rupu-cli/src/main.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (or wherever the top-level Subcommand enum lives)
- Create: `crates/rupu-cli/tests/models_subcommand.rs`

- [ ] **Step 1: Read existing cmd dispatch**

Read `crates/rupu-cli/src/cmd/mod.rs` and `main.rs` to confirm where the top-level `Cli` clap struct lives and how subcommands are registered.

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-cli/tests/models_subcommand.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use httpmock::prelude::*;

#[test]
fn models_list_prints_table_header() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("SOURCE"));
}

#[test]
fn models_list_copilot_shows_baked_in_entries_offline() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-4o"))
        .stdout(predicate::str::contains("baked-in"));
}

#[test]
fn models_refresh_writes_cache_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{ "id": "gpt-5", "object": "model" }]
        }));
    });
    Command::cargo_bin("rupu")
        .unwrap()
        .env("HOME", dir.path())
        .env("RUPU_CACHE_DIR_OVERRIDE", dir.path())
        .env("OPENAI_API_KEY", "sk-test")
        .env("RUPU_OPENAI_API_URL_OVERRIDE", server.url("/v1/responses"))
        .args(["models", "refresh", "--provider", "openai"])
        .assert()
        .success();
    let cache = dir.path().join("openai.json");
    assert!(cache.exists() || dir.path().join(".rupu/cache/models/openai.json").exists());
}
```

- [ ] **Step 3: Implement the subcommand**

Create `crates/rupu-cli/src/cmd/models.rs`:

```rust
//! `rupu models list | refresh`.

use std::process::ExitCode;

use clap::Subcommand;
use rupu_providers::{ModelRegistry, ModelSource};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List available models (custom + cached + baked-in).
    List {
        #[arg(long)]
        provider: Option<String>,
    },
    /// Re-fetch live model lists from each provider.
    Refresh {
        #[arg(long)]
        provider: Option<String>,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List { provider } => match list(provider).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models list: {e}");
                ExitCode::FAILURE
            }
        },
        Action::Refresh { provider } => match refresh(provider).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models refresh: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

async fn list(filter: Option<String>) -> anyhow::Result<()> {
    let registry = build_registry().await?;
    println!("{:<10} {:<32} {:<10} {}", "PROVIDER", "MODEL", "SOURCE", "CONTEXT");
    for p in &PROVIDERS {
        if let Some(only) = &filter {
            if only != p {
                continue;
            }
        }
        let models = registry.list(p).await;
        for m in models {
            let src = match m.source {
                ModelSource::Custom => "custom",
                ModelSource::Live => "live",
                ModelSource::BakedIn => "baked-in",
            };
            let ctx = m.entry.context_window.map(|c| c.to_string()).unwrap_or_else(|| "-".into());
            println!("{:<10} {:<32} {:<10} {}", p, m.entry.id, src, ctx);
        }
    }
    Ok(())
}

async fn refresh(filter: Option<String>) -> anyhow::Result<()> {
    let registry = build_registry().await?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    for p in &PROVIDERS {
        if let Some(only) = &filter {
            if only != p {
                continue;
            }
        }
        match populate_live(&registry, &resolver, p).await {
            Ok(n) => println!("rupu: refreshed {p} ({n} models)"),
            Err(e) => eprintln!("rupu: skip {p}: {e}"),
        }
        registry.save_cache(p).await.ok();
    }
    Ok(())
}

async fn populate_live(
    registry: &ModelRegistry,
    resolver: &rupu_auth::resolver::KeychainResolver,
    provider: &str,
) -> anyhow::Result<usize> {
    use rupu_providers::provider::LlmProvider;
    use rupu_auth::CredentialResolver;
    let (_, creds) = resolver.get(provider, None).await?;
    let mut client: Box<dyn LlmProvider> = match provider {
        "anthropic" => Box::new(rupu_providers::AnthropicClient::new_from_creds(creds)?),
        "openai" => Box::new(rupu_providers::OpenAiCodexClient::new(creds, None)?),
        "gemini" => Box::new(rupu_providers::GoogleGeminiClient::new(creds, None)?),
        "copilot" => Box::new(rupu_providers::GithubCopilotClient::new(creds, None)?),
        _ => anyhow::bail!("unknown provider: {provider}"),
    };
    let models = client.list_models().await;
    let n = models.len();
    registry.set_live_cache(provider, models).await;
    Ok(n)
}

async fn build_registry() -> anyhow::Result<ModelRegistry> {
    let cache_dir = if let Ok(o) = std::env::var("RUPU_CACHE_DIR_OVERRIDE") {
        std::path::PathBuf::from(o)
    } else {
        crate::paths::global_dir()?.join("cache/models")
    };
    let registry = ModelRegistry::with_cache_dir(&cache_dir);
    // Load Copilot baked-in.
    registry.set_baked_in(
        "copilot",
        ["gpt-4o", "gpt-4o-mini", "claude-sonnet-4", "o4-mini"]
            .iter()
            .map(|id| rupu_providers::ModelInfo { id: (*id).into(), ..Default::default() })
            .collect(),
    ).await;
    // Load custom from config.toml.
    let cfg_path = crate::paths::global_dir()?.join("config.toml");
    if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path)?;
        let cfg: rupu_config::Config = toml::from_str(&text)?;
        for (name, pcfg) in &cfg.providers {
            if pcfg.models.is_empty() {
                continue;
            }
            registry.set_custom(
                name,
                pcfg.models
                    .iter()
                    .map(|m| rupu_providers::ModelInfo {
                        id: m.id.clone(),
                        context_window: m.context_window,
                        ..Default::default()
                    })
                    .collect(),
            ).await;
        }
    }
    // Load any persisted live caches.
    for p in &PROVIDERS {
        registry.load_cache(p).await.ok();
    }
    Ok(registry)
}
```

(`AnthropicClient::new_from_creds` may not exist — wrap an existing constructor that takes `AuthCredentials` similar to the others. Read `anthropic.rs` first; add a helper there if needed.)

- [ ] **Step 4: Register the subcommand in `cmd/mod.rs` and the main dispatch**

In `cmd/mod.rs` add `pub mod models;`. In `main.rs` (or wherever the top-level `Cli`/`Command` is), add `Models` variant and dispatch to `cmd::models::handle`.

- [ ] **Step 5: Run tests**

```
cargo test -p rupu-cli --test models_subcommand
```

Expected: at least the offline-Copilot listing test passes; `models refresh openai` test passes if `httpmock` reaches the test server.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/models.rs crates/rupu-cli/src/cmd/mod.rs crates/rupu-cli/src/main.rs crates/rupu-cli/tests/models_subcommand.rs
git commit -m "$(cat <<'EOF'
rupu-cli: add `rupu models list | refresh`

Spec §8a, §6b. List shows custom + live + baked-in entries with
their source. Refresh re-fetches each provider's /models endpoint
and writes the cache file.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Streaming flag

### Task 8: `--no-stream` flag on `rupu run`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`
- Modify: `crates/rupu-agent/src/runner.rs` (path through to provider call)
- Create: `crates/rupu-cli/tests/no_stream_flag.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/no_stream_flag.rs`:

```rust
use assert_cmd::Command;

#[test]
fn no_stream_flag_runs_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("hello.md"),
        "---\nname: hello\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nhi",
    ).unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{"text":"ok","stop":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}]"#,
        )
        .args(["run", "hello", "--prompt", "hi", "--no-stream"])
        .assert()
        .success();
}
```

- [ ] **Step 2: Add the flag**

In `cmd/run.rs`:

```rust
/// Skip token streaming; receive the full response at once.
#[arg(long)]
no_stream: bool,
```

Pass through to `run_agent(..., no_stream)`. In `runner.rs`, if `no_stream`, call `provider.send(...)` instead of `provider.stream(...)`.

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-cli --test no_stream_flag
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-agent/src/runner.rs crates/rupu-cli/tests/no_stream_flag.rs
git commit -m "$(cat <<'EOF'
rupu-cli: add --no-stream flag to `rupu run`

Spec §7a. Default is streaming (◐ working glyph behavior); --no-stream
calls provider.send() for one-shot debug/CI output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Documentation

### Task 9: README "Configuring providers" section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the section**

In `README.md`, after the install instructions, add:

```markdown
## Configuring providers

rupu supports four LLM providers; each works with API-key auth or SSO.

| Provider | API key                              | SSO                                 |
| -------- | ------------------------------------ | ----------------------------------- |
| anthropic | `console.anthropic.com` → API Keys   | Claude.ai login (browser callback)  |
| openai    | `platform.openai.com` → API Keys     | ChatGPT login (browser callback)    |
| gemini    | `aistudio.google.com` → Get API Key  | Google account (browser callback)   |
| copilot   | (PAT not supported for Copilot API)  | GitHub login (device code)          |

Quick start:

```bash
# API key
rupu auth login --provider anthropic --mode api-key --key sk-ant-...

# SSO
rupu auth login --provider anthropic --mode sso

# Verify
rupu auth status
```

See `docs/providers/<name>.md` for per-provider walkthroughs and
`docs/providers.md` for the full reference.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs: README adds 'Configuring providers' quick-start

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: `docs/providers.md` canonical reference

**Files:**
- Create: `docs/providers.md`

- [ ] **Step 1: Write the doc**

Create `docs/providers.md` with these sections (write actual content, not placeholders):

- **Overview** — one paragraph on the multi-provider model + auth modes.
- **Provider × auth-mode matrix** — full table with: provider name, auth modes supported, where to get credentials, known quirks (one-line each).
- **Auth flow walkthrough**:
  - API-key: `rupu auth login --provider X --mode api-key --key ...`
  - SSO browser callback: open browser, complete login, return to terminal
  - SSO device code: print code, visit URL, return
- **Configuration** — full `[providers.<name>]` schema reference: `base_url`, `org_id`, `region`, `timeout_ms`, `max_retries`, `max_concurrency`, `default_model`, `[[providers.X.models]]` array.
- **Troubleshooting**:
  - "rupu auth status shows configured but rupu run errors with Unauthorized" → token may be expired; `rupu auth login --mode sso` again.
  - "SSO login fails on a server" → use `--mode api-key`.
  - "Custom model rejected" → register via `[[providers.X.models]]` block.
- **Backlog / future** — pointer to `rupu usage`, local provider, Anthropic prompt-cache toggles.

(Total ~250 lines is reasonable. Write it inline.)

- [ ] **Step 2: Commit**

```bash
git add docs/providers.md
git commit -m "$(cat <<'EOF'
docs: add docs/providers.md canonical reference

Spec §11. Full provider × auth-mode matrix, config schema reference,
troubleshooting tree.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Per-provider walkthroughs

**Files:**
- Create: `docs/providers/anthropic.md`
- Create: `docs/providers/openai.md`
- Create: `docs/providers/gemini.md`
- Create: `docs/providers/copilot.md`

- [ ] **Step 1: Anthropic doc**

Create `docs/providers/anthropic.md` with:
- API-key acquisition: link to `console.anthropic.com/settings/keys`, format expectations (`sk-ant-...`).
- SSO walkthrough: command, what happens (browser, login, redirect, "you can close this tab"), expected `rupu auth status` output, expiry behavior.
- Example agent file:

```markdown
---
name: example-anthropic
description: Anthropic via API key.
provider: anthropic
auth: api-key
model: claude-sonnet-4-6
---

You are a coding assistant.
```

And the same with `auth: sso`.

- Refresh expectations: SSO tokens auto-refresh near expiry; refresh failure surfaces an actionable error.

- [ ] **Step 2: OpenAI doc**

Create `docs/providers/openai.md` covering:
- API key vs ChatGPT SSO endpoints (different base URLs).
- `org_id` configuration for org-scoped keys.
- Example agent.
- Note on Codex Responses API vs Chat Completions API (Slice B-1 uses Responses).

- [ ] **Step 3: Gemini doc**

Create `docs/providers/gemini.md`:
- Google AI Studio API key vs Vertex AI region.
- `region` field in config for Vertex users.
- Example agent.

- [ ] **Step 4: Copilot doc**

Create `docs/providers/copilot.md`:
- Subscription requirement (paid Copilot needed).
- Device-code flow walkthrough (mirrors `gh auth login` UX).
- Baked-in model list disclaimer; how to add custom entries via `[[providers.copilot.models]]`.
- Example agent.

- [ ] **Step 5: Commit**

```bash
git add docs/providers/
git commit -m "$(cat <<'EOF'
docs: per-provider walkthroughs

Spec §11. Step-by-step credential acquisition + example agent files
for each of the four providers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Live integration tests

### Task 12: Nightly live-integration test workflow

**Files:**
- Create: `.github/workflows/nightly-live-tests.yml`
- Create: `crates/rupu-providers/tests/live_smoke.rs`

- [ ] **Step 1: Write the gated tests**

Create `crates/rupu-providers/tests/live_smoke.rs`:

```rust
//! Live smoke tests. Skipped silently unless RUPU_LIVE_TESTS=1 AND
//! per-provider credentials are present in the env.

use rupu_providers::auth::AuthCredentials;
use rupu_providers::provider::LlmProvider;
use rupu_providers::types::{LlmRequest, Message};

fn live_enabled() -> bool {
    std::env::var("RUPU_LIVE_TESTS").as_deref() == Ok("1")
}

#[tokio::test]
async fn anthropic_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_ANTHROPIC_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let mut client = rupu_providers::AnthropicClient::new(key);
    let resp = client
        .send(&LlmRequest {
            model: "claude-haiku-4-5".into(),
            system: None,
            messages: vec![Message::user("Say hi.")],
            max_tokens: 64,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        })
        .await
        .expect("anthropic round-trip");
    assert!(!resp.content.is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn openai_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_OPENAI_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let creds = AuthCredentials::ApiKey { key };
    let mut client = rupu_providers::OpenAiCodexClient::new(creds, None).expect("init");
    let resp = client
        .send(&LlmRequest {
            model: "gpt-4o-mini".into(),
            system: None,
            messages: vec![Message::user("Say hi.")],
            max_tokens: 64,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        })
        .await
        .expect("openai round-trip");
    assert!(resp.usage.output_tokens > 0);
}

// (Same shape for gemini and copilot; omit for brevity in this plan
// step but include in implementation.)
```

- [ ] **Step 2: Workflow file**

Create `.github/workflows/nightly-live-tests.yml`:

```yaml
name: nightly-live-tests
on:
  schedule:
    - cron: "0 8 * * *"  # 08:00 UTC every day
  workflow_dispatch: {}

jobs:
  live:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    env:
      RUPU_LIVE_TESTS: "1"
      RUPU_LIVE_ANTHROPIC_KEY: ${{ secrets.RUPU_LIVE_ANTHROPIC_KEY }}
      RUPU_LIVE_OPENAI_KEY: ${{ secrets.RUPU_LIVE_OPENAI_KEY }}
      RUPU_LIVE_GEMINI_KEY: ${{ secrets.RUPU_LIVE_GEMINI_KEY }}
      RUPU_LIVE_COPILOT_TOKEN: ${{ secrets.RUPU_LIVE_COPILOT_TOKEN }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.88"
      - run: cargo test -p rupu-providers --test live_smoke -- --nocapture
```

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-providers/tests/live_smoke.rs .github/workflows/nightly-live-tests.yml
git commit -m "$(cat <<'EOF'
ci: nightly live-integration tests gated by RUPU_LIVE_TESTS

Spec §10d. Runs once daily at 08:00 UTC and on workflow_dispatch.
Per-provider env vars come from repo secrets; tests skip silently
when secrets are absent.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — CHANGELOG + final polish

### Task 13: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add the Slice B-1 entry**

Insert at the top of `CHANGELOG.md`:

```markdown
## v0.1.0 — Slice B-1: Multi-provider wiring (TBD release date)

### Added
- OpenAI, Gemini, GitHub Copilot provider adapters wired end-to-end.
- SSO authentication for all four providers:
  - Browser-callback (PKCE) for Anthropic, OpenAI, Gemini.
  - GitHub device-code for Copilot.
- `CredentialResolver` trait + `KeychainResolver` impl with
  refresh-on-expiry. Per-credential keychain entries
  (`rupu/<provider>/<api-key|sso>`).
- Default auth precedence: SSO wins when both modes configured.
- `rupu auth login --mode <api-key|sso>`.
- `rupu auth logout --provider X [--mode <m>]` and
  `rupu auth logout --all [--yes]`.
- `rupu auth status` two-column rendering (API-KEY / SSO).
- `rupu models list [--provider X]` — custom + live + baked-in.
- `rupu models refresh [--provider X]` — re-fetch /models.
- `[providers.<name>]` config block in `~/.rupu/config.toml`
  (base_url, org_id, region, timeout_ms, max_retries,
  max_concurrency, default_model, [[models]]).
- `Event::Usage { provider, model, input_tokens, output_tokens,
  cached_tokens }` written to JSONL transcripts.
- `rupu run` header (`agent: X  provider: Y/Z  model: M`) and
  footer (`Total: I input / O output tokens (C cached)`).
- `--no-stream` flag on `rupu run`.
- Documentation: `docs/providers.md` + four `docs/providers/<name>.md`.
- Nightly live-integration test workflow gated by RUPU_LIVE_TESTS.

### Changed
- `AgentSpec` now accepts optional `auth: <api-key|sso>` field.
- `provider_factory` consults `CredentialResolver` instead of
  `AuthBackend` directly.

### Backward-compatible
- Existing Slice A agent files (`provider: anthropic` only) load
  unchanged; missing `auth:` triggers the default-precedence path.
- Legacy keychain entries (Slice A `rupu/<provider>` shape) still
  read by the resolver as API-key on first lookup.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "$(cat <<'EOF'
CHANGELOG: Slice B-1 release entry

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: Workspace gates + release verification

- [ ] **Step 1: cargo fmt**

```
cargo fmt --all -- --check
```

- [ ] **Step 2: clippy**

```
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3: tests**

```
cargo test --workspace
```

- [ ] **Step 4: Manual smoke checklist**

Run by hand and confirm:

```bash
# All four providers parse, login (api-key), run a sample
rupu auth login --provider anthropic --mode api-key --key $A_KEY
rupu auth login --provider openai --mode api-key --key $O_KEY
rupu auth login --provider gemini --mode api-key --key $G_KEY
# (skip copilot if no subscription)
rupu auth status
rupu models list
rupu models list --provider openai
rupu models refresh
rupu run sample-anthropic --prompt "hi"
rupu run sample-openai --prompt "hi"
rupu run sample-gemini --prompt "hi"
rupu run sample-anthropic --prompt "hi" --no-stream
```

Expected outputs for each step documented in the spec §14 success criteria.

---

## Plan 3 success criteria

- `rupu models list` renders custom + live + baked-in entries with sources marked.
- `rupu models refresh` re-fetches `/models` for each provider where credentials are configured and writes the cache file at `~/.rupu/cache/models/<provider>.json`.
- Custom model entries in `~/.rupu/config.toml` `[[providers.X.models]]` blocks override live entries by ID.
- Unknown model name in an agent file produces the actionable error pointing at `rupu models list`.
- Anthropic responses populate `Usage.cached_tokens` from `cache_read_input_tokens`.
- Run footer shows `(N cached)` when caching benefited a run.
- `rupu run --no-stream` works end-to-end.
- README has the "Configuring providers" section.
- `docs/providers.md` and four `docs/providers/<name>.md` exist.
- Nightly workflow runs and skips silently when secrets are absent.
- `cargo fmt`, `cargo clippy`, `cargo test --workspace` all green.

## Slice B-1 release readiness

After Plan 3 lands, follow `docs/RELEASING.md` to cut `v0.1.0-cli` (or whatever version slug matches the bump). Smoke each binary on the platforms you build for; the live-integration suite running green for the previous night is the additional confidence signal that wasn't available for v0.0.3-cli.
