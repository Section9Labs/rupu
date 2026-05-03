# rupu Slice B-1 — Plan 1: Foundation & API-key wiring

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire OpenAI, Gemini, and Copilot provider adapters end-to-end with **API-key authentication only**, alongside the existing Anthropic adapter. Add the neutral types (`AuthMode`, refined `ProviderError`, `Usage` event), extend `AgentSpec` to accept the new `auth:` field, and per-provider concurrency / config plumbing. After this plan, a user with API keys for any of the four providers can run agents end-to-end.

**Architecture:** Most of the wire-level adapter code already exists (lifted from phi-cell): `OpenAiCodexClient`, `GoogleGeminiClient`, `GithubCopilotClient` are full implementations. Plan 1 adds the `LlmProvider` trait impls, builds them from API-key credentials in `provider_factory.rs`, and adds the foundational types the rest of B-1 will lean on. SSO flows are deferred to Plan 2; model resolution and polish to Plan 3.

**Tech Stack:** Rust 2021 (MSRV 1.88), `tokio`, `async-trait`, `reqwest`, `serde`, `thiserror`, `tracing`, `clap`. No new workspace deps.

**Spec:** `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`

---

## File Structure

```
crates/
  rupu-auth/
    src/
      backend.rs                  # MODIFY: add ProviderId::Gemini variant
  rupu-providers/
    src/
      auth_mode.rs                # NEW: AuthMode enum (ApiKey | Sso)
      classify.rs                 # NEW: classify_error() helpers per vendor
      concurrency.rs              # NEW: per-provider Semaphore registry
      error.rs                    # MODIFY: add structured variants per spec §4a
      lib.rs                      # MODIFY: pub mod the new modules + re-export
      openai_codex.rs             # MODIFY: add `impl LlmProvider for OpenAiCodexClient`
      google_gemini.rs            # MODIFY: add `impl LlmProvider for GoogleGeminiClient`
      github_copilot.rs           # MODIFY: add `impl LlmProvider for GithubCopilotClient`
    tests/
      classify.rs                 # NEW: pure-function error-classification tests
      lifecycle_provider.rs       # NEW: send/stream round-trip per adapter via mock HTTP
  rupu-transcript/
    src/
      event.rs                    # MODIFY: add Event::Usage variant
    tests/
      usage_event.rs              # NEW: serde round-trip for Usage event
  rupu-config/
    src/
      config.rs                   # MODIFY: add `providers: BTreeMap<String, ProviderConfig>`
      provider_config.rs          # NEW: ProviderConfig struct
    tests/
      provider_config.rs          # NEW: TOML parse + override
  rupu-agent/
    src/
      spec.rs                     # MODIFY: add optional `auth: Option<AuthMode>`
    tests/
      spec_auth_field.rs          # NEW: parse with/without auth field
  rupu-cli/
    src/
      provider_factory.rs         # MODIFY: wire openai/gemini/copilot for API-key
      cmd/
        run.rs                    # MODIFY: print run header + usage footer
        auth.rs                   # MODIFY: add gemini to login/logout/status loops
    tests/
      multi_provider_e2e.rs       # NEW: end-to-end via RUPU_MOCK_PROVIDER_SCRIPT
```

## Conventions to honor

- Workspace deps only — no version pins inside crate `Cargo.toml` files.
- `#![deny(clippy::all)]` at every crate root.
- `unsafe_code` forbidden.
- Tests use real I/O against `tempfile::TempDir`; no mocks except at the HTTP boundary (`httpmock`/`wiremock`-style).
- Error types: `thiserror` for libraries, `anyhow` for the CLI.
- Per the "no mock features" memory: every code path either does the real work or returns an explicit error. No silent `Ok(SilentNoOp)`.

## Important pre-existing state (read before starting)

- `rupu-providers::ProviderId` enum has variants `Anthropic`, `OpenaiCodex`, `GoogleGeminiCli`, `GoogleAntigravity`, `GithubCopilot` — used by the `LlmProvider::provider_id()` method.
- `rupu-auth::ProviderId` enum has variants `Anthropic`, `Openai`, `Copilot`, `Local` — used as the keychain entry username. **Missing `Gemini`** — Task 1 adds it.
- `rupu-providers::ProviderError` exists with variants `Http`, `Api`, `SseParse`, `Json`, `MissingAuth`, `UnexpectedEndOfStream`, `TokenRefreshFailed`, `AuthConfig`, `NotImplemented`. **The spec §4a calls for additional variants** (`RateLimited`, `Unauthorized`, `QuotaExceeded`, `ModelUnavailable`, `BadRequest`, `Transient`, `Other`) — Task 4 adds them as additive variants. Existing variants stay (Anthropic adapter and broker-client code uses them).
- `OpenAiCodexClient::new(creds: AuthCredentials, auth_json_path: Option<PathBuf>)` already accepts both `ApiKey` and `OAuth` shapes of `AuthCredentials`.
- `GoogleGeminiClient::new(...)` and `GithubCopilotClient::new(...)` exist; check their signatures inline before wiring.
- `AnthropicClient` already implements `LlmProvider` at `anthropic.rs:842` — use that as the template for the three new impls.

---

## Phase 0 — Foundation types

### Task 1: Add `Gemini` variant to `rupu_auth::ProviderId`

**Files:**
- Modify: `crates/rupu-auth/src/backend.rs:14-34`
- Test: existing `crates/rupu-auth/src/backend.rs` test module + new test below

- [ ] **Step 1: Write the failing test**

In `crates/rupu-auth/src/backend.rs`, add after the existing test module (or create one if absent):

```rust
#[cfg(test)]
mod gemini_id_tests {
    use super::*;

    #[test]
    fn provider_id_gemini_string_form() {
        assert_eq!(ProviderId::Gemini.as_str(), "gemini");
    }

    #[test]
    fn provider_id_gemini_serde_roundtrip() {
        let json = serde_json::to_string(&ProviderId::Gemini).unwrap();
        assert_eq!(json, "\"gemini\"");
        let parsed: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ProviderId::Gemini);
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-auth gemini_id_tests
```

Expected: compilation error `no variant or associated item named Gemini found for enum ProviderId`.

- [ ] **Step 3: Add the variant**

Modify `crates/rupu-auth/src/backend.rs` so the enum and `as_str()` look like:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Gemini,
    Copilot,
    Local,
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Local => "local",
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-auth
```

Expected: all green, including the new gemini tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth/src/backend.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add ProviderId::Gemini

Slice B-1 wires Gemini as a first-class provider; add the keychain
identifier alongside the existing Anthropic / Openai / Copilot / Local
variants. Stable as_str() form is "gemini".

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Create `AuthMode` enum in `rupu-providers`

**Files:**
- Create: `crates/rupu-providers/src/auth_mode.rs`
- Modify: `crates/rupu-providers/src/lib.rs`
- Test: `crates/rupu-providers/src/auth_mode.rs` (inline)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-providers/src/auth_mode.rs`:

```rust
//! Neutral auth-mode marker used across the runtime.
//!
//! Decouples the agent runtime and CLI from the on-the-wire shape of
//! `AuthCredentials`. The runtime only needs to know "is this an API
//! key or an SSO bearer?" for routing and rendering; the actual secret
//! lives behind the `CredentialResolver` (Plan 2) or the existing
//! `AuthBackend` (Plan 1).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    ApiKey,
    Sso,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::Sso => "sso",
        }
    }
}

impl fmt::Display for AuthMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AuthMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api-key" | "api_key" | "apikey" => Ok(Self::ApiKey),
            "sso" | "oauth" => Ok(Self::Sso),
            _ => Err(format!("unknown auth mode: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_as_str() {
        assert_eq!(AuthMode::ApiKey.to_string(), "api-key");
        assert_eq!(AuthMode::Sso.to_string(), "sso");
    }

    #[test]
    fn from_str_accepts_canonical_and_aliases() {
        assert_eq!(AuthMode::from_str("api-key").unwrap(), AuthMode::ApiKey);
        assert_eq!(AuthMode::from_str("api_key").unwrap(), AuthMode::ApiKey);
        assert_eq!(AuthMode::from_str("sso").unwrap(), AuthMode::Sso);
        assert_eq!(AuthMode::from_str("oauth").unwrap(), AuthMode::Sso);
        assert!(AuthMode::from_str("nope").is_err());
    }

    #[test]
    fn serde_roundtrip_kebab_case() {
        let json = serde_json::to_string(&AuthMode::ApiKey).unwrap();
        assert_eq!(json, "\"api-key\"");
        let parsed: AuthMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, AuthMode::ApiKey);
    }
}
```

- [ ] **Step 2: Run the tests to verify the file is wired**

```
cargo test -p rupu-providers --lib auth_mode::tests
```

Expected: compile error `file not found for module auth_mode` until Step 3.

- [ ] **Step 3: Wire the module in `lib.rs`**

Modify `crates/rupu-providers/src/lib.rs`. After `pub mod auth;` add:

```rust
pub mod auth_mode;
```

And after `pub use auth::{...};` add:

```rust
pub use auth_mode::AuthMode;
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-providers --lib auth_mode::tests
```

Expected: all three tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-providers/src/auth_mode.rs crates/rupu-providers/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-providers: add neutral AuthMode enum

The agent runtime needs to know whether a credential is API-key or
SSO-issued for routing and rendering, but should not depend on the
on-the-wire shape of AuthCredentials. AuthMode is the marker; the
secret stays behind CredentialResolver / AuthBackend.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Add structured `ProviderError` variants

**Files:**
- Modify: `crates/rupu-providers/src/error.rs`
- Test: `crates/rupu-providers/src/error.rs` inline test module

- [ ] **Step 1: Write the failing test**

Append at the bottom of `crates/rupu-providers/src/error.rs`:

```rust
#[cfg(test)]
mod structured_variants_tests {
    use super::*;
    use std::time::Duration;

    use crate::auth_mode::AuthMode;

    #[test]
    fn rate_limited_carries_retry_after() {
        let e = ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        };
        let s = e.to_string();
        assert!(s.contains("rate limited"), "got: {s}");
    }

    #[test]
    fn unauthorized_renders_provider_and_mode() {
        let e = ProviderError::Unauthorized {
            provider: "anthropic".into(),
            auth_mode: AuthMode::Sso,
            hint: "run rupu auth login --provider anthropic --mode sso".into(),
        };
        let s = e.to_string();
        assert!(s.contains("anthropic"));
        assert!(s.contains("sso"));
        assert!(s.contains("rupu auth login"));
    }

    #[test]
    fn quota_exceeded_names_provider() {
        let e = ProviderError::QuotaExceeded {
            provider: "openai".into(),
        };
        assert!(e.to_string().contains("openai"));
    }

    #[test]
    fn model_unavailable_names_model() {
        let e = ProviderError::ModelUnavailable {
            model: "gpt-5".into(),
        };
        assert!(e.to_string().contains("gpt-5"));
    }

    #[test]
    fn bad_request_includes_message() {
        let e = ProviderError::BadRequest {
            message: "max_tokens too large".into(),
        };
        assert!(e.to_string().contains("max_tokens too large"));
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-providers --lib structured_variants_tests
```

Expected: compile errors `no variant named RateLimited` etc.

- [ ] **Step 3: Add the new variants additively**

Modify `crates/rupu-providers/src/error.rs`. Replace the entire `ProviderError` enum block with the version below (preserving every existing variant and adding the new ones). Keep the existing `From<reqwest::Error>` and `From<serde_json::Error>` impls untouched at the bottom of the file.

```rust
use crate::auth_mode::AuthMode;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("SSE parse error: {0}")]
    SseParse(String),

    #[error("JSON deserialization error: {0}")]
    Json(String),

    #[error("missing auth for {provider}: set {env_hint} or provide auth.json")]
    MissingAuth { provider: String, env_hint: String },

    #[error("stream ended unexpectedly")]
    UnexpectedEndOfStream,

    #[error("token refresh failed: {0}")]
    TokenRefreshFailed(String),

    #[error("auth config error: {0}")]
    AuthConfig(String),

    #[error("provider {provider} is not yet implemented")]
    NotImplemented { provider: String },

    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("unauthorized: {provider} ({auth_mode}). {hint}")]
    Unauthorized {
        provider: String,
        auth_mode: AuthMode,
        hint: String,
    },

    #[error("quota exceeded for {provider}")]
    QuotaExceeded { provider: String },

    #[error("model unavailable: {model}")]
    ModelUnavailable { model: String },

    #[error("bad request: {message}")]
    BadRequest { message: String },

    #[error("transient error: {0}")]
    Transient(#[source] anyhow::Error),

    #[error("provider error: {0}")]
    Other(#[source] anyhow::Error),
}
```

Note: the existing `From<reqwest::Error>` and `From<serde_json::Error>` impls below the enum stay verbatim. Do not remove them — call sites in `anthropic.rs` etc. depend on them.

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-providers
```

Expected: all green. The existing tests must still pass — no behavior change to old variants.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-providers/src/error.rs
git commit -m "$(cat <<'EOF'
rupu-providers: add structured ProviderError variants

Adds the variants Slice B-1 spec §4a calls for (RateLimited,
Unauthorized, QuotaExceeded, ModelUnavailable, BadRequest, Transient,
Other) alongside the existing variants. Existing call sites are
unchanged; the new variants are wired in by the per-vendor
classify_error() helpers in a later task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Add `Event::Usage` to `rupu-transcript`

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs`
- Create: `crates/rupu-transcript/tests/usage_event.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-transcript/tests/usage_event.rs`:

```rust
use rupu_transcript::Event;

#[test]
fn usage_event_serde_roundtrip() {
    let e = Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        input_tokens: 1234,
        output_tokens: 567,
        cached_tokens: 890,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"type\":\"usage\""));
    assert!(json.contains("\"input_tokens\":1234"));
    let back: Event = serde_json::from_str(&json).unwrap();
    match back {
        Event::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model, "claude-sonnet-4-6");
            assert_eq!(input_tokens, 1234);
            assert_eq!(output_tokens, 567);
            assert_eq!(cached_tokens, 890);
        }
        _ => panic!("expected Event::Usage"),
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-transcript --test usage_event
```

Expected: compile error `no variant or associated item named Usage` — the variant doesn't exist yet.

- [ ] **Step 3: Add the variant**

Modify `crates/rupu-transcript/src/event.rs` to add the `Usage` variant in the `Event` enum. Locate the existing enum (it uses `#[serde(tag = "type", rename_all = "snake_case")]` or similar — confirm the attributes by reading the file first) and add:

```rust
Usage {
    provider: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cached_tokens: u32,
},
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-transcript
```

Expected: all green, including the new `usage_event` test.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-transcript/src/event.rs crates/rupu-transcript/tests/usage_event.rs
git commit -m "$(cat <<'EOF'
rupu-transcript: add Event::Usage variant

Captures per-response token telemetry (provider, model, input_tokens,
output_tokens, cached_tokens) in JSONL transcripts. Foundation for
Slice B-1's per-run usage footer and Slice D's `rupu usage` aggregator.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Add `ProviderConfig` to `rupu-config`

**Files:**
- Create: `crates/rupu-config/src/provider_config.rs`
- Modify: `crates/rupu-config/src/lib.rs`
- Modify: `crates/rupu-config/src/config.rs`
- Create: `crates/rupu-config/tests/provider_config.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-config/tests/provider_config.rs`:

```rust
use rupu_config::{Config, ProviderConfig};

#[test]
fn provider_config_parses_with_overrides() {
    let toml = r#"
[providers.anthropic]
base_url = "https://example-proxy.test"
timeout_ms = 60000
max_retries = 5
max_concurrency = 4
default_model = "claude-sonnet-4-6"

[providers.openai]
org_id = "org-abc123"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let anthro = cfg.providers.get("anthropic").expect("anthropic block");
    assert_eq!(anthro.base_url.as_deref(), Some("https://example-proxy.test"));
    assert_eq!(anthro.timeout_ms, Some(60000));
    assert_eq!(anthro.max_retries, Some(5));
    assert_eq!(anthro.max_concurrency, Some(4));
    assert_eq!(anthro.default_model.as_deref(), Some("claude-sonnet-4-6"));

    let openai = cfg.providers.get("openai").expect("openai block");
    assert_eq!(openai.org_id.as_deref(), Some("org-abc123"));
    assert_eq!(openai.base_url, None);

    // Unset block: default_model is None
    assert!(cfg.providers.get("gemini").is_none());
}

#[test]
fn provider_config_empty_when_unset() {
    let cfg: Config = toml::from_str("").expect("parse empty");
    assert!(cfg.providers.is_empty());
}

#[test]
fn provider_config_serialize_omits_none_fields() {
    let mut cfg = Config::default();
    cfg.providers.insert(
        "anthropic".into(),
        ProviderConfig {
            base_url: Some("https://x.test".into()),
            ..Default::default()
        },
    );
    let s = toml::to_string(&cfg).unwrap();
    assert!(s.contains("[providers.anthropic]"));
    assert!(s.contains("base_url = \"https://x.test\""));
    assert!(!s.contains("timeout_ms"));
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-config --test provider_config
```

Expected: compile error — `Config` has no `providers` field; `ProviderConfig` doesn't exist.

- [ ] **Step 3: Create `ProviderConfig`**

Create `crates/rupu-config/src/provider_config.rs`:

```rust
//! Per-provider runtime knobs. All fields optional — vendor defaults
//! apply when absent, per Slice B-1 spec §9a.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
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
}
```

- [ ] **Step 4: Add the field to `Config` and re-export**

Read `crates/rupu-config/src/config.rs` and `crates/rupu-config/src/lib.rs` first to confirm the existing `Config` struct shape. Then:

In `crates/rupu-config/src/config.rs`, add to the existing `Config` struct (preserving all existing fields and attrs):

```rust
use std::collections::BTreeMap;

use crate::provider_config::ProviderConfig;

// inside `pub struct Config { ... }`:
#[serde(default)]
pub providers: BTreeMap<String, ProviderConfig>,
```

In `crates/rupu-config/src/lib.rs` add:

```rust
pub mod provider_config;
pub use provider_config::ProviderConfig;
```

- [ ] **Step 5: Run the tests to verify they pass**

```
cargo test -p rupu-config
```

Expected: all green, including the new `provider_config` integration test.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-config/src/provider_config.rs \
        crates/rupu-config/src/config.rs \
        crates/rupu-config/src/lib.rs \
        crates/rupu-config/tests/provider_config.rs
git commit -m "$(cat <<'EOF'
rupu-config: add per-provider config block

Adds ProviderConfig with optional base_url, org_id, region, timeout_ms,
max_retries, max_concurrency, default_model — all fields optional so
the user only has to set what differs from vendor defaults.
Slice B-1 spec §9a.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Extend `AgentSpec` to accept `auth:` field

**Files:**
- Modify: `crates/rupu-agent/src/spec.rs`
- Create: `crates/rupu-agent/tests/spec_auth_field.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-agent/tests/spec_auth_field.rs`:

```rust
use rupu_agent::spec::AgentSpec;
use rupu_providers::AuthMode;

const WITH_AUTH: &str = "---
name: test
provider: anthropic
auth: sso
model: claude-sonnet-4-6
---
You are a test agent.";

const WITHOUT_AUTH: &str = "---
name: test
provider: anthropic
model: claude-sonnet-4-6
---
You are a test agent.";

const WITH_API_KEY_AUTH: &str = "---
name: test
provider: openai
auth: api-key
model: gpt-5
---
hi";

#[test]
fn parses_explicit_sso_auth() {
    let spec = AgentSpec::parse(WITH_AUTH).unwrap();
    assert_eq!(spec.auth, Some(AuthMode::Sso));
    assert_eq!(spec.provider.as_deref(), Some("anthropic"));
}

#[test]
fn parses_explicit_api_key_auth() {
    let spec = AgentSpec::parse(WITH_API_KEY_AUTH).unwrap();
    assert_eq!(spec.auth, Some(AuthMode::ApiKey));
}

#[test]
fn auth_field_optional_for_backwards_compat() {
    let spec = AgentSpec::parse(WITHOUT_AUTH).unwrap();
    assert_eq!(spec.auth, None);
    assert_eq!(spec.provider.as_deref(), Some("anthropic"));
}

#[test]
fn unknown_auth_value_is_a_parse_error() {
    let bad = "---
name: test
provider: anthropic
auth: bogus
model: claude-sonnet-4-6
---
hi";
    assert!(AgentSpec::parse(bad).is_err());
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-agent --test spec_auth_field
```

Expected: compile error — `AgentSpec` has no `auth` field; `rupu_providers::AuthMode` not imported.

- [ ] **Step 3: Add the field to the parser and the struct**

Modify `crates/rupu-agent/src/spec.rs`. At the top:

```rust
use rupu_providers::AuthMode;
```

In `Frontmatter`, add (under `provider`):

```rust
#[serde(default)]
auth: Option<AuthMode>,
```

In `AgentSpec`, add:

```rust
pub auth: Option<AuthMode>,
```

In the `parse()` function, propagate the field:

```rust
auth: fm.auth,
```

- [ ] **Step 4: Add `rupu-providers` to dev-deps if not already present**

Read `crates/rupu-agent/Cargo.toml` first. If `rupu-providers` is already a `[dependencies]` entry it's also visible to integration tests; no change needed. If absent, add it under `[dependencies]` with the workspace marker:

```toml
rupu-providers = { path = "../rupu-providers" }
```

- [ ] **Step 5: Run the tests to verify they pass**

```
cargo test -p rupu-agent
```

Expected: all four new tests pass; existing agent tests still green.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-agent/src/spec.rs crates/rupu-agent/tests/spec_auth_field.rs crates/rupu-agent/Cargo.toml
git commit -m "$(cat <<'EOF'
rupu-agent: add optional auth field to AgentSpec

Per Slice B-1 spec §4c: agents declare provider+auth+model. The auth
field is optional — when absent, the credential resolver applies the
default precedence (SSO > API-key) at run time. Backwards compatible:
existing agent files with only `provider:` continue to load.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Wire `LlmProvider` trait for the three new adapters

### Task 7: Implement `LlmProvider` for `OpenAiCodexClient`

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-providers/src/openai_codex.rs` test module (or create one at the bottom):

```rust
#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    #[test]
    fn implements_llm_provider_trait() {
        let creds = AuthCredentials::ApiKey { key: "sk-test".into() };
        let client = OpenAiCodexClient::new(creds, None).expect("new");
        // The trait object cast must succeed. If `OpenAiCodexClient`
        // does not impl `LlmProvider`, this fails to compile.
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::OpenaiCodex);
        assert!(!boxed.default_model().is_empty());
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-providers --lib openai_codex::llm_provider_impl_tests
```

Expected: compile error `the trait LlmProvider is not implemented for OpenAiCodexClient`.

- [ ] **Step 3: Add the trait impl**

At the bottom of `crates/rupu-providers/src/openai_codex.rs` (above any `#[cfg(test)]` module), add:

```rust
#[async_trait::async_trait]
impl crate::provider::LlmProvider for OpenAiCodexClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        OpenAiCodexClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        // Inherent stream uses `impl FnMut`; `&mut dyn FnMut` satisfies the bound.
        OpenAiCodexClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gpt-5"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::OpenaiCodex
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-providers --lib openai_codex
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-providers/src/openai_codex.rs
git commit -m "$(cat <<'EOF'
rupu-providers: impl LlmProvider for OpenAiCodexClient

Wires OpenAI into the polymorphic Box<dyn LlmProvider> machinery so
provider_factory can return it. Same shape as AnthropicClient's impl —
delegates to the inherent send/stream methods.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Implement `LlmProvider` for `GoogleGeminiClient`

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-providers/src/google_gemini.rs` (under any existing `#[cfg(test)]` module, or create one):

```rust
#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    #[test]
    fn implements_llm_provider_trait() {
        let creds = AuthCredentials::ApiKey { key: "test-key".into() };
        let client = GoogleGeminiClient::new(creds, None).expect("new");
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::GoogleGeminiCli);
        assert!(!boxed.default_model().is_empty());
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-providers --lib google_gemini::llm_provider_impl_tests
```

Expected: compile error `the trait LlmProvider is not implemented for GoogleGeminiClient`. If the test fails with a different error (e.g., `GoogleGeminiClient::new` has a different signature), read the file's `impl GoogleGeminiClient` to discover the actual constructor and adjust the test inputs to match. Then return to Step 2.

- [ ] **Step 3: Add the trait impl**

At the bottom of `crates/rupu-providers/src/google_gemini.rs` (above any `#[cfg(test)]` module):

```rust
#[async_trait::async_trait]
impl crate::provider::LlmProvider for GoogleGeminiClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gemini-2.5-pro"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::GoogleGeminiCli
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-providers --lib google_gemini
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-providers/src/google_gemini.rs
git commit -m "$(cat <<'EOF'
rupu-providers: impl LlmProvider for GoogleGeminiClient

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Implement `LlmProvider` for `GithubCopilotClient`

**Files:**
- Modify: `crates/rupu-providers/src/github_copilot.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-providers/src/github_copilot.rs`:

```rust
#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::auth::AuthCredentials;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    #[test]
    fn implements_llm_provider_trait() {
        let creds = AuthCredentials::ApiKey { key: "ghp_test".into() };
        let client = GithubCopilotClient::new(creds, None).expect("new");
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::GithubCopilot);
        assert!(!boxed.default_model().is_empty());
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-providers --lib github_copilot::llm_provider_impl_tests
```

Expected: compile error `the trait LlmProvider is not implemented`. If `GithubCopilotClient::new` has a different signature, read the impl block and adjust.

- [ ] **Step 3: Add the trait impl**

At the bottom of `crates/rupu-providers/src/github_copilot.rs`:

```rust
#[async_trait::async_trait]
impl crate::provider::LlmProvider for GithubCopilotClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        GithubCopilotClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        GithubCopilotClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gpt-4o"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        crate::provider_id::ProviderId::GithubCopilot
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-providers --lib github_copilot
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-providers/src/github_copilot.rs
git commit -m "$(cat <<'EOF'
rupu-providers: impl LlmProvider for GithubCopilotClient

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Per-provider concurrency

### Task 10: Add per-provider semaphore registry

**Files:**
- Create: `crates/rupu-providers/src/concurrency.rs`
- Modify: `crates/rupu-providers/src/lib.rs`
- Test: inline

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-providers/src/concurrency.rs`:

```rust
//! Per-provider concurrency limits.
//!
//! Slice B-1 spec §7c: each provider has its own Semaphore so that a
//! saturated rate-limit on one vendor doesn't drain capacity for the
//! others. Defaults are conservative; override via
//! `[providers.<name>].max_concurrency` in `~/.rupu/config.toml`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::Semaphore;

/// Default per-provider permits. Mirrors documented per-key rate limits.
pub fn default_permits(provider: &str) -> usize {
    match provider {
        "anthropic" => 4,
        "openai" => 8,
        "gemini" => 4,
        "copilot" => 4,
        _ => 4,
    }
}

/// Process-wide semaphore registry. Lazily initialized per provider.
static REGISTRY: OnceLock<std::sync::Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();

fn registry() -> &'static std::sync::Mutex<HashMap<String, Arc<Semaphore>>> {
    REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Look up (or create) the semaphore for `provider`. `permits_override`
/// applies only the first time the entry is created; subsequent calls
/// re-use the existing semaphore.
pub fn semaphore_for(provider: &str, permits_override: Option<usize>) -> Arc<Semaphore> {
    let mut map = registry().lock().expect("semaphore registry poisoned");
    map.entry(provider.to_string())
        .or_insert_with(|| {
            let permits = permits_override.unwrap_or_else(|| default_permits(provider));
            Arc::new(Semaphore::new(permits))
        })
        .clone()
}

#[cfg(test)]
#[allow(clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn default_permits_match_spec() {
        assert_eq!(default_permits("anthropic"), 4);
        assert_eq!(default_permits("openai"), 8);
        assert_eq!(default_permits("gemini"), 4);
        assert_eq!(default_permits("copilot"), 4);
        assert_eq!(default_permits("unknown"), 4);
    }

    #[tokio::test]
    async fn semaphore_for_returns_isolated_semaphores() {
        let a = semaphore_for("alpha-test", Some(2));
        let b = semaphore_for("beta-test", Some(2));
        let _g1 = a.clone().acquire_owned().await.unwrap();
        let _g2 = a.clone().acquire_owned().await.unwrap();
        // alpha at 0 permits; beta should still allow acquire.
        let _g3 = b.clone().acquire_owned().await.unwrap();
        assert_eq!(a.available_permits(), 0);
        assert_eq!(b.available_permits(), 1);
    }

    #[tokio::test]
    async fn semaphore_for_caches_first_call() {
        let a1 = semaphore_for("gamma-test", Some(3));
        let a2 = semaphore_for("gamma-test", Some(99));
        // Same Arc → same permits.
        assert_eq!(a1.available_permits(), 3);
        assert_eq!(a2.available_permits(), 3);
    }
}
```

- [ ] **Step 2: Wire the module**

Modify `crates/rupu-providers/src/lib.rs`. Add:

```rust
pub mod concurrency;
```

- [ ] **Step 3: Run the tests to verify they pass**

```
cargo test -p rupu-providers --lib concurrency
```

Expected: all three tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/src/concurrency.rs crates/rupu-providers/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-providers: add per-provider semaphore registry

One Arc<Semaphore> per provider, lazily created with sensible defaults
(Anthropic 4, OpenAI 8, Gemini 4, Copilot 4). Used by adapter call
sites to isolate rate-limit pressure across vendors. Slice B-1 §7c.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Error classification

### Task 11: Add `classify` module with per-vendor pure functions

**Files:**
- Create: `crates/rupu-providers/src/classify.rs`
- Modify: `crates/rupu-providers/src/lib.rs`
- Create: `crates/rupu-providers/tests/classify.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-providers/tests/classify.rs`:

```rust
use rupu_providers::classify::{classify_anthropic, classify_copilot, classify_gemini, classify_openai};
use rupu_providers::error::ProviderError;

#[test]
fn anthropic_429_is_rate_limited() {
    let e = classify_anthropic(429, "{}", None);
    assert!(matches!(e, ProviderError::RateLimited { .. }));
}

#[test]
fn anthropic_529_is_rate_limited_overloaded() {
    let e = classify_anthropic(529, "{}", None);
    assert!(matches!(e, ProviderError::RateLimited { .. }));
}

#[test]
fn anthropic_401_is_unauthorized() {
    let e = classify_anthropic(401, "{}", None);
    assert!(matches!(e, ProviderError::Unauthorized { .. }));
}

#[test]
fn openai_403_with_billing_message_is_quota() {
    let body = r#"{"error":{"type":"billing_hard_limit_reached"}}"#;
    let e = classify_openai(403, body, Some("billing_hard_limit_reached"));
    assert!(matches!(e, ProviderError::QuotaExceeded { .. }));
}

#[test]
fn openai_404_model_not_found() {
    let body = r#"{"error":{"type":"model_not_found"}}"#;
    let e = classify_openai(404, body, Some("model_not_found"));
    assert!(matches!(e, ProviderError::ModelUnavailable { .. }));
}

#[test]
fn openai_400_is_bad_request() {
    let e = classify_openai(400, r#"{"error":{"message":"max_tokens too large"}}"#, None);
    match e {
        ProviderError::BadRequest { message } => assert!(message.contains("max_tokens")),
        other => panic!("expected BadRequest, got {other:?}"),
    }
}

#[test]
fn gemini_503_is_transient() {
    let e = classify_gemini(503, "{}", None);
    assert!(matches!(e, ProviderError::Transient(_)));
}

#[test]
fn copilot_500_is_transient() {
    let e = classify_copilot(500, "{}", None);
    assert!(matches!(e, ProviderError::Transient(_)));
}

#[test]
fn unknown_status_falls_to_other() {
    let e = classify_anthropic(418, "I'm a teapot", None);
    assert!(matches!(e, ProviderError::Other(_)));
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-providers --test classify
```

Expected: compile error `unresolved import rupu_providers::classify`.

- [ ] **Step 3: Implement the classifier**

Create `crates/rupu-providers/src/classify.rs`:

```rust
//! Pure functions mapping vendor HTTP/error responses to ProviderError.
//!
//! Each adapter calls its corresponding `classify_*` at the boundary
//! between raw HTTP and the agent loop. Spec §7b. Pure for testability.

use std::time::Duration;

use crate::auth_mode::AuthMode;
use crate::error::ProviderError;

fn parse_retry_after(_body: &str) -> Option<Duration> {
    // Plan 1 keeps it simple — body parsing varies by vendor and isn't
    // observed by the tests. Plan 2 wires the actual Retry-After header
    // via the call site (which has access to reqwest::Response::headers).
    None
}

pub fn classify_anthropic(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "anthropic".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider anthropic".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 | 529 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "anthropic transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!(
            "anthropic {status}: {}",
            truncate(body, 200)
        )),
    }
}

pub fn classify_openai(status: u16, body: &str, vendor_code: Option<&str>) -> ProviderError {
    match (status, vendor_code) {
        (401, _) => ProviderError::Unauthorized {
            provider: "openai".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider openai".into(),
        },
        (403, Some(c)) if c.contains("billing") || c.contains("quota") => {
            ProviderError::QuotaExceeded {
                provider: "openai".into(),
            }
        }
        (404, Some("model_not_found")) | (404, _) => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        (400, _) => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        (429, _) => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        (500..=503, _) => ProviderError::Transient(anyhow::anyhow!(
            "openai transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!(
            "openai {status}: {}",
            truncate(body, 200)
        )),
    }
}

pub fn classify_gemini(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "gemini".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider gemini".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "gemini transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!("gemini {status}: {}", truncate(body, 200))),
    }
}

pub fn classify_copilot(status: u16, body: &str, _vendor_code: Option<&str>) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized {
            provider: "copilot".into(),
            auth_mode: AuthMode::ApiKey,
            hint: "run: rupu auth login --provider copilot".into(),
        },
        404 => ProviderError::ModelUnavailable {
            model: "(unknown)".into(),
        },
        400 => ProviderError::BadRequest {
            message: truncate(body, 500),
        },
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(body),
        },
        500..=503 => ProviderError::Transient(anyhow::anyhow!(
            "copilot transient {status}: {}",
            truncate(body, 200)
        )),
        _ => ProviderError::Other(anyhow::anyhow!("copilot {status}: {}", truncate(body, 200))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
```

- [ ] **Step 4: Wire the module**

Modify `crates/rupu-providers/src/lib.rs`. Add:

```rust
pub mod classify;
```

- [ ] **Step 5: Run the tests to verify they pass**

```
cargo test -p rupu-providers --test classify
```

Expected: all 9 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-providers/src/classify.rs crates/rupu-providers/src/lib.rs crates/rupu-providers/tests/classify.rs
git commit -m "$(cat <<'EOF'
rupu-providers: add per-vendor error classification

Pure functions mapping HTTP status + body + vendor code to the
structured ProviderError variants. Spec §7b. Each adapter call site
will use these at the boundary between raw HTTP and the agent loop.

Plan 1 lands the classifiers and tests; adapter call sites switch
over in their own commits as part of Plan 2 / Plan 3 polish.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Wire `provider_factory` for API-key flows

### Task 12: Build OpenAI from API key

**Files:**
- Modify: `crates/rupu-cli/src/provider_factory.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-cli/src/provider_factory.rs` (under existing `#[cfg(test)]` module, or create one):

```rust
#[cfg(test)]
mod build_openai_tests {
    use super::*;
    use rupu_auth::{AuthBackend, AuthError, ProviderId as AuthProviderId};

    struct FixedKeyBackend(&'static str);
    impl AuthBackend for FixedKeyBackend {
        fn store(&self, _: AuthProviderId, _: &str) -> Result<(), AuthError> { Ok(()) }
        fn retrieve(&self, p: AuthProviderId) -> Result<String, AuthError> {
            if p == AuthProviderId::Openai {
                Ok(self.0.to_string())
            } else {
                Err(AuthError::NotConfigured(p))
            }
        }
        fn forget(&self, _: AuthProviderId) -> Result<(), AuthError> { Ok(()) }
        fn name(&self) -> &'static str { "fixed-test" }
    }

    #[tokio::test]
    async fn build_openai_returns_provider() {
        let backend = FixedKeyBackend("sk-test-openai");
        let p = build_for_provider("openai", "gpt-5", &backend).await.expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::OpenaiCodex);
    }

    #[tokio::test]
    async fn build_openai_missing_credential_errors() {
        struct EmptyBackend;
        impl AuthBackend for EmptyBackend {
            fn store(&self, _: AuthProviderId, _: &str) -> Result<(), AuthError> { Ok(()) }
            fn retrieve(&self, p: AuthProviderId) -> Result<String, AuthError> {
                Err(AuthError::NotConfigured(p))
            }
            fn forget(&self, _: AuthProviderId) -> Result<(), AuthError> { Ok(()) }
            fn name(&self) -> &'static str { "empty" }
        }
        // Clear env var so the env fallback doesn't accidentally satisfy the request.
        let prev = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("OPENAI_API_KEY");
        let result = build_for_provider("openai", "gpt-5", &EmptyBackend).await;
        if let Some(p) = prev { std::env::set_var("OPENAI_API_KEY", p); }
        assert!(matches!(result, Err(FactoryError::MissingCredential { .. })));
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-cli --lib build_openai_tests
```

Expected: the OpenAI branch still hits `NotWiredInV0`; first test fails.

- [ ] **Step 3: Wire OpenAI in the factory**

Modify `crates/rupu-cli/src/provider_factory.rs`. Replace the `match name` block with:

```rust
match name {
    "anthropic" => build_anthropic(model, backend).await,
    "openai" | "openai_codex" | "codex" => build_openai(model, backend).await,
    "gemini" | "google_gemini" => build_gemini(model, backend).await,
    "copilot" | "github_copilot" => build_copilot(model, backend).await,
    "local" => Err(FactoryError::NotWiredInV0(name.to_string())),
    _ => Err(FactoryError::UnknownProvider(name.to_string())),
}
```

Then add the `build_openai` function (near `build_anthropic`):

```rust
async fn build_openai(
    model: &str,
    backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let _ = model;
    let api_key = match backend.retrieve(rupu_auth::ProviderId::Openai) {
        Ok(k) => k,
        Err(_) => std::env::var("OPENAI_API_KEY").map_err(|_| FactoryError::MissingCredential {
            provider: "openai".to_string(),
        })?,
    };
    let creds = rupu_providers::auth::AuthCredentials::ApiKey { key: api_key };
    let client = rupu_providers::openai_codex::OpenAiCodexClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("openai client init: {e}")))?;
    Ok(Box::new(client))
}
```

(Stub `build_gemini` and `build_copilot` for now — they're filled in Tasks 13 and 14, but the factory has to compile. Add temporary stubs that return `FactoryError::NotWiredInV0` for `gemini`/`copilot`. Tasks 13/14 replace the stubs.)

```rust
async fn build_gemini(
    _model: &str,
    _backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    Err(FactoryError::NotWiredInV0("gemini".to_string())) // wired in Task 13
}

async fn build_copilot(
    _model: &str,
    _backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    Err(FactoryError::NotWiredInV0("copilot".to_string())) // wired in Task 14
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-cli --lib build_openai_tests
```

Expected: both new tests pass; existing tests still green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/provider_factory.rs
git commit -m "$(cat <<'EOF'
rupu-cli: wire OpenAI in provider_factory

API-key flow: retrieve from backend (fall back to OPENAI_API_KEY env),
construct OpenAiCodexClient with AuthCredentials::ApiKey, return as
Box<dyn LlmProvider>. Gemini/Copilot still stubbed; wired in Tasks 13-14.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Build Gemini from API key

**Files:**
- Modify: `crates/rupu-cli/src/provider_factory.rs`

- [ ] **Step 1: Write the failing test**

Append to the same test module in `provider_factory.rs`:

```rust
#[cfg(test)]
mod build_gemini_tests {
    use super::*;
    use rupu_auth::{AuthBackend, AuthError, ProviderId as AuthProviderId};

    struct FixedKeyBackend;
    impl AuthBackend for FixedKeyBackend {
        fn store(&self, _: AuthProviderId, _: &str) -> Result<(), AuthError> { Ok(()) }
        fn retrieve(&self, p: AuthProviderId) -> Result<String, AuthError> {
            if p == AuthProviderId::Gemini {
                Ok("test-gemini-key".to_string())
            } else {
                Err(AuthError::NotConfigured(p))
            }
        }
        fn forget(&self, _: AuthProviderId) -> Result<(), AuthError> { Ok(()) }
        fn name(&self) -> &'static str { "fixed-test" }
    }

    #[tokio::test]
    async fn build_gemini_returns_provider() {
        let p = build_for_provider("gemini", "gemini-2.5-pro", &FixedKeyBackend).await.expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::GoogleGeminiCli);
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-cli --lib build_gemini_tests
```

Expected: test fails — `build_gemini` returns `NotWiredInV0`.

- [ ] **Step 3: Implement `build_gemini`**

Replace the stub `build_gemini` function in `provider_factory.rs` with:

```rust
async fn build_gemini(
    model: &str,
    backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let _ = model;
    let api_key = match backend.retrieve(rupu_auth::ProviderId::Gemini) {
        Ok(k) => k,
        Err(_) => std::env::var("GOOGLE_GEMINI_API_KEY").or_else(|_| std::env::var("GEMINI_API_KEY"))
            .map_err(|_| FactoryError::MissingCredential {
                provider: "gemini".to_string(),
            })?,
    };
    let creds = rupu_providers::auth::AuthCredentials::ApiKey { key: api_key };
    let client = rupu_providers::google_gemini::GoogleGeminiClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("gemini client init: {e}")))?;
    Ok(Box::new(client))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-cli --lib build_gemini_tests
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/provider_factory.rs
git commit -m "$(cat <<'EOF'
rupu-cli: wire Gemini in provider_factory

API-key flow with GOOGLE_GEMINI_API_KEY / GEMINI_API_KEY env fallback.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: Build Copilot from API key

**Files:**
- Modify: `crates/rupu-cli/src/provider_factory.rs`

- [ ] **Step 1: Write the failing test**

Append to the test module:

```rust
#[cfg(test)]
mod build_copilot_tests {
    use super::*;
    use rupu_auth::{AuthBackend, AuthError, ProviderId as AuthProviderId};

    struct FixedKeyBackend;
    impl AuthBackend for FixedKeyBackend {
        fn store(&self, _: AuthProviderId, _: &str) -> Result<(), AuthError> { Ok(()) }
        fn retrieve(&self, p: AuthProviderId) -> Result<String, AuthError> {
            if p == AuthProviderId::Copilot {
                Ok("ghp_test_copilot".to_string())
            } else {
                Err(AuthError::NotConfigured(p))
            }
        }
        fn forget(&self, _: AuthProviderId) -> Result<(), AuthError> { Ok(()) }
        fn name(&self) -> &'static str { "fixed-test" }
    }

    #[tokio::test]
    async fn build_copilot_returns_provider() {
        let p = build_for_provider("copilot", "gpt-4o", &FixedKeyBackend).await.expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::GithubCopilot);
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-cli --lib build_copilot_tests
```

Expected: fails — stub returns `NotWiredInV0`.

- [ ] **Step 3: Implement `build_copilot`**

Replace the stub `build_copilot`:

```rust
async fn build_copilot(
    model: &str,
    backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let _ = model;
    let api_key = match backend.retrieve(rupu_auth::ProviderId::Copilot) {
        Ok(k) => k,
        Err(_) => std::env::var("GITHUB_TOKEN").map_err(|_| FactoryError::MissingCredential {
            provider: "copilot".to_string(),
        })?,
    };
    let creds = rupu_providers::auth::AuthCredentials::ApiKey { key: api_key };
    let client = rupu_providers::github_copilot::GithubCopilotClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("copilot client init: {e}")))?;
    Ok(Box::new(client))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-cli --lib build_copilot_tests
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/provider_factory.rs
git commit -m "$(cat <<'EOF'
rupu-cli: wire Copilot in provider_factory

API-key flow with GITHUB_TOKEN env fallback. Note: Copilot also
supports a session-token shape (paid subscription via gh login) — that
goes through the SSO device-code path in Plan 2.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — CLI surface (login/logout/status for new providers)

### Task 15: Extend `rupu auth` to recognize gemini

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-cli/src/cmd/auth.rs` (in a new `#[cfg(test)]` module if absent):

```rust
#[cfg(test)]
mod parse_provider_tests {
    use super::*;

    #[test]
    fn recognizes_all_four_providers() {
        assert_eq!(parse_provider("anthropic").unwrap(), ProviderId::Anthropic);
        assert_eq!(parse_provider("openai").unwrap(), ProviderId::Openai);
        assert_eq!(parse_provider("gemini").unwrap(), ProviderId::Gemini);
        assert_eq!(parse_provider("copilot").unwrap(), ProviderId::Copilot);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_provider("typo").is_err());
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-cli --lib parse_provider_tests
```

Expected: fails — `parse_provider("gemini")` returns the unknown-provider error.

- [ ] **Step 3: Add `gemini` to `parse_provider` and to the `status()` iteration**

Modify `parse_provider` in `crates/rupu-cli/src/cmd/auth.rs`:

```rust
fn parse_provider(s: &str) -> anyhow::Result<ProviderId> {
    match s {
        "anthropic" => Ok(ProviderId::Anthropic),
        "openai" => Ok(ProviderId::Openai),
        "gemini" => Ok(ProviderId::Gemini),
        "copilot" => Ok(ProviderId::Copilot),
        "local" => Ok(ProviderId::Local),
        _ => Err(anyhow::anyhow!("unknown provider: {s}")),
    }
}
```

In `status()`, add `ProviderId::Gemini` to the iteration array (after `Openai` for stable ordering):

```rust
for p in [
    ProviderId::Anthropic,
    ProviderId::Openai,
    ProviderId::Gemini,
    ProviderId::Copilot,
    ProviderId::Local,
] { /* existing body */ }
```

In the `Login` clap doc-comment, replace `"Provider name (anthropic | openai | copilot | local)."` with `"Provider name (anthropic | openai | gemini | copilot | local)."`.

- [ ] **Step 4: Run the tests to verify they pass**

```
cargo test -p rupu-cli
```

Expected: all green.

- [ ] **Step 5: Manual smoke**

```
cargo run -q -p rupu-cli -- auth status
```

Expected output: backend line + 5 rows with `gemini` listed `-` (not configured). No error.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/auth.rs
git commit -m "$(cat <<'EOF'
rupu-cli: surface gemini in `rupu auth login|logout|status`

Plan 1 keeps the current single-column rendering; Plan 2 splits into
the API-KEY / SSO two-column layout from spec §8b.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Run header & footer

### Task 16: Print run header (`provider/auth model`) before first model output

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`

- [ ] **Step 1: Read current `cmd/run.rs`**

Read `crates/rupu-cli/src/cmd/run.rs` end-to-end before editing. Find where the agent is launched (probably a call to `run_agent` or similar). The header line must print exactly once, immediately before the agent loop begins streaming.

- [ ] **Step 2: Write the integration test**

Create `crates/rupu-cli/tests/run_header.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

#[test]
fn run_header_prints_provider_model_line() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join("hello.md");
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: hello").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
    drop(f);

    // Mock provider script: one assistant turn that ends.
    let script = r#"[{"text":"hi","stop":"end_turn"}]"#;

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .args(["run", "hello", "--prompt", "say hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent: hello"))
        .stdout(predicate::str::contains("provider: anthropic"))
        .stdout(predicate::str::contains("model: claude-sonnet-4-6"));
}
```

- [ ] **Step 3: Run the test and verify it fails**

```
cargo test -p rupu-cli --test run_header
```

Expected: assertions on `stdout` fail because the header is not yet printed.

- [ ] **Step 4: Implement the header**

Modify `crates/rupu-cli/src/cmd/run.rs`. Just before the agent-loop call, add a print-line. The auth-mode portion is `?` for now — Plan 2 fills it in once `CredentialResolver::get` returns the actual mode used. Use the `agent.auth.map(|m| m.to_string()).unwrap_or_else(|| "?".into())` pattern:

```rust
println!(
    "agent: {}  provider: {}/{}  model: {}",
    agent.name,
    agent.provider.as_deref().unwrap_or("anthropic"),
    agent.auth.map(|m| m.to_string()).unwrap_or_else(|| "?".into()),
    agent.model.as_deref().unwrap_or("(default)"),
);
```

(Adjust the field names to match the actual `AgentSpec` access pattern in this file. The general shape is what matters.)

- [ ] **Step 5: Run the tests to verify they pass**

```
cargo test -p rupu-cli --test run_header
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/tests/run_header.rs
git commit -m "$(cat <<'EOF'
rupu-cli: print run header (agent / provider/auth / model)

Slice B-1 spec §8d. The auth field shows '?' until Plan 2 wires the
CredentialResolver and can report the actual mode used. Sample agents
without explicit `auth:` get the default-precedence mode at run time.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 17: Emit `Event::Usage` and print run footer

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`
- Modify: `crates/rupu-agent/src/runner.rs` (emit Usage event after each provider response)
- Modify: `crates/rupu-cli/tests/run_header.rs` (rename to `run_header_footer.rs` or extend)

- [ ] **Step 1: Read current runner**

Read `crates/rupu-agent/src/runner.rs` to find where the provider returns `LlmResponse` and the next turn begins. The accumulator pattern probably has a single point where `response.usage` is available.

- [ ] **Step 2: Write the failing footer test**

Append to `crates/rupu-cli/tests/run_header.rs`:

```rust
#[test]
fn run_footer_prints_token_totals() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let agent_path = agent_dir.join("hello.md");
    let mut f = std::fs::File::create(&agent_path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: hello").unwrap();
    writeln!(f, "provider: anthropic").unwrap();
    writeln!(f, "model: claude-sonnet-4-6").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
    drop(f);

    let script =
        r#"[{"text":"hi","stop":"end_turn","usage":{"input_tokens":42,"output_tokens":7}}]"#;

    assert_cmd::Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .args(["run", "hello", "--prompt", "say hi"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Total: 42 input"))
        .stdout(predicates::str::contains("7 output"));
}
```

- [ ] **Step 3: Run the test and verify it fails**

```
cargo test -p rupu-cli --test run_header
```

Expected: footer assertions fail.

- [ ] **Step 4: Wire the runner to emit `Event::Usage`**

In `crates/rupu-agent/src/runner.rs`, locate the loop where a provider response arrives and an action protocol step is emitted. After the response is in hand, emit:

```rust
transcript.write(&rupu_transcript::Event::Usage {
    provider: agent.provider.clone().unwrap_or_else(|| "anthropic".into()),
    model: response.model.clone(),
    input_tokens: response.usage.input_tokens,
    output_tokens: response.usage.output_tokens,
    cached_tokens: 0, // populated for Anthropic prompt-cache responses in Plan 3
})?;
```

(The exact form depends on the runner's transcript handle. The existing event-write call sites are the template.)

Also accumulate totals into the runner's return value or a local sum that the CLI can read. Simplest path: add a `RunSummary { input_tokens, output_tokens, cached_tokens }` returned from `run_agent`.

- [ ] **Step 5: Print the footer in `cmd/run.rs`**

After the agent loop returns:

```rust
println!(
    "Total: {} input / {} output tokens{}",
    summary.input_tokens,
    summary.output_tokens,
    if summary.cached_tokens > 0 {
        format!(" ({} cached)", summary.cached_tokens)
    } else {
        String::new()
    }
);
```

- [ ] **Step 6: Run the tests to verify they pass**

```
cargo test -p rupu-cli --test run_header
```

Expected: both header and footer assertions pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-agent/src/runner.rs crates/rupu-cli/tests/run_header.rs
git commit -m "$(cat <<'EOF'
rupu-cli: emit Event::Usage and print per-run token footer

Per-response Usage events go to the JSONL transcript; the CLI sums
them and prints `Total: X input / Y output tokens` at the end of the
run. cached_tokens is reserved (always 0 in Plan 1; Anthropic
prompt-cache integration lands in Plan 3).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — End-to-end via mock-provider seam

### Task 18: Multi-provider end-to-end test

**Files:**
- Create: `crates/rupu-cli/tests/multi_provider_e2e.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/multi_provider_e2e.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn write_agent(dir: &std::path::Path, name: &str, provider: &str, model: &str) {
    let agent_dir = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let path = agent_dir.join(format!("{name}.md"));
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "name: {name}").unwrap();
    writeln!(f, "provider: {provider}").unwrap();
    writeln!(f, "model: {model}").unwrap();
    writeln!(f, "---").unwrap();
    writeln!(f, "You are a hello-world agent.").unwrap();
}

const SCRIPT: &str =
    r#"[{"text":"hi from mock","stop":"end_turn","usage":{"input_tokens":11,"output_tokens":3}}]"#;

#[test]
fn run_against_anthropic_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "anthropic", "claude-sonnet-4-6");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .args(["run", "hello", "--prompt", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: anthropic"))
        .stdout(predicate::str::contains("Total: 11 input"));
}

#[test]
fn run_against_openai_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "openai", "gpt-5");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .args(["run", "hello", "--prompt", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: openai"));
}

#[test]
fn run_against_gemini_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "gemini", "gemini-2.5-pro");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .args(["run", "hello", "--prompt", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: gemini"));
}

#[test]
fn run_against_copilot_via_mock_seam() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "hello", "copilot", "gpt-4o");
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", SCRIPT)
        .args(["run", "hello", "--prompt", "hi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: copilot"));
}
```

- [ ] **Step 2: Run the tests to verify they pass**

```
cargo test -p rupu-cli --test multi_provider_e2e
```

Expected: all four pass — the mock seam bypasses real provider construction, so the test exercises CLI dispatch + agent runner + transcript wiring + run header / footer for every provider name.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cli/tests/multi_provider_e2e.rs
git commit -m "$(cat <<'EOF'
rupu-cli: end-to-end mock-seam test for all four providers

Verifies CLI dispatch, agent loop, transcript Usage events, run header
and footer all wire correctly for anthropic / openai / gemini /
copilot. The mock-provider env-var seam means no real API keys needed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Workspace check + sample agents

### Task 19: Sample agent files for each provider

**Files:**
- Create: `.rupu/agents/sample-openai.md`
- Create: `.rupu/agents/sample-gemini.md`
- Create: `.rupu/agents/sample-copilot.md`

- [ ] **Step 1: Create sample agents**

Create `.rupu/agents/sample-openai.md`:

```markdown
---
name: sample-openai
description: Demonstrates OpenAI provider with API-key auth.
provider: openai
auth: api-key
model: gpt-5
---

You are a coding assistant powered by OpenAI's GPT-5. Be concise and direct.
```

Create `.rupu/agents/sample-gemini.md`:

```markdown
---
name: sample-gemini
description: Demonstrates Gemini provider with API-key auth.
provider: gemini
auth: api-key
model: gemini-2.5-pro
---

You are a coding assistant powered by Google's Gemini 2.5 Pro. Be concise and direct.
```

Create `.rupu/agents/sample-copilot.md`:

```markdown
---
name: sample-copilot
description: Demonstrates GitHub Copilot provider.
provider: copilot
auth: api-key
model: gpt-4o
---

You are a coding assistant powered by GitHub Copilot. Be concise and direct.
```

- [ ] **Step 2: Smoke that they parse**

```
cargo run -q -p rupu-cli -- agent list
```

Expected: all three new agents appear in the listing.

- [ ] **Step 3: Commit**

```bash
git add .rupu/agents/sample-openai.md .rupu/agents/sample-gemini.md .rupu/agents/sample-copilot.md
git commit -m "$(cat <<'EOF'
samples: add provider-specific sample agents

Lets users see the multi-provider frontmatter in action and gives the
project-discovery code paths real samples to load when developers run
rupu inside the rupu checkout.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 20: Workspace gates

**Files:** none

- [ ] **Step 1: cargo fmt**

```
cargo fmt --all -- --check
```

Expected: exits 0. If not, run `cargo fmt --all` and commit the result.

- [ ] **Step 2: clippy**

```
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no warnings or errors.

- [ ] **Step 3: full test run**

```
cargo test --workspace
```

Expected: all green.

- [ ] **Step 4: commit any formatting fixups (if needed)**

If Step 1 produced changes:

```bash
git add -A
git commit -m "$(cat <<'EOF'
chore: cargo fmt

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 21: Update CLAUDE.md to point to Plan 1

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the "Read first" pointers**

In `CLAUDE.md`, replace the "Read first" bullets with the Slice B-1 spec and Plan 1 paths:

```markdown
## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Plan 1 (foundation & API-key wiring, in progress): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-1-foundation-and-api-key.md`
- Plan 2 (SSO flows): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-2-sso-and-resolver.md`
- Plan 3 (model resolution & polish): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-3-models-and-polish.md`
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
CLAUDE.md: point to Slice B-1 plans

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Plan 1 success criteria

After all 21 tasks complete, the following must hold:

- `cargo fmt --all -- --check` exits 0.
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo test --workspace` exits 0.
- `rupu auth login --provider <anthropic|openai|gemini|copilot> --key sk-...` stores the API key in the keychain and reports the backend used.
- `rupu auth status` shows all four providers (plus local), with `configured` / `-` per row.
- `rupu run sample-openai --prompt "hi"` (with `OPENAI_API_KEY` env or stored credential) executes against the real OpenAI endpoint and prints the run header + footer. Same for gemini and copilot. `RUPU_LIVE_TESTS` is NOT used — those land in Plan 3.
- `rupu run sample-anthropic --prompt "hi"` continues to work unchanged from Slice A.
- The mock-seam test (`multi_provider_e2e.rs`) passes for all four providers.
- `Event::Usage` events appear in `~/.rupu/transcripts/<run_id>.jsonl` for every model response.

## Out of scope (deferred)

- SSO browser-callback flow (Anthropic/OpenAI/Gemini) — Plan 2.
- GitHub device-code flow (Copilot) — Plan 2.
- `CredentialResolver` trait + `KeychainResolver` impl — Plan 2.
- `auth status` two-column rendering (API-KEY × SSO) — Plan 2.
- Live `/models` fetching, `rupu models list/refresh` — Plan 3.
- Custom model registration in `~/.rupu/config.toml` — Plan 3.
- Streaming polish (`--no-stream` flag) — already supported by adapters; CLI flag in Plan 3.
- Documentation (`docs/providers/*.md`) — Plan 3.
- `RUPU_LIVE_TESTS=1` integration tests — Plan 3.
