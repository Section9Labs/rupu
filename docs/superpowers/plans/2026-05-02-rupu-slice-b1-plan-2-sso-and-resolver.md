# rupu Slice B-1 — Plan 2: SSO flows & CredentialResolver

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add SSO authentication for all four providers — browser-callback OAuth (PKCE) for Anthropic, OpenAI, Gemini; GitHub device-code for Copilot. Build the `CredentialResolver` trait + `KeychainResolver` impl with refresh-on-expiry. Update `rupu auth login`/`logout`/`status` for the multi-credential semantics (per-mode storage, two-column status rendering, `--mode`/`--all` flags). Default precedence: SSO > API-key when no `auth:` field on the agent.

**Architecture:** OAuth flows live in `rupu-auth` as feature-flagged modules: `oauth_callback` (PKCE browser-callback) and `oauth_device` (GitHub device-code). The `CredentialResolver` sits between the agent runtime and `rupu-providers`; agents now call `resolver.get(provider, hint)` instead of asking the `AuthBackend` directly. The resolver decides which credential to use, refreshes if needed, and returns the actual `AuthMode` used (so the run header prints the correct mode).

**Tech Stack:** Rust 2021 (MSRV 1.88), `tokio`, `reqwest`, `serde`, `thiserror`. New deps: `tiny_http` (~0.12; localhost callback listener), `webbrowser` (~1; cross-platform browser launch with graceful error), `rand` (PKCE verifier), `sha2` (PKCE S256 challenge), `base64` (already in workspace). Optional: `httpmock` for token-endpoint integration tests.

**Spec:** `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`

**Prereq:** Plan 1 must be merged.

---

## File Structure

```
crates/
  rupu-auth/
    Cargo.toml                    # MODIFY: add tiny_http, webbrowser, rand, sha2 deps
    src/
      backend.rs                  # MODIFY: keychain entry naming -> rupu/<provider>/<mode>
      keychain_layout.rs          # NEW: keychain key construction (provider × mode)
      stored.rs                   # NEW: StoredCredential struct (creds + refresh_token)
      resolver.rs                 # NEW: CredentialResolver trait + KeychainResolver
      in_memory.rs                # NEW: InMemoryResolver for tests
      oauth/
        mod.rs                    # NEW: shared types (PkcePair, OAuthClient, etc.)
        callback.rs               # NEW: PKCE browser-callback flow (Anthropic/OpenAI/Gemini)
        device.rs                 # NEW: GitHub device-code flow (Copilot)
        providers.rs              # NEW: per-provider OAuth config (URLs, client_ids, scopes)
      lib.rs                      # MODIFY: pub mod oauth, pub use Resolver / Stored
    tests/
      oauth_callback.rs           # NEW: PKCE flow with httpmock'd token endpoint
      oauth_device.rs             # NEW: device-code flow with httpmock'd github
      resolver_default_pref.rs    # NEW: SSO > API-key precedence
      resolver_refresh.rs         # NEW: expired bearer triggers refresh
  rupu-providers/
    src/
      auth_mode.rs                # MODIFY: add Credentials struct + helpers
  rupu-cli/
    src/
      cmd/
        auth.rs                   # MODIFY: --mode, two-column status, --all logout
      paths.rs                    # MODIFY: helper for resolver bootstrap
      provider_factory.rs         # MODIFY: take resolver instead of bare AuthBackend
    tests/
      auth_login_modes.rs         # NEW: login api-key vs sso flag flows
      auth_status_table.rs        # NEW: two-column rendering
```

## Important pre-existing state

- Plan 1 added `AuthMode` (`api-key` | `sso`) and the structured `ProviderError` variants.
- `rupu-providers::auth::AuthCredentials` already serializes as `{"type":"api_key", ...}` / `{"type":"oauth", ...}`. The `KeychainResolver` reads/writes this same JSON shape so the existing adapters keep working.
- `keyring` workspace dep already has `apple-native`, `windows-native`, `sync-secret-service`, `vendored` features (Slice A bug fix). Verify still set in root `Cargo.toml` before starting.
- `rupu-auth::ProviderId` (added Gemini in Plan 1 Task 1) is the keychain identifier.
- The existing `cmd/auth.rs::login()` does a synchronous `read_to_string(stdin)` for the key. Plan 2's `--mode sso` path takes a different code path entirely.

---

## Phase 0 — Workspace deps

### Task 1: Add OAuth deps to workspace

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rupu-auth/Cargo.toml`

- [ ] **Step 1: Add deps to workspace `[workspace.dependencies]`**

In root `Cargo.toml`, in `[workspace.dependencies]`, add:

```toml
tiny_http = "0.12"
webbrowser = "1"
rand = "0.8"
sha2 = "0.10"
url = "2"
httpmock = "0.7"
```

- [ ] **Step 2: Wire them into `rupu-auth`**

In `crates/rupu-auth/Cargo.toml`, under `[dependencies]`:

```toml
tiny_http = { workspace = true }
webbrowser = { workspace = true }
rand = { workspace = true }
sha2 = { workspace = true }
base64 = { workspace = true }
url = { workspace = true }
reqwest = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }
chrono = { workspace = true }
tracing = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
rupu-providers = { path = "../rupu-providers" }

[dev-dependencies]
httpmock = { workspace = true }
tempfile = { workspace = true }
```

(Confirm exact entries by reading the existing file first; only add what's missing.)

- [ ] **Step 3: Verify workspace builds**

```
cargo build --workspace
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/rupu-auth/Cargo.toml
git commit -m "$(cat <<'EOF'
deps: add OAuth/PKCE crates to workspace and rupu-auth

tiny_http for the localhost callback listener; webbrowser for
cross-platform launch; rand + sha2 for PKCE verifier+challenge; url
for redirect-URI parsing; httpmock for testing the token endpoints.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Keychain layout & StoredCredential

### Task 2: Multi-mode keychain entry naming

**Files:**
- Create: `crates/rupu-auth/src/keychain_layout.rs`
- Modify: `crates/rupu-auth/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/src/keychain_layout.rs`:

```rust
//! Construct the (service, account) tuple used as the keychain key.
//!
//! Slice B-1 spec §9b: each provider × mode is its own entry, e.g.
//! `rupu/anthropic/api-key` and `rupu/anthropic/sso`. The legacy Slice
//! A layout used `rupu` as the service and the provider name as the
//! account. We keep that for backwards compat at read time but write
//! the new shape going forward.

use rupu_providers::AuthMode;

use crate::backend::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainKey {
    pub service: String,
    pub account: String,
}

pub fn key_for(provider: ProviderId, mode: AuthMode) -> KeychainKey {
    KeychainKey {
        service: "rupu".into(),
        account: format!("{}/{}", provider.as_str(), mode.as_str()),
    }
}

/// Legacy single-mode key from Slice A; only used for read-side
/// compatibility (treat any value found here as API-key for migration).
pub fn legacy_key_for(provider: ProviderId) -> KeychainKey {
    KeychainKey {
        service: "rupu".into(),
        account: provider.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_for_separates_modes() {
        let api = key_for(ProviderId::Anthropic, AuthMode::ApiKey);
        let sso = key_for(ProviderId::Anthropic, AuthMode::Sso);
        assert_ne!(api.account, sso.account);
        assert_eq!(api.account, "anthropic/api-key");
        assert_eq!(sso.account, "anthropic/sso");
    }

    #[test]
    fn legacy_key_keeps_old_shape() {
        let k = legacy_key_for(ProviderId::Openai);
        assert_eq!(k.account, "openai");
    }
}
```

- [ ] **Step 2: Wire and run**

In `crates/rupu-auth/src/lib.rs`:

```rust
pub mod keychain_layout;
pub use keychain_layout::{key_for, legacy_key_for, KeychainKey};
```

Run:

```
cargo test -p rupu-auth --lib keychain_layout
```

Expected: green.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-auth/src/keychain_layout.rs crates/rupu-auth/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add per-mode keychain key layout

`rupu/<provider>/<mode>` for new entries; legacy `rupu/<provider>` is
read-only fallback so Slice A users don't have to re-login on upgrade.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `StoredCredential` struct + JSON serialization

**Files:**
- Create: `crates/rupu-auth/src/stored.rs`
- Modify: `crates/rupu-auth/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/src/stored.rs`:

```rust
//! What lives inside a single keychain entry.
//!
//! Slice B-1 spec §4b: provider adapters never see the refresh token.
//! `StoredCredential` is what the resolver writes to the keychain; the
//! resolver materializes a `Credentials` for adapters from this.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rupu_providers::auth::AuthCredentials;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredential {
    /// The on-the-wire creds the provider adapter consumes.
    pub credentials: AuthCredentials,
    /// Refresh token, if SSO. None for API-key entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token expires (UTC). None means non-expiring (API key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl StoredCredential {
    pub fn api_key(key: impl Into<String>) -> Self {
        Self {
            credentials: AuthCredentials::ApiKey { key: key.into() },
            refresh_token: None,
            expires_at: None,
        }
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            Some(exp) => exp <= now,
            None => false,
        }
    }

    pub fn is_near_expiry(&self, now: DateTime<Utc>, buffer_secs: i64) -> bool {
        match self.expires_at {
            Some(exp) => (exp - chrono::Duration::seconds(buffer_secs)) <= now,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_constructor_has_no_refresh_or_expiry() {
        let s = StoredCredential::api_key("sk-test");
        assert!(s.refresh_token.is_none());
        assert!(s.expires_at.is_none());
        assert!(matches!(s.credentials, AuthCredentials::ApiKey { .. }));
    }

    #[test]
    fn json_roundtrip() {
        let s = StoredCredential::api_key("sk-test");
        let json = serde_json::to_string(&s).unwrap();
        let back: StoredCredential = serde_json::from_str(&json).unwrap();
        match back.credentials {
            AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-test"),
            _ => panic!(),
        }
    }

    #[test]
    fn near_expiry_window_correct() {
        let exp = Utc::now() + chrono::Duration::seconds(30);
        let s = StoredCredential {
            credentials: AuthCredentials::ApiKey { key: "x".into() },
            refresh_token: None,
            expires_at: Some(exp),
        };
        // 60-second buffer means 30s-from-now is "near".
        assert!(s.is_near_expiry(Utc::now(), 60));
        // 10-second buffer means 30s-from-now is NOT near.
        assert!(!s.is_near_expiry(Utc::now(), 10));
    }
}
```

- [ ] **Step 2: Wire and run**

In `crates/rupu-auth/src/lib.rs`:

```rust
pub mod stored;
pub use stored::StoredCredential;
```

Run:

```
cargo test -p rupu-auth --lib stored
```

Expected: green.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-auth/src/stored.rs crates/rupu-auth/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add StoredCredential

The resolver-private wrapper around AuthCredentials that adds a
refresh token and expiry timestamp. Provider adapters never see this.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Per-provider OAuth config

### Task 4: Per-provider OAuth metadata

**Files:**
- Create: `crates/rupu-auth/src/oauth/mod.rs`
- Create: `crates/rupu-auth/src/oauth/providers.rs`
- Modify: `crates/rupu-auth/src/lib.rs`

- [ ] **Step 1: Module skeleton**

Create `crates/rupu-auth/src/oauth/mod.rs`:

```rust
//! OAuth flows for Slice B-1 SSO. Two shapes:
//!
//! - `callback`: PKCE browser-redirect flow (Anthropic, OpenAI, Gemini).
//! - `device`: device-code polling flow (GitHub Copilot).

pub mod callback;
pub mod device;
pub mod providers;

pub use providers::{provider_oauth, ProviderOAuth, OAuthFlow};
```

Create `crates/rupu-auth/src/oauth/providers.rs`:

```rust
//! Per-provider OAuth metadata. All values are public client IDs (not
//! secrets); they're embedded in the rupu binary the same way `gh`
//! embeds its client ID.
//!
//! IMPORTANT: client IDs are vendor-controlled and may change. Validate
//! during smoke tests; if a vendor rotates a client ID, ship a patch.

use crate::backend::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthFlow {
    /// Browser-callback PKCE flow.
    Callback,
    /// Device-code polling flow.
    Device,
}

#[derive(Debug, Clone)]
pub struct ProviderOAuth {
    pub flow: OAuthFlow,
    pub client_id: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub device_url: Option<&'static str>,   // device-code only
    pub scopes: &'static [&'static str],
    pub redirect_path: &'static str,        // local listener path, e.g. "/callback"
}

pub fn provider_oauth(p: ProviderId) -> Option<ProviderOAuth> {
    match p {
        ProviderId::Anthropic => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            // Anthropic's official OAuth client_id; verify before each release.
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            authorize_url: "https://claude.ai/oauth/authorize",
            token_url: "https://console.anthropic.com/v1/oauth/token",
            device_url: None,
            scopes: &["org:create_api_key", "user:profile", "user:inference"],
            redirect_path: "/callback",
        }),
        ProviderId::Openai => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            authorize_url: "https://auth.openai.com/oauth/authorize",
            token_url: "https://auth.openai.com/oauth/token",
            device_url: None,
            scopes: &["openid", "profile", "email", "offline_access"],
            redirect_path: "/callback",
        }),
        ProviderId::Gemini => Some(ProviderOAuth {
            flow: OAuthFlow::Callback,
            client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            device_url: None,
            scopes: &["https://www.googleapis.com/auth/cloud-platform", "openid", "email"],
            redirect_path: "/callback",
        }),
        ProviderId::Copilot => Some(ProviderOAuth {
            flow: OAuthFlow::Device,
            client_id: "Iv1.b507a08c87ecfe98",   // GitHub Copilot's public client_id
            authorize_url: "",                   // unused for device flow
            token_url: "https://github.com/login/oauth/access_token",
            device_url: Some("https://github.com/login/device/code"),
            scopes: &["read:user"],
            redirect_path: "",
        }),
        ProviderId::Local => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_supported_provider_has_metadata() {
        for p in [
            ProviderId::Anthropic,
            ProviderId::Openai,
            ProviderId::Gemini,
            ProviderId::Copilot,
        ] {
            let cfg = provider_oauth(p).unwrap_or_else(|| panic!("missing oauth config for {p}"));
            assert!(!cfg.client_id.is_empty(), "{p}: empty client_id");
        }
    }

    #[test]
    fn local_has_no_oauth() {
        assert!(provider_oauth(ProviderId::Local).is_none());
    }

    #[test]
    fn copilot_uses_device_flow() {
        let c = provider_oauth(ProviderId::Copilot).unwrap();
        assert_eq!(c.flow, OAuthFlow::Device);
        assert!(c.device_url.is_some());
    }

    #[test]
    fn callback_providers_have_no_device_url() {
        for p in [ProviderId::Anthropic, ProviderId::Openai, ProviderId::Gemini] {
            let c = provider_oauth(p).unwrap();
            assert_eq!(c.flow, OAuthFlow::Callback);
            assert!(c.device_url.is_none());
        }
    }
}
```

Skeleton stubs for `callback.rs` and `device.rs` so the module compiles:

Create `crates/rupu-auth/src/oauth/callback.rs`:

```rust
//! PKCE browser-callback OAuth flow. Implementation lands in Tasks 5-6.

use crate::stored::StoredCredential;
use crate::backend::ProviderId;
use anyhow::Result;

pub async fn run(_provider: ProviderId) -> Result<StoredCredential> {
    anyhow::bail!("oauth_callback::run not yet implemented")
}
```

Create `crates/rupu-auth/src/oauth/device.rs`:

```rust
//! GitHub device-code OAuth flow. Implementation lands in Task 7.

use crate::stored::StoredCredential;
use crate::backend::ProviderId;
use anyhow::Result;

pub async fn run(_provider: ProviderId) -> Result<StoredCredential> {
    anyhow::bail!("oauth_device::run not yet implemented")
}
```

In `crates/rupu-auth/src/lib.rs`:

```rust
pub mod oauth;
```

- [ ] **Step 2: Run tests**

```
cargo test -p rupu-auth --lib oauth::providers
```

Expected: green.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-auth/src/oauth/ crates/rupu-auth/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-auth: per-provider OAuth metadata

Centralizes vendor client_ids, authorize/token URLs, scopes, and the
flow shape (Callback vs Device). Stubs for the actual flows land in
Tasks 5-7.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — PKCE browser-callback flow

### Task 5: PKCE verifier/challenge generator

**Files:**
- Create: `crates/rupu-auth/src/oauth/pkce.rs`
- Modify: `crates/rupu-auth/src/oauth/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/src/oauth/pkce.rs`:

```rust
//! PKCE (RFC 7636) helpers — base64url-no-pad encoded random verifier
//! plus its S256 challenge.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
    pub method: &'static str,
}

impl PkcePair {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
        Self {
            verifier,
            challenge,
            method: "S256",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_valid_lengths() {
        let p = PkcePair::generate();
        assert!(p.verifier.len() >= 43, "verifier too short: {}", p.verifier.len());
        assert!(p.verifier.len() <= 128, "verifier too long");
        assert_eq!(p.challenge.len(), 43); // 32 bytes -> 43 char base64url
        assert_eq!(p.method, "S256");
    }

    #[test]
    fn challenge_matches_verifier_hash() {
        let p = PkcePair::generate();
        let mut h = Sha256::new();
        h.update(p.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(h.finalize());
        assert_eq!(p.challenge, expected);
    }

    #[test]
    fn each_call_is_unique() {
        let a = PkcePair::generate();
        let b = PkcePair::generate();
        assert_ne!(a.verifier, b.verifier);
    }
}
```

- [ ] **Step 2: Wire and run**

In `crates/rupu-auth/src/oauth/mod.rs` add `pub mod pkce;`.

```
cargo test -p rupu-auth --lib oauth::pkce
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-auth/src/oauth/pkce.rs crates/rupu-auth/src/oauth/mod.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add PKCE verifier/challenge helper

Standard RFC 7636 S256: 32 random bytes -> base64url verifier;
SHA-256 of the verifier -> base64url challenge. Used by the browser
callback flow in Task 6.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: PKCE browser-callback flow

**Files:**
- Modify: `crates/rupu-auth/src/oauth/callback.rs`
- Create: `crates/rupu-auth/tests/oauth_callback.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-auth/tests/oauth_callback.rs`:

```rust
//! Test the callback flow against an httpmock'd token endpoint. We
//! drive the listener directly (no real browser) by extracting the
//! redirect URL constructor and POSTing to the callback ourselves.

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::callback;

#[tokio::test]
async fn callback_completes_with_mocked_token_endpoint() {
    let server = MockServer::start();
    // Mock token endpoint
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200).header("content-type", "application/json").json_body(serde_json::json!({
            "access_token": "test-access",
            "refresh_token": "test-refresh",
            "expires_in": 3600,
            "token_type": "bearer",
        }));
    });

    // Run with overridden token URL via env (or test hook — see callback.rs).
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));
    std::env::set_var("RUPU_OAUTH_SKIP_BROWSER", "1");

    let port = portpicker::pick_unused_port().unwrap();
    std::env::set_var("RUPU_OAUTH_FORCE_PORT", port.to_string());

    // Spawn the flow; immediately POST to the callback URL.
    let flow_handle = tokio::spawn(async move {
        callback::run(ProviderId::Anthropic).await
    });
    // Wait briefly for the listener to bind.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    // Hit the callback. The flow validates state — read the
    // generated state from RUPU_OAUTH_LAST_STATE (test seam).
    let state = std::env::var("RUPU_OAUTH_LAST_STATE").expect("test seam not set by flow");
    let url = format!("http://127.0.0.1:{port}/callback?code=stub-code&state={state}");
    let _ = reqwest::get(&url).await.unwrap();
    let stored = flow_handle.await.unwrap().expect("flow ok");

    token_mock.assert();
    assert!(stored.refresh_token.is_some());
    assert!(stored.expires_at.is_some());
    std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_OAUTH_SKIP_BROWSER");
    std::env::remove_var("RUPU_OAUTH_FORCE_PORT");
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
}
```

(`portpicker` is a tiny dev-dep; alternative: bind to `127.0.0.1:0` and read the chosen port from the listener — see Step 2 implementation. Add `portpicker = "0.1"` to root `Cargo.toml` workspace deps and `[dev-dependencies]` of rupu-auth, OR use the bind-to-0 pattern directly. Prefer the bind-to-0 pattern — no extra dep.)

If avoiding the `portpicker` dep, replace its use with reading the port from the listener after bind. Adjust the test: instead of forcing a port, read it from a small file written by the flow under a test env var (`RUPU_OAUTH_PORT_FILE`).

- [ ] **Step 2: Run the test and verify it fails**

```
cargo test -p rupu-auth --test oauth_callback
```

Expected: compile error or panic from `callback::run` returning `bail!`.

- [ ] **Step 3: Implement the callback flow**

Replace `crates/rupu-auth/src/oauth/callback.rs` entirely:

```rust
//! PKCE browser-callback OAuth flow. Anthropic, OpenAI, Gemini.
//!
//! 1. Generate PKCE pair + state nonce.
//! 2. Bind localhost listener (port 0 -> OS picks).
//! 3. Open browser to authorize URL with redirect_uri pointing at us.
//! 4. Receive redirect; validate state; exchange code at token URL.
//! 5. Return StoredCredential.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rand::RngCore;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;
use tracing::{debug, info};

use crate::backend::ProviderId;
use crate::oauth::pkce::PkcePair;
use crate::oauth::providers::{provider_oauth, OAuthFlow};
use crate::stored::StoredCredential;

const CALLBACK_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    let oauth = provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Callback {
        anyhow::bail!("provider {provider} does not use the callback flow");
    }

    // Headless detection: error early on Linux without DISPLAY/BROWSER.
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("BROWSER").is_none()
        && std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_none()
    {
        anyhow::bail!(
            "SSO requires a desktop browser. \
             Run with --mode api-key for headless setups."
        );
    }

    let pkce = PkcePair::generate();
    let state = random_state();

    // For tests, expose the state so the test driver can craft the redirect.
    if std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_some() {
        std::env::set_var("RUPU_OAUTH_LAST_STATE", &state);
    }

    // Bind the listener.
    let port = std::env::var("RUPU_OAUTH_FORCE_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let server = tiny_http::Server::http(format!("127.0.0.1:{port}"))
        .map_err(|e| anyhow!("bind localhost listener: {e}"))?;
    let bound_port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
        _ => return Err(anyhow!("listener bound to unexpected address type")),
    };
    let redirect_uri = format!("http://127.0.0.1:{bound_port}{}", oauth.redirect_path);

    // Build authorize URL.
    let authorize = build_authorize_url(&oauth, &pkce.challenge, &state, &redirect_uri)?;
    if std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_none() {
        info!("opening browser to {}", authorize);
        if webbrowser::open(&authorize).is_err() {
            eprintln!(
                "Could not open a browser automatically. \
                 Open this URL in a browser to continue:\n\n  {authorize}\n"
            );
        }
    } else {
        debug!("RUPU_OAUTH_SKIP_BROWSER set; not launching browser");
    }

    // Wait for the redirect.
    let server = Arc::new(server);
    let server2 = server.clone();
    let recv = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
        loop {
            let req = server2
                .recv_timeout(Duration::from_secs(CALLBACK_TIMEOUT_SECS))
                .map_err(|e| anyhow!("listener recv error: {e}"))?
                .ok_or_else(|| anyhow!("oauth callback timed out"))?;
            let url = req.url().to_string();
            if !url.starts_with(&format!("{}?", oauth.redirect_path))
                && !url.starts_with(oauth.redirect_path)
            {
                let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
                continue;
            }
            let parsed = url::Url::parse(&format!("http://localhost{}", url))
                .map_err(|e| anyhow!("parse callback url: {e}"))?;
            let code = parsed
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(|| anyhow!("no `code` in redirect"))?;
            let got_state = parsed
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(|| anyhow!("no `state` in redirect"))?;
            let resp = tiny_http::Response::from_string(
                "<html><body>Authentication complete — return to your terminal.</body></html>",
            )
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap(),
            );
            let _ = req.respond(resp);
            return Ok((code, got_state));
        }
    });
    let (code, got_state) = recv.await??;
    if got_state != state {
        anyhow::bail!("state mismatch: possible replay or CSRF attempt");
    }

    // Exchange the code.
    let token_url = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.token_url.to_string());
    let client = reqwest::Client::new();
    let body = [
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("client_id", oauth.client_id),
        ("redirect_uri", &redirect_uri),
        ("code_verifier", &pkce.verifier),
    ];
    let token: TokenResponse = client
        .post(&token_url)
        .form(&body)
        .send()
        .await
        .context("token exchange request")?
        .error_for_status()
        .context("token exchange status")?
        .json()
        .await
        .context("token exchange json")?;

    let expires_at = token
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s));

    Ok(StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: token.access_token,
            refresh: token.refresh_token.clone().unwrap_or_default(),
            expires: expires_at.map(|d| d.timestamp_millis() as u64).unwrap_or(0),
            extra: Default::default(),
        },
        refresh_token: token.refresh_token,
        expires_at,
    })
}

fn build_authorize_url(
    oauth: &crate::oauth::providers::ProviderOAuth,
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String> {
    let mut url = url::Url::parse(oauth.authorize_url)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", oauth.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &oauth.scopes.join(" "))
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}
```

- [ ] **Step 4: Run the integration test**

```
cargo test -p rupu-auth --test oauth_callback
```

Expected: green. If `tiny_http::ListenAddr` isn't available in this version, switch to extracting `server.server_addr()` via the `.to_ip()` helper. Adjust if needed; the test stays unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth/src/oauth/callback.rs crates/rupu-auth/tests/oauth_callback.rs
git commit -m "$(cat <<'EOF'
rupu-auth: implement PKCE browser-callback OAuth flow

Spec §5b: bind 127.0.0.1:0, open browser, validate state, exchange
code at the provider's token endpoint. Headless detection on Linux
without DISPLAY/BROWSER. Test seams (RUPU_OAUTH_SKIP_BROWSER,
RUPU_OAUTH_TOKEN_URL_OVERRIDE, RUPU_OAUTH_LAST_STATE) keep the
integration test offline.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Device-code flow (Copilot)

### Task 7: GitHub device-code flow

**Files:**
- Modify: `crates/rupu-auth/src/oauth/device.rs`
- Create: `crates/rupu-auth/tests/oauth_device.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-auth/tests/oauth_device.rs`:

```rust
use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::device;

#[tokio::test]
async fn device_code_flow_completes() {
    let server = MockServer::start();
    let device_mock = server.mock(|when, then| {
        when.method(POST).path("/login/device/code");
        then.status(200).header("content-type", "application/json").json_body(serde_json::json!({
            "device_code": "DEV-CODE",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 1,
        }));
    });
    // First poll: pending. Second: success.
    let mut count = 0u32;
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/login/oauth/access_token");
        then.status(200).header("content-type", "application/json").json_body(serde_json::json!({
            "access_token": "ghp-test",
            "token_type": "bearer",
            "scope": "read:user",
        }));
        // (httpmock doesn't trivially do "Nth response" — for this test, return success first.)
        let _ = &mut count;
    });

    std::env::set_var("RUPU_DEVICE_DEVICE_URL_OVERRIDE", server.url("/login/device/code"));
    std::env::set_var("RUPU_DEVICE_TOKEN_URL_OVERRIDE", server.url("/login/oauth/access_token"));
    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let stored = device::run(ProviderId::Copilot).await.expect("flow ok");
    device_mock.assert();
    token_mock.assert();
    match stored.credentials {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "ghp-test");
        }
        _ => panic!("expected OAuth"),
    }
    std::env::remove_var("RUPU_DEVICE_DEVICE_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");
}
```

- [ ] **Step 2: Implement the device flow**

Replace `crates/rupu-auth/src/oauth/device.rs`:

```rust
//! GitHub device-code OAuth flow. Used for Copilot.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;

use crate::backend::ProviderId;
use crate::oauth::providers::{provider_oauth, OAuthFlow};
use crate::stored::StoredCredential;

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AccessTokenResponse {
    Success {
        access_token: String,
        #[serde(default)]
        token_type: Option<String>,
        #[serde(default)]
        scope: Option<String>,
    },
    Pending {
        error: String,
        #[serde(default)]
        interval: Option<u64>,
    },
}

pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    let oauth = provider_oauth(provider)
        .ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Device {
        anyhow::bail!("provider {provider} does not use the device flow");
    }

    let device_url = std::env::var("RUPU_DEVICE_DEVICE_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.device_url.unwrap_or_default().to_string());
    let token_url = std::env::var("RUPU_DEVICE_TOKEN_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.token_url.to_string());

    let client = reqwest::Client::new();
    let dc: DeviceCodeResponse = client
        .post(&device_url)
        .header("Accept", "application/json")
        .form(&[("client_id", oauth.client_id), ("scope", &oauth.scopes.join(" "))])
        .send()
        .await
        .context("device-code request")?
        .error_for_status()
        .context("device-code status")?
        .json()
        .await
        .context("device-code json")?;

    println!(
        "Visit {} and enter code: {}",
        dc.verification_uri, dc.user_code
    );

    let mut interval = dc.interval.unwrap_or(5);
    if std::env::var_os("RUPU_DEVICE_FAST_POLL").is_some() {
        interval = 0; // poll as fast as the test wants
    }
    let deadline = Utc::now() + chrono::Duration::seconds(dc.expires_in.unwrap_or(900) as i64);

    loop {
        if interval > 0 {
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
        if Utc::now() >= deadline {
            anyhow::bail!("device-code flow timed out");
        }
        let resp: AccessTokenResponse = client
            .post(&token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", oauth.client_id),
                ("device_code", dc.device_code.as_str()),
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code",
                ),
            ])
            .send()
            .await
            .context("device-code poll request")?
            .json()
            .await
            .context("device-code poll json")?;
        match resp {
            AccessTokenResponse::Success { access_token, .. } => {
                return Ok(StoredCredential {
                    credentials: AuthCredentials::OAuth {
                        access: access_token.clone(),
                        refresh: String::new(),
                        expires: 0,
                        extra: Default::default(),
                    },
                    refresh_token: Some(access_token),
                    expires_at: None, // GitHub PATs don't include explicit expiry in this flow
                });
            }
            AccessTokenResponse::Pending { error, interval: i } => {
                if error == "authorization_pending" || error == "slow_down" {
                    if let Some(new_interval) = i {
                        if std::env::var_os("RUPU_DEVICE_FAST_POLL").is_none() {
                            interval = new_interval;
                        }
                    }
                    continue;
                }
                anyhow::bail!("device-code flow error: {error}");
            }
        }
    }
}
```

- [ ] **Step 3: Run the integration test**

```
cargo test -p rupu-auth --test oauth_device
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-auth/src/oauth/device.rs crates/rupu-auth/tests/oauth_device.rs
git commit -m "$(cat <<'EOF'
rupu-auth: implement GitHub device-code OAuth flow (Copilot)

Spec §5c. Prints user_code + verification_uri, polls the access-token
endpoint until success or expiry. Test seams keep the integration
test offline.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — CredentialResolver

### Task 8: `CredentialResolver` trait + `KeychainResolver`

**Files:**
- Create: `crates/rupu-auth/src/resolver.rs`
- Create: `crates/rupu-auth/src/in_memory.rs`
- Modify: `crates/rupu-auth/src/lib.rs`
- Create: `crates/rupu-auth/tests/resolver_default_pref.rs`
- Create: `crates/rupu-auth/tests/resolver_refresh.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-auth/tests/resolver_default_pref.rs`:

```rust
use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::resolver::CredentialResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn sso_wins_when_both_present_and_no_hint() {
    let r = InMemoryResolver::new();
    r.put(ProviderId::Anthropic, AuthMode::ApiKey, StoredCredential::api_key("sk-test")).await;
    r.put(
        ProviderId::Anthropic,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "tok".into(), refresh: "rt".into(), expires: 0, extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        },
    ).await;
    let (mode, _) = r.get("anthropic", None).await.unwrap();
    assert_eq!(mode, AuthMode::Sso);
}

#[tokio::test]
async fn api_key_used_when_only_api_key_present() {
    let r = InMemoryResolver::new();
    r.put(ProviderId::Openai, AuthMode::ApiKey, StoredCredential::api_key("sk-test")).await;
    let (mode, _) = r.get("openai", None).await.unwrap();
    assert_eq!(mode, AuthMode::ApiKey);
}

#[tokio::test]
async fn explicit_hint_overrides_precedence() {
    let r = InMemoryResolver::new();
    r.put(ProviderId::Openai, AuthMode::ApiKey, StoredCredential::api_key("sk-test")).await;
    r.put(
        ProviderId::Openai,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "tok".into(), refresh: "rt".into(), expires: 0, extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        },
    ).await;
    let (mode, _) = r.get("openai", Some(AuthMode::ApiKey)).await.unwrap();
    assert_eq!(mode, AuthMode::ApiKey);
}

#[tokio::test]
async fn missing_credential_errors() {
    let r = InMemoryResolver::new();
    let result = r.get("gemini", None).await;
    assert!(result.is_err());
}
```

Create `crates/rupu-auth/tests/resolver_refresh.rs`:

```rust
use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::resolver::CredentialResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn near_expiry_triggers_refresh_callback() {
    let r = InMemoryResolver::new();
    let near = chrono::Utc::now() + chrono::Duration::seconds(10);
    r.put(
        ProviderId::Anthropic,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "old".into(), refresh: "rt".into(), expires: 0, extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(near),
        },
    ).await;
    r.set_refresh_callback(|_p, mode, sc| {
        assert_eq!(mode, AuthMode::Sso);
        assert!(sc.refresh_token.is_some());
        Ok(StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "new".into(),
                refresh: "rt".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        })
    }).await;
    let (mode, creds) = r.get("anthropic", None).await.unwrap();
    assert_eq!(mode, AuthMode::Sso);
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "new", "refresh should have replaced the access token");
        }
        _ => panic!(),
    }
}
```

- [ ] **Step 2: Implement the resolver trait**

Create `crates/rupu-auth/src/resolver.rs`:

```rust
//! CredentialResolver: the runtime's single point of truth for "which
//! credential should this provider call use right now?"

use anyhow::Result;
use async_trait::async_trait;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;

use crate::stored::StoredCredential;

/// Buffer (seconds) before expiry at which we proactively refresh.
pub const EXPIRY_REFRESH_BUFFER_SECS: i64 = 60;

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Resolve credentials for `provider`. `hint` may force a specific
    /// auth mode; if None, applies SSO > API-key precedence.
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)>;

    /// Force-refresh credentials. Used when an adapter sees a 401 mid-request.
    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials>;
}
```

Create `crates/rupu-auth/src/in_memory.rs`:

```rust
//! In-process resolver for tests. Holds `StoredCredential`s in a
//! `RwLock<HashMap>` and lets tests inject a refresh callback.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::RwLock;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;

use crate::backend::ProviderId;
use crate::resolver::{CredentialResolver, EXPIRY_REFRESH_BUFFER_SECS};
use crate::stored::StoredCredential;

type RefreshFn = dyn Fn(ProviderId, AuthMode, &StoredCredential) -> Result<StoredCredential>
    + Send
    + Sync;

#[derive(Default)]
pub struct InMemoryResolver {
    inner: Arc<RwLock<HashMap<(ProviderId, AuthMode), StoredCredential>>>,
    refresh_cb: Arc<RwLock<Option<Box<RefreshFn>>>>,
}

impl InMemoryResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn put(&self, p: ProviderId, mode: AuthMode, sc: StoredCredential) {
        let mut g = self.inner.write().await;
        g.insert((p, mode), sc);
    }

    pub async fn set_refresh_callback<F>(&self, f: F)
    where
        F: Fn(ProviderId, AuthMode, &StoredCredential) -> Result<StoredCredential>
            + Send
            + Sync
            + 'static,
    {
        *self.refresh_cb.write().await = Some(Box::new(f));
    }

    fn parse_provider(name: &str) -> Result<ProviderId> {
        match name {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai" => Ok(ProviderId::Openai),
            "gemini" => Ok(ProviderId::Gemini),
            "copilot" => Ok(ProviderId::Copilot),
            "local" => Ok(ProviderId::Local),
            other => Err(anyhow!("unknown provider: {other}")),
        }
    }
}

#[async_trait]
impl CredentialResolver for InMemoryResolver {
    async fn get(&self, provider: &str, hint: Option<AuthMode>) -> Result<(AuthMode, AuthCredentials)> {
        let p = Self::parse_provider(provider)?;
        let preferred: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };

        for mode in preferred {
            let mut sc_opt = {
                let g = self.inner.read().await;
                g.get(&(p, mode)).cloned()
            };
            if let Some(sc) = sc_opt.as_mut() {
                let now = chrono::Utc::now();
                if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                    if let Some(cb) = self.refresh_cb.read().await.as_ref() {
                        let new = (cb)(p, mode, sc)?;
                        let mut g = self.inner.write().await;
                        g.insert((p, mode), new.clone());
                        return Ok((mode, new.credentials));
                    } else if sc.is_expired(now) {
                        anyhow::bail!(
                            "{} SSO token expired and no refresh available. \
                             Run: rupu auth login --provider {} --mode sso",
                            provider,
                            provider
                        );
                    }
                }
                return Ok((mode, sc.credentials.clone()));
            }
        }
        Err(anyhow!(
            "no credentials configured for {provider}. \
             Run: rupu auth login --provider {provider} --mode <api-key|sso>"
        ))
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let p = Self::parse_provider(provider)?;
        let sc_opt = { self.inner.read().await.get(&(p, mode)).cloned() };
        let sc = sc_opt
            .ok_or_else(|| anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let cb_guard = self.refresh_cb.read().await;
        let cb = cb_guard
            .as_ref()
            .ok_or_else(|| anyhow!("refresh callback not set in test resolver"))?;
        let new = (cb)(p, mode, &sc)?;
        drop(cb_guard);
        self.inner.write().await.insert((p, mode), new.clone());
        Ok(new.credentials)
    }
}
```

In `crates/rupu-auth/src/lib.rs`:

```rust
pub mod in_memory;
pub mod resolver;
pub use resolver::CredentialResolver;
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-auth --test resolver_default_pref --test resolver_refresh
```

Expected: all four+1 tests green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-auth/src/resolver.rs crates/rupu-auth/src/in_memory.rs \
        crates/rupu-auth/src/lib.rs \
        crates/rupu-auth/tests/resolver_default_pref.rs \
        crates/rupu-auth/tests/resolver_refresh.rs
git commit -m "$(cat <<'EOF'
rupu-auth: CredentialResolver trait + InMemoryResolver

Default precedence (SSO > API-key) implemented on the in-memory
resolver. Pre-emptive refresh fires when expires_at - now < 60s and a
refresh callback is registered.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: `KeychainResolver` against the OS keychain

**Files:**
- Modify: `crates/rupu-auth/src/resolver.rs` (add KeychainResolver)
- Create: `crates/rupu-auth/tests/keychain_resolver.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/rupu-auth/tests/keychain_resolver.rs`:

```rust
//! Real-keyring round-trip. Verifies the spec §13 risk: "no mock
//! features" — the test shells out to `security` on macOS to confirm
//! data actually persisted.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn keychain_resolver_roundtrip_api_key() {
    // Use a unique service name so parallel test runs don't collide.
    let unique = format!("rupu-test-{}", uuid_like());
    let r = KeychainResolver::with_service(&unique);
    r.store(ProviderId::Anthropic, AuthMode::ApiKey, &StoredCredential::api_key("sk-roundtrip"))
        .await
        .expect("store");
    let (mode, creds) = r.get("anthropic", Some(AuthMode::ApiKey)).await.expect("get");
    assert_eq!(mode, AuthMode::ApiKey);
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-roundtrip"),
        _ => panic!(),
    }

    // On macOS, confirm with `security find-generic-password`.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", &unique, "-a", "anthropic/api-key"])
            .output()
            .expect("run security");
        assert!(out.status.success(), "security exited non-zero");
    }

    r.forget(ProviderId::Anthropic, AuthMode::ApiKey).await.expect("forget");
    assert!(r.get("anthropic", Some(AuthMode::ApiKey)).await.is_err());
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}
```

- [ ] **Step 2: Implement `KeychainResolver`**

Append to `crates/rupu-auth/src/resolver.rs` (or create `crates/rupu-auth/src/keychain_resolver.rs` and module-mod it):

```rust
use crate::backend::ProviderId;
use crate::keychain_layout::{key_for, legacy_key_for, KeychainKey};
use crate::stored::StoredCredential;
use rupu_providers::auth::AuthCredentials;

pub struct KeychainResolver {
    service: String,
}

impl KeychainResolver {
    pub fn new() -> Self {
        Self {
            service: "rupu".to_string(),
        }
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, key: &KeychainKey) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &key.account)
            .map_err(|e| anyhow::anyhow!("keychain entry: {e}"))
    }

    pub async fn store(
        &self,
        p: ProviderId,
        mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<()> {
        let key = key_for(p, mode);
        let entry = self.entry(&key)?;
        let json = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        entry
            .set_password(&json)
            .map_err(|e| anyhow::anyhow!("keychain set: {e}"))?;
        Ok(())
    }

    pub async fn forget(&self, p: ProviderId, mode: AuthMode) -> Result<()> {
        let key = key_for(p, mode);
        let entry = self.entry(&key)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("keychain delete: {e}")),
        }
    }

    fn read(&self, p: ProviderId, mode: AuthMode) -> Result<Option<StoredCredential>> {
        let key = key_for(p, mode);
        match self.entry(&key)?.get_password() {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(keyring::Error::NoEntry) => {
                // Slice A legacy fallback: try the old single-key shape
                // (treated as api-key). Only relevant for ApiKey lookups.
                if mode == AuthMode::ApiKey {
                    let lk = legacy_key_for(p);
                    match self.entry(&lk)?.get_password() {
                        Ok(s) => Ok(Some(StoredCredential::api_key(s))),
                        Err(keyring::Error::NoEntry) => Ok(None),
                        Err(e) => Err(anyhow::anyhow!("keychain legacy read: {e}")),
                    }
                } else {
                    Ok(None)
                }
            }
            Err(e) => Err(anyhow::anyhow!("keychain read: {e}")),
        }
    }

    fn parse_provider(name: &str) -> Result<ProviderId> {
        match name {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai" => Ok(ProviderId::Openai),
            "gemini" => Ok(ProviderId::Gemini),
            "copilot" => Ok(ProviderId::Copilot),
            "local" => Ok(ProviderId::Local),
            other => anyhow::bail!("unknown provider: {other}"),
        }
    }
}

#[async_trait]
impl CredentialResolver for KeychainResolver {
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)> {
        let p = Self::parse_provider(provider)?;
        let modes: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };
        for mode in modes {
            if let Some(mut sc) = self.read(p, mode)? {
                let now = chrono::Utc::now();
                if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                    let new = self.refresh_inner(p, mode, &sc).await?;
                    self.store(p, mode, &new).await?;
                    sc = new;
                }
                return Ok((mode, sc.credentials));
            }
        }
        anyhow::bail!(
            "no credentials configured for {provider}. \
             Run: rupu auth login --provider {provider} --mode <api-key|sso>"
        )
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let p = Self::parse_provider(provider)?;
        let sc = self
            .read(p, mode)?
            .ok_or_else(|| anyhow::anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let new = self.refresh_inner(p, mode, &sc).await?;
        self.store(p, mode, &new).await?;
        Ok(new.credentials)
    }
}

impl KeychainResolver {
    async fn refresh_inner(
        &self,
        p: ProviderId,
        _mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<StoredCredential> {
        let oauth = crate::oauth::providers::provider_oauth(p)
            .ok_or_else(|| anyhow::anyhow!("no oauth config for {p}"))?;
        let refresh_token = sc
            .refresh_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(
                "{p} SSO token expired and no refresh token stored. \
                 Run: rupu auth login --provider {p} --mode sso"
            ))?;
        // Provider-agnostic refresh: standard OAuth refresh-token grant.
        let token_url = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| oauth.token_url.to_string());
        let client = reqwest::Client::new();
        let resp = client
            .post(&token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", oauth.client_id),
            ])
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("refresh request: {e}"))?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "refresh failed for {p}: HTTP {}. Run: rupu auth login --provider {p} --mode sso",
                resp.status()
            );
        }
        #[derive(serde::Deserialize)]
        struct R {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<i64>,
        }
        let r: R = resp.json().await.map_err(|e| anyhow::anyhow!("refresh json: {e}"))?;
        Ok(StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: r.access_token.clone(),
                refresh: r.refresh_token.clone().unwrap_or_else(|| refresh_token.to_string()),
                expires: r.expires_in.unwrap_or(0) as u64,
                extra: Default::default(),
            },
            refresh_token: Some(r.refresh_token.unwrap_or_else(|| refresh_token.to_string())),
            expires_at: r.expires_in.map(|s| chrono::Utc::now() + chrono::Duration::seconds(s)),
        })
    }
}
```

(Add `pub use resolver::KeychainResolver;` in `lib.rs`.)

- [ ] **Step 3: Run the integration test**

```
cargo test -p rupu-auth --test keychain_resolver
```

Expected: green on macOS (verifies via `security`); on Linux/Windows the platform-specific verification step is `#[cfg]`-gated out but the round-trip still runs.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-auth/src/resolver.rs crates/rupu-auth/src/lib.rs crates/rupu-auth/tests/keychain_resolver.rs
git commit -m "$(cat <<'EOF'
rupu-auth: KeychainResolver against the OS keychain

Reads/writes StoredCredential JSON at rupu/<provider>/<mode>; falls
back to legacy single-key entries for Slice A users on first
ApiKey lookup. Pre-emptive refresh on near-expiry SSO entries.
Test verifies persistence with `security find-generic-password` on
macOS — guards against the Slice A no-features-flag mock-keychain bug.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — CLI surface for SSO

### Task 10: `rupu auth login --mode <api-key|sso>`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`
- Modify: `crates/rupu-cli/Cargo.toml` (depend on `rupu-auth`'s new resolver if not already)

- [ ] **Step 1: Read existing `cmd/auth.rs`**

Refresh on the current `Login` clap struct shape; the existing `--key` flag stays and becomes API-key-mode-only.

- [ ] **Step 2: Write the test**

Create `crates/rupu-cli/tests/auth_login_modes.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn login_api_key_with_inline_key_succeeds() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args([
            "auth", "login",
            "--provider", "anthropic",
            "--mode", "api-key",
            "--key", "sk-test-flag-only",
        ])
        .assert()
        .success();
}

#[test]
fn login_sso_without_browser_errors_on_headless_linux() {
    // Only meaningful on Linux without DISPLAY; skipped elsewhere.
    if std::env::var_os("DISPLAY").is_some() || cfg!(not(target_os = "linux")) {
        return;
    }
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env_remove("DISPLAY")
        .env_remove("BROWSER")
        .args(["auth", "login", "--provider", "anthropic", "--mode", "sso"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("requires a desktop browser"));
}
```

- [ ] **Step 3: Update the CLI**

Modify `crates/rupu-cli/src/cmd/auth.rs` `Login` variant:

```rust
Login {
    /// Provider name (anthropic | openai | gemini | copilot).
    #[arg(long)]
    provider: String,
    /// Authentication mode.
    #[arg(long, value_enum, default_value = "api-key")]
    mode: AuthModeArg,
    /// API key (only valid with --mode api-key). If omitted, reads from stdin.
    #[arg(long)]
    key: Option<String>,
}
```

Add the enum:

```rust
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum AuthModeArg {
    #[clap(name = "api-key")]
    ApiKey,
    Sso,
}

impl From<AuthModeArg> for rupu_providers::AuthMode {
    fn from(a: AuthModeArg) -> Self {
        match a {
            AuthModeArg::ApiKey => Self::ApiKey,
            AuthModeArg::Sso => Self::Sso,
        }
    }
}
```

Replace the `login()` body:

```rust
async fn login(provider: &str, mode: AuthModeArg, key: Option<&str>) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let mode_neutral: rupu_providers::AuthMode = mode.clone().into();
    match mode {
        AuthModeArg::ApiKey => {
            let secret = match key {
                Some(k) => k.to_string(),
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf.trim().to_string()
                }
            };
            if secret.is_empty() {
                anyhow::bail!("empty API key");
            }
            let sc = rupu_auth::stored::StoredCredential::api_key(secret);
            resolver.store(pid, mode_neutral, &sc).await?;
            println!("rupu: stored {provider} api-key credential");
        }
        AuthModeArg::Sso => {
            let oauth = rupu_auth::oauth::providers::provider_oauth(pid)
                .ok_or_else(|| anyhow::anyhow!("provider {provider} has no SSO flow"))?;
            let stored = match oauth.flow {
                rupu_auth::oauth::providers::OAuthFlow::Callback => {
                    rupu_auth::oauth::callback::run(pid).await?
                }
                rupu_auth::oauth::providers::OAuthFlow::Device => {
                    rupu_auth::oauth::device::run(pid).await?
                }
            };
            resolver.store(pid, mode_neutral, &stored).await?;
            println!("rupu: stored {provider} sso credential");
        }
    }
    Ok(())
}
```

(`handle()` and the dispatch table need updating to pass the new `mode` field through.)

- [ ] **Step 4: Run the tests**

```
cargo test -p rupu-cli --test auth_login_modes
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/auth.rs crates/rupu-cli/tests/auth_login_modes.rs
git commit -m "$(cat <<'EOF'
rupu-cli: rupu auth login --mode <api-key|sso>

API-key mode keeps the existing UX. SSO mode dispatches to the
PKCE callback (Anthropic/OpenAI/Gemini) or device-code (Copilot)
flow. Headless Linux without DISPLAY/BROWSER errors with the
documented message instead of trying to launch a browser anyway.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: `rupu auth logout --mode | --all`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-cli/tests/auth_login_modes.rs`:

```rust
#[test]
fn logout_provider_only_removes_both_modes() {
    // Setup: store an api-key for openai under a unique service via env.
    let unique = format!("rupu-logout-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    std::env::set_var("RUPU_KEYRING_SERVICE_OVERRIDE", &unique);
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "login", "--provider", "openai", "--mode", "api-key", "--key", "k"])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "logout", "--provider", "openai"])
        .assert()
        .success();
    // Now status shows openai api-key as `-`.
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "status"])
        .assert()
        .stdout(predicate::str::contains("openai")
            .and(predicate::str::contains("-")));
    std::env::remove_var("RUPU_KEYRING_SERVICE_OVERRIDE");
}

#[test]
fn logout_all_clears_everything() {
    let unique = format!("rupu-logout-all-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    std::env::set_var("RUPU_KEYRING_SERVICE_OVERRIDE", &unique);
    for p in ["anthropic", "openai", "gemini"] {
        Command::cargo_bin("rupu")
            .unwrap()
            .args(["auth", "login", "--provider", p, "--mode", "api-key", "--key", "x"])
            .assert()
            .success();
    }
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "logout", "--all", "--yes"])
        .assert()
        .success();
    std::env::remove_var("RUPU_KEYRING_SERVICE_OVERRIDE");
}
```

- [ ] **Step 2: Update the `Logout` clap variant**

```rust
Logout {
    /// Provider name (omit with --all to clear everything).
    #[arg(long, conflicts_with = "all")]
    provider: Option<String>,
    /// Specific auth mode to remove. If omitted, both api-key and sso
    /// for that provider are removed.
    #[arg(long, value_enum)]
    mode: Option<AuthModeArg>,
    /// Remove every stored credential across all providers and modes.
    #[arg(long, conflicts_with = "provider")]
    all: bool,
    /// Skip the confirmation prompt for --all.
    #[arg(long, requires = "all")]
    yes: bool,
}
```

- [ ] **Step 3: Implement the dispatch**

```rust
async fn logout(opts: LogoutOpts) -> anyhow::Result<()> {
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    if opts.all {
        if !opts.yes {
            print!("Remove all stored credentials? [y/N]: ");
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf)?;
            if !matches!(buf.trim(), "y" | "yes" | "Y") {
                println!("aborted.");
                return Ok(());
            }
        }
        for p in [
            ProviderId::Anthropic,
            ProviderId::Openai,
            ProviderId::Gemini,
            ProviderId::Copilot,
            ProviderId::Local,
        ] {
            for m in [AuthMode::ApiKey, AuthMode::Sso] {
                let _ = resolver.forget(p, m).await;
            }
        }
        println!("rupu: cleared all credentials");
        return Ok(());
    }
    let provider = opts
        .provider
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--provider required (or use --all)"))?;
    let pid = parse_provider(provider)?;
    let modes = match opts.mode {
        Some(m) => vec![m.into()],
        None => vec![AuthMode::ApiKey, AuthMode::Sso],
    };
    for m in modes {
        resolver.forget(pid, m).await?;
    }
    println!("rupu: forgot credential(s) for {provider}");
    Ok(())
}
```

(`LogoutOpts` is a small struct mirroring the clap variant; pass through from `handle()`.)

- [ ] **Step 4: Run the tests**

```
cargo test -p rupu-cli --test auth_login_modes
```

Expected: green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/auth.rs crates/rupu-cli/tests/auth_login_modes.rs
git commit -m "$(cat <<'EOF'
rupu-cli: logout grammar — --mode, --all (with confirmation)

Spec §5f. --provider X without --mode wipes both api-key and sso for
that provider; --provider X --mode <m> removes just that one. --all
prompts for confirmation (skipped with --yes) and iterates every
combination.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: `rupu auth status` two-column rendering

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`
- Create: `crates/rupu-cli/tests/auth_status_table.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/auth_status_table.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn status_renders_two_column_header() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("API-KEY"))
        .stdout(predicate::str::contains("SSO"));
}

#[test]
fn status_lists_all_four_providers() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("anthropic"))
        .stdout(predicate::str::contains("openai"))
        .stdout(predicate::str::contains("gemini"))
        .stdout(predicate::str::contains("copilot"));
}
```

- [ ] **Step 2: Reimplement `status()`**

```rust
async fn status() -> anyhow::Result<()> {
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    println!("{:<10} {:<10} {}", "PROVIDER", "API-KEY", "SSO");
    for (label, pid) in [
        ("anthropic", ProviderId::Anthropic),
        ("openai", ProviderId::Openai),
        ("gemini", ProviderId::Gemini),
        ("copilot", ProviderId::Copilot),
    ] {
        let api = if resolver.peek(pid, AuthMode::ApiKey).await {
            "✓"
        } else {
            "-"
        };
        let sso = match resolver.peek_sso(pid).await {
            Some(expiry_repr) => format!("✓ ({expiry_repr})"),
            None => "-".to_string(),
        };
        println!("{:<10} {:<10} {}", label, api, sso);
    }
    Ok(())
}
```

(`peek` / `peek_sso` are convenience methods on `KeychainResolver`; expose them in `resolver.rs`. `peek_sso` returns `Option<String>` with the expiry pretty-printed, e.g., `"expires in 23d"` or `"expires in 7d"` or `"expired — re-login"`.)

Add to `KeychainResolver` in `resolver.rs`:

```rust
pub async fn peek(&self, p: ProviderId, mode: AuthMode) -> bool {
    self.read(p, mode).map(|o| o.is_some()).unwrap_or(false)
}

pub async fn peek_sso(&self, p: ProviderId) -> Option<String> {
    let sc = self.read(p, AuthMode::Sso).ok().flatten()?;
    let exp = sc.expires_at?;
    let now = chrono::Utc::now();
    let dur = exp.signed_duration_since(now);
    if dur.num_seconds() <= 0 {
        Some("expired — re-login".into())
    } else if dur.num_days() >= 1 {
        Some(format!("expires in {}d", dur.num_days()))
    } else {
        Some(format!("expires in {}h", dur.num_hours().max(1)))
    }
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-cli --test auth_status_table
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/cmd/auth.rs crates/rupu-cli/src/cmd/auth.rs crates/rupu-cli/tests/auth_status_table.rs crates/rupu-auth/src/resolver.rs
git commit -m "$(cat <<'EOF'
rupu-cli: two-column auth status (API-KEY / SSO)

Spec §8b. SSO column shows expiry pretty-printed (`expires in 23d` /
`expires in 7h` / `expired — re-login`).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Wire resolver into `provider_factory`

### Task 13: `provider_factory` consults the resolver

**Files:**
- Modify: `crates/rupu-cli/src/provider_factory.rs`
- Modify: `crates/rupu-cli/src/cmd/run.rs`

- [ ] **Step 1: Update the factory signature**

Change `build_for_provider` to take a `&dyn CredentialResolver` and an `Option<AuthMode>` hint instead of `&dyn AuthBackend`:

```rust
pub async fn build_for_provider(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
    if let Ok(json) = std::env::var("RUPU_MOCK_PROVIDER_SCRIPT") {
        return Ok((
            rupu_providers::AuthMode::ApiKey,
            build_mock_from_script(&json)?,
        ));
    }
    let (mode, creds) = resolver
        .get(name, auth_hint)
        .await
        .map_err(|e| FactoryError::MissingCredential {
            provider: format!("{name}: {e}"),
        })?;
    let client = match name {
        "anthropic" => build_anthropic(creds, model).await?,
        "openai" | "openai_codex" | "codex" => build_openai(creds, model).await?,
        "gemini" | "google_gemini" => build_gemini(creds, model).await?,
        "copilot" | "github_copilot" => build_copilot(creds, model).await?,
        _ => return Err(FactoryError::UnknownProvider(name.to_string())),
    };
    Ok((mode, client))
}
```

(Each `build_*` now takes `AuthCredentials` directly — drop the env-var fallback since the resolver is the authoritative source. The Plan 1 OPENAI_API_KEY etc. fallbacks move into the resolver as a Step 2 follow-up if needed; for B-1 v1, require explicit login.)

Update `build_anthropic`, `build_openai`, `build_gemini`, `build_copilot` to accept `AuthCredentials` and pass it to the respective client constructors.

- [ ] **Step 2: Update `cmd/run.rs` callers**

Change all `build_for_provider` call sites in `cmd/run.rs` to construct a `KeychainResolver` and pass `agent.auth` as the hint. Use the returned `AuthMode` to fill the run header (replacing the `?` placeholder from Plan 1 Task 16).

- [ ] **Step 3: Update existing tests**

Tests in `provider_factory.rs` that used `FixedKeyBackend` change to use `InMemoryResolver` (already implemented in Task 8). Insert API-key-mode `StoredCredential`s into the resolver, then call the factory.

- [ ] **Step 4: Run all tests**

```
cargo test --workspace
```

Expected: green. The plan-1 mock-seam tests still pass (RUPU_MOCK_PROVIDER_SCRIPT short-circuits before touching the resolver).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/provider_factory.rs crates/rupu-cli/src/cmd/run.rs
git commit -m "$(cat <<'EOF'
rupu-cli: provider_factory consults CredentialResolver

The factory now takes a resolver + auth hint; resolver decides which
mode and returns the actual mode used so the run header can show it.
Slice A's env-var fallback (ANTHROPIC_API_KEY etc.) is dropped at
this layer — explicit `rupu auth login` is the documented path. Live
integration tests (Plan 3) re-introduce env-var support behind
RUPU_LIVE_TESTS for nightly CI only.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Update sample agents to demonstrate auth field

### Task 14: Sample agents with `auth:` field

**Files:**
- Modify: `.rupu/agents/sample-openai.md`
- Modify: `.rupu/agents/sample-gemini.md`
- Modify: `.rupu/agents/sample-copilot.md`
- Create: `.rupu/agents/sample-anthropic-sso.md`

- [ ] **Step 1: Add an explicit SSO agent**

Create `.rupu/agents/sample-anthropic-sso.md`:

```markdown
---
name: sample-anthropic-sso
description: Anthropic via SSO (Claude Pro). Run `rupu auth login --provider anthropic --mode sso` first.
provider: anthropic
auth: sso
model: claude-sonnet-4-6
---

You are a coding assistant. Be concise.
```

(The Plan 1 sample agents already include explicit `auth: api-key` — they remain valid.)

- [ ] **Step 2: Smoke**

```
cargo run -q -p rupu-cli -- agent list
```

Expected: shows the new sample.

- [ ] **Step 3: Commit**

```bash
git add .rupu/agents/sample-anthropic-sso.md
git commit -m "$(cat <<'EOF'
samples: add Anthropic SSO sample agent

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9 — Workspace gates

### Task 15: Final fmt / clippy / test

- [ ] **Step 1: cargo fmt**

```
cargo fmt --all -- --check
```

If needed, `cargo fmt --all` and stage the diff.

- [ ] **Step 2: clippy**

```
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 3: Tests**

```
cargo test --workspace
```

Expected: clean.

- [ ] **Step 4: Manual smoke (matt or dev runs this)**

```
# Verify the legacy Slice A flow still works (no breaking change for existing users)
rupu auth login --provider anthropic --mode api-key --key sk-ant-test
rupu auth status

# Verify SSO via mock (or with a real Anthropic SSO account)
RUPU_OAUTH_SKIP_BROWSER=1 rupu auth login --provider anthropic --mode sso || true
```

- [ ] **Step 5: Commit any fmt fixups**

If Step 1 produced changes, commit them.

---

## Plan 2 success criteria

- `rupu auth login --provider X --mode sso` works for Anthropic, OpenAI, Gemini (browser-callback) and Copilot (device-code).
- `rupu auth status` renders the two-column matrix (`PROVIDER  API-KEY  SSO  (expires in Yd)`).
- `rupu auth logout --provider X` removes both modes; `--mode <m>` removes just one; `--all --yes` clears everything.
- `rupu run` with an agent that has `auth: sso` resolves the SSO credential, refreshes if near expiry, and prints `provider: <name>/sso` in the header.
- An agent without `auth:` falls back to SSO > API-key precedence at run time.
- Refresh failures surface the actionable message: `"<provider> SSO token expired and refresh failed: ... Run: rupu auth login --provider <name> --mode sso"`.
- Headless Linux (no DISPLAY/BROWSER) errors out cleanly when `--mode sso` is requested.
- `KeychainResolver` round-trip test passes on macOS and verifies persistence with `security find-generic-password`.
- `cargo test --workspace`, `cargo clippy ...`, `cargo fmt ...` all green.

## Out of scope (deferred to Plan 3)

- Live `/models` fetching, caching, custom-model TOML registration.
- `rupu models list/refresh` subcommands.
- Streaming `--no-stream` flag on `rupu run`.
- `RUPU_LIVE_TESTS` smoke against real APIs.
- Documentation (`docs/providers.md`, four `docs/providers/<name>.md`).
- Anthropic prompt-cache `cached_tokens` population.
