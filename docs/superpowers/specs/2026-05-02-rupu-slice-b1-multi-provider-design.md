# rupu Slice B-1: Multi-provider wiring — Design

**Status:** Draft for review
**Date:** 2026-05-02
**Slice:** B-1 (first of three Slice B sub-projects)
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice C TUI sketch (memory)](../../../../.claude/projects/-Users-matt-Code-Oracle-rupu/memory/project_slice_c_tui_sketch.md)

---

## 1. Goal

Extend rupu from a single-provider (Anthropic) CLI to a four-provider (Anthropic, OpenAI, Gemini, Copilot) CLI where each provider supports both API-key and SSO authentication. Agents declare which provider+auth+model to use; the runtime resolves credentials, refreshes them on expiry, classifies provider errors uniformly, streams responses, and emits usage telemetry to the transcript.

This is the first of three Slice B sub-projects. B-2 (SCM/issue connectors) and B-3 (`rupu init --with-samples`) follow.

## 2. Why

Slice A wired one provider end-to-end. Real-world coding agents need flexibility: users have ChatGPT subscriptions (SSO), Copilot subscriptions (GitHub-tied), Anthropic API keys, and access to Gemini via Google. Forcing a single provider blocks adoption. Tying auth to API keys only blocks paid SSO users.

The architectural lift is small because phi-providers already defines a normalized `Provider` trait with neutral `ToolCall`/`ToolResult` shapes. B-1 adds three sibling adapters and a credential resolver — no shape changes to the agent runtime.

## 3. Architecture

Hexagonal layout, no change to crate count.

| Crate | Change |
|---|---|
| `rupu-providers` | Add `openai/`, `gemini/`, `copilot/` modules alongside existing `anthropic/`. Add neutral `ProviderError`, `Usage`, `Credentials`, `AuthMode` types at the crate root. |
| `rupu-auth` | Add `CredentialResolver` trait + `KeychainResolver` impl. Add browser-callback OAuth flow (Anthropic, OpenAI, Gemini) and GitHub device-code flow (Copilot). Per-credential keychain entries. |
| `rupu-agent` | Calls `CredentialResolver::get(provider, auth_hint)`; passes `Credentials` to provider adapter. Agent runtime is unchanged structurally — just gains an extra resolution step. |
| `rupu-cli` | Extends `auth login`/`logout`/`status`. Adds `models list`/`refresh`. `rupu run` header gains `provider/auth` annotation; footer gains usage summary. |
| `rupu-orchestrator` | Unchanged. |
| `rupu-transcript` | Adds `Event::Usage { provider, model, input_tokens, output_tokens, cached_tokens }`. |

**Architectural rules preserved (from CLAUDE.md):**
- `rupu-providers` defines ports; agent runtime only knows traits.
- `rupu-cli` stays thin — argument parsing + delegation only.
- Workspace deps only.
- `#![deny(clippy::all)]`, `unsafe_code` forbidden.

## 4. Core types

### 4a. Neutral types (in `rupu-providers`)

```rust
pub enum AuthMode {
    ApiKey,
    Sso,
}

pub enum Credentials {
    ApiKey(String),
    BearerToken {
        token: String,
        expires_at: Option<DateTime<Utc>>,
    },
}

pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_tokens: u32,
}

#[derive(thiserror::Error, Debug)]
pub enum ProviderError {
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("unauthorized: {provider} ({auth_mode:?}). {hint}")]
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

pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub org_id: Option<String>,
    pub region: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_concurrency: Option<usize>,
}
```

The existing `Provider` trait (lifted from phi-providers) keeps its current shape: `complete(&self, req) -> Result<CompletionResponse>` and `stream(&self, req) -> Stream<Item = ProviderEvent>`. Adapter constructors take `Credentials` + `ProviderConfig`.

### 4b. CredentialResolver (in `rupu-auth`)

```rust
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Resolve credentials for a provider. `hint` may force a specific auth mode;
    /// if None, the resolver applies the default precedence (SSO > API-key).
    /// Returns the actual auth mode used + the credentials.
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, Credentials)>;

    /// Force-refresh credentials for a provider+mode. Called by adapters that
    /// receive `Unauthorized` mid-request, and proactively when expiry is near.
    async fn refresh(
        &self,
        provider: &str,
        mode: AuthMode,
    ) -> Result<Credentials>;
}

pub struct KeychainResolver {
    keyring_backend: Box<dyn KeyringBackend>,
    cache: RwLock<HashMap<(String, AuthMode), Credentials>>,
}

#[cfg(test)]
pub struct InMemoryResolver { /* HashMap-backed for tests */ }

// Stored in the keychain entry; the resolver materializes `Credentials` from
// this when an adapter requests them. Provider adapters never see the refresh
// token — that's the resolver's job.
struct StoredCredential {
    credentials: Credentials,
    refresh_token: Option<String>,   // None for API-key entries
}
```

### 4c. AgentSpec frontmatter extension

```rust
pub struct AgentSpec {
    pub provider: String,
    pub auth: Option<AuthMode>,   // None → resolver applies SSO > API-key default
    pub model: String,
    /* existing fields: name, description, system_prompt, tools, etc. */
}
```

Backwards compatible: existing agent files with only `provider: anthropic` continue to load.

## 5. Auth flows

### 5a. API-key flow (all four providers)

```
rupu auth login --provider <name> --mode api-key [--key <value>]
```

1. If `--key` not provided, prompt (terminal masked input).
2. Construct adapter with `Credentials::ApiKey(value)` + default `ProviderConfig`.
3. Call `provider.probe()` — issues a cheap GET (e.g., `/v1/models`) to verify the key reaches the API and authenticates. **Must exercise the real persistence path; no silent success.**
4. On success, write to keychain at `rupu/<provider>/api-key`.
5. On failure, print provider's error verbatim and exit non-zero.

### 5b. Browser-callback SSO (Anthropic, OpenAI, Gemini)

```
rupu auth login --provider <name> --mode sso
```

1. Generate PKCE code verifier (RFC 7636) + S256 challenge.
2. Bind a localhost listener on `127.0.0.1:0` (OS picks free port).
3. Construct provider's authorize URL with `redirect_uri=http://127.0.0.1:<port>/callback`, `state` (random nonce), `code_challenge`, `code_challenge_method=S256`, scopes per provider.
4. Open default browser via `open` crate. If browser launch fails, print URL with: `"Open this URL in a browser to continue: <url>"`.
5. Block on the listener; receive redirect with `?code=...&state=...`. Validate `state`. Respond with a self-closing HTML page ("Authentication complete — return to your terminal").
6. POST `code` + `code_verifier` to provider's token endpoint. Parse access_token + refresh_token + expires_in.
7. Persist `StoredCredential { credentials: BearerToken { token, expires_at: now + expires_in }, refresh_token: Some(rt) }` to keychain at `rupu/<provider>/sso` (JSON-serialized).

**Headless detection:** if `DISPLAY` is unset on Linux and `BROWSER` env var is absent, error with: `"SSO requires a desktop browser. Use --mode api-key for headless setups."` Don't auto-fall-back to device-code in B-1.

### 5c. GitHub device-code flow (Copilot)

```
rupu auth login --provider copilot --mode sso
```

1. POST `https://github.com/login/device/code` with Copilot's client_id and `read:user` scope.
2. Print: `Visit https://github.com/login/device and enter code: ABCD-1234`.
3. Poll `https://github.com/login/oauth/access_token` every 5s (or as `interval` field directs) until `access_token` arrives or the user grants permission.
4. Exchange the GitHub token for a Copilot API token via Copilot's internal endpoint.
5. Store both tokens in keychain at `rupu/copilot/sso` (GitHub token is the refresh path).

### 5d. Refresh logic

`CredentialResolver::refresh(provider, mode)`:
1. Read current credentials from keychain.
2. Call provider's refresh endpoint with the stored refresh token.
3. On success, write new tokens (and new expires_at) back to keychain atomically; return new credentials.
4. On failure, return `Err` with the provider's error message. Caller (agent loop) propagates as a workflow failure with the actionable message: `"<provider> SSO token expired and refresh failed: <reason>. Run: rupu auth login --provider <name> --mode sso"`. **No automatic fall-back to API-key** — the user explicitly chose SSO; switching modes silently would be a "no mock features" violation.

The resolver also pre-emptively refreshes when `expires_at - now < 60s` on a `get()` call.

### 5e. Default precedence

When `agent.auth = None`:
1. Check keychain for `rupu/<provider>/sso`.
2. If present and not expired beyond refresh, use SSO.
3. Else check `rupu/<provider>/api-key`.
4. If present, use API-key.
5. Else error: `"no credentials configured for <provider>. Run: rupu auth login --provider <name> --mode <api-key|sso>"`.

When `agent.auth = Some(mode)`:
- Only that mode is consulted; missing credential errors immediately.

### 5f. `rupu auth logout`

```
rupu auth logout --provider <name> [--mode api-key|sso]
rupu auth logout --all
```

- `--mode` omitted: removes both API-key and SSO entries for that provider.
- `--mode <x>`: removes only that one.
- `--all`: prompts `Remove all stored credentials? [y/N]` then iterates every provider × auth-mode combination.

## 6. Model resolution

### 6a. Sources, in order

1. **User custom models** in `~/.rupu/config.toml`:
   ```toml
   [[providers.openai.models]]
   id = "gpt-5-internal-finetune"
   context_window = 200000
   max_output = 16000
   ```
2. **Live-fetched cache** at `~/.rupu/cache/models/<provider>.json`. TTL: 1 hour. Each entry: `{ id, context_window?, max_output?, deprecated? }`.
3. **Baked-in fallback** for Copilot only (no public `/models` endpoint).

### 6b. Fetch path

- Lazy: first model resolution per process triggers a fetch if cache is stale or absent.
- `/v1/models` (Anthropic, OpenAI), Gemini's `models.list`. Each adapter normalizes the response to the cache schema.
- `rupu models refresh [--provider X]` invalidates and re-fetches.
- `rupu models list [--provider X]` prints the resolved view (custom + cached + baked-in), marking sources.
- Fetch failures: if cache exists, use stale + log warning; if no cache, error.

### 6c. Resolution at agent-load time

1. Look up `agent.model` in custom config → if found, use entry.
2. Else look up in cache (refresh if stale; tolerate stale-on-fetch-fail).
3. Else (Copilot only) consult baked-in list.
4. Unknown model → `"model 'xyz' not found for provider 'openai'. Run 'rupu models list --provider openai' to see available models, or add a custom entry to ~/.rupu/config.toml."`.

### 6d. Default model

No defaults shipped in code. If `agent.model` is missing, check `[providers.<name>].default_model` in `~/.rupu/config.toml`. If still missing, error: `"agent 'foo' has no model and no default_model configured for provider 'anthropic'"`.

## 7. Streaming, errors, concurrency

### 7a. Streaming (default)

All four adapters implement `Provider::stream(&self, req) -> Stream<Item = ProviderEvent>`. `ProviderEvent` (existing in phi-providers) variants:
- `TextDelta { text }`
- `ToolCallStart { id, name }`
- `ToolCallDelta { id, partial_input }`
- `ToolCallEnd { id }`
- `Usage(Usage)`
- `Done`

Wire formats per provider:
- **Anthropic** — existing.
- **OpenAI / Copilot** — SSE `data: {...}\n\n` framing, `[DONE]` sentinel.
- **Gemini** — SSE on `streamGenerateContent`.

Agent loop reads events, renders text incrementally, materializes tool calls into the runtime's `ToolCall` type, emits transcript events. The Slice C `◐ working` glyph maps to "stream open, no `Done` yet."

`--no-stream` flag on `rupu run` calls `Provider::complete(&self, req) -> Result<CompletionResponse>` (one-shot variant). Both methods always available.

### 7b. Error classification

Each adapter has a pure function:

```rust
fn classify_error(status: u16, body: &str, vendor_code: Option<&str>) -> ProviderError;
```

Vendor → variant maps (table-driven, unit-tested):

| Vendor signal | Variant |
|---|---|
| HTTP 401 / `invalid_api_key` / `unauthorized` | `Unauthorized` |
| HTTP 403 / `quota_exceeded` / `billing_*` | `QuotaExceeded` |
| HTTP 404 / `model_not_found` | `ModelUnavailable` |
| HTTP 429 / `rate_limit_exceeded` / 529 (Anthropic overloaded) | `RateLimited { retry_after: parse_retry_after(headers) }` |
| HTTP 400 / `invalid_request_error` | `BadRequest` |
| HTTP 500-503 / network errors | `Transient` |
| Anything else | `Other` |

`Retry-After` header parsed where present (seconds or HTTP-date).

### 7c. Concurrency & client lifecycle

- **One `OnceLock<reqwest::Client>` per provider**, lazily built with `ProviderConfig`'s timeout, HTTP/2 settings.
- **One `Arc<Semaphore>` per provider**, permits = `[providers.<name>].max_concurrency` (defaults: Anthropic 4, OpenAI 8, Gemini 4, Copilot 4 — matching documented per-key rate limits).
- Rate-limit isolation: a saturated OpenAI doesn't drain the connection pool or block calls to Anthropic / Gemini / Copilot.
- **Backoff policy** for `RateLimited` and `Transient`: exponential with jitter, capped at 60s, max 5 attempts. `RateLimited.retry_after` (when present) overrides the backoff schedule for the next attempt.
- **Retry exhausted** → propagate the last error; workflow fails.

## 8. CLI surface

### 8a. New / changed subcommands

```
rupu auth login --provider <anthropic|openai|gemini|copilot> --mode <api-key|sso> [--key <value>]
rupu auth logout --provider <name> [--mode api-key|sso]
rupu auth logout --all
rupu auth status
rupu models list [--provider <name>]
rupu models refresh [--provider <name>]
```

### 8b. `rupu auth status` rendering

```
PROVIDER    API-KEY   SSO
anthropic   ✓         ✓ (expires in 23d)
openai      ✓         -
gemini      -         ✓ (expires in 7d)
copilot     -         ✓ (device-code, expires in 90d)
```

`-` for unconfigured. SSO column shows expiry; API-key column shows only `✓`/`-` (API keys don't expire from rupu's perspective).

### 8c. `rupu models list` rendering

```
PROVIDER   MODEL                       SOURCE       CONTEXT
anthropic  claude-sonnet-4-6           live         200000
anthropic  claude-haiku-4-5            live         200000
openai     gpt-5                       live         400000
openai     gpt-5-internal-finetune     custom       200000
copilot    gpt-4o                      baked-in     128000
```

### 8d. `rupu run` annotations

Header (before first model output):
```
agent: code-writer  provider: anthropic/sso  model: claude-sonnet-4-6
```

Footer (after run completes):
```
Total: 42,103 input / 1,892 output tokens (45,995 cached)
```

### 8e. Help text

Every new flag listed inline in `--help`. `rupu auth login --help` includes a one-line "see `docs/providers/<name>.md` for setup."

## 9. Configuration

### 9a. `~/.rupu/config.toml` schema

```toml
[providers.anthropic]
# All fields optional; vendor defaults apply when absent.
base_url = "https://custom-proxy.example.com"
timeout_ms = 60000
max_retries = 5
max_concurrency = 4
default_model = "claude-sonnet-4-6"

[providers.openai]
org_id = "org-abc123"
default_model = "gpt-5"

[providers.gemini]
region = "us-central1"   # for Vertex AI users

[providers.copilot]
# typically nothing needed

[[providers.openai.models]]   # custom/private models
id = "gpt-5-internal-finetune"
context_window = 200000
max_output = 16000
```

Only fields the user sets need appear. Defaults: `timeout_ms=120000`, `max_retries=5`, `max_concurrency` per the table above.

### 9b. Keychain entry naming

- `rupu/anthropic/api-key`
- `rupu/anthropic/sso` (token + refresh_token + expires_at, JSON-serialized)
- ...same shape for openai, gemini, copilot.

## 10. Testing strategy

### 10a. Per-adapter unit tests (`rupu-providers`)

- **Translation tests** — feed recorded vendor JSON/SSE bytes, assert `ProviderEvent` stream output. One fixture per provider per scenario:
  - text-only response
  - single tool call
  - parallel tool calls
  - streaming text+tool interleave
  - error response (each `ProviderError` variant)
  - rate-limit response with `Retry-After` header
- **Error classification** — pure-function `classify_error(status, body, vendor_code)` table-driven tests covering each variant.
- **httpmock-based integration** — full request → response round-trip per adapter exercising header construction, retry-after handling, SSE framing.

### 10b. Auth flow tests (`rupu-auth`)

- **`KeychainResolver`** — real `keyring` with platform feature flags enabled. Round-trip write/read/delete per provider per auth mode. **Verify with native query (`security find-generic-password` on macOS) that data actually persisted** — this is the explicit guard against the Slice A keyring bug.
- **Browser-callback flow** — integration test spins up the localhost listener, simulates an HTTP GET to the redirect URL with `code`+`state`, mocks the token endpoint via httpmock, asserts keychain entry written.
- **Device-code flow** — httpmock the GitHub `/device/code` and `/access_token` endpoints; assert polling, success, and pending states.
- **Refresh logic** — `Credentials::BearerToken { expires_at: now - 1s }` → resolver invokes refresh → keychain rewritten with new token + new expiry.
- **Default precedence** — SSO present + API-key present → resolver returns SSO; API-key only → returns API-key; neither → error.

### 10c. End-to-end tests (`rupu-cli`)

Extend `RUPU_MOCK_PROVIDER_SCRIPT` env-var seam to accept `--provider <name>` so the same harness exercises CLI dispatch for all four providers. One test per provider:
- `rupu run sample-agent.md` with mock responses scripted
- assert transcript has `Event::Usage` with right provider name
- assert footer prints correct totals

`rupu auth login`/`logout`/`status` exercised against `InMemoryResolver` injected via env var or a test-only `--auth-store` flag.

### 10d. Live integration tests (gated)

- Gated by `RUPU_LIVE_TESTS=1` + per-provider env vars (`RUPU_LIVE_ANTHROPIC_KEY`, etc.).
- One round-trip per provider per auth mode where credentials available — minimal prompt, assert non-empty response and `Usage > 0`.
- Run nightly via separate workflow (NOT per-PR). Skipped silently when env vars absent.

### 10e. Test fixtures

- Recorded vendor bytes: `crates/rupu-providers/tests/fixtures/<provider>/*.{json,sse}`.
- Regen scripts: `crates/rupu-providers/tests/fixtures/regen-<provider>.sh` documented in README so fixtures can be refreshed when vendors change schemas.

## 11. Documentation

- **`README.md`** — adds "Configuring providers" quick-start: 4-row matrix (provider × supported auth modes × where to get credentials) + install commands.
- **`docs/providers.md`** — canonical reference: full provider × auth-mode matrix, common gotchas, troubleshooting tree, `[providers.<name>]` config schema reference.
- **`docs/providers/anthropic.md`** — API-key (console.anthropic.com), SSO browser-callback walkthrough, example agent file per auth mode, refresh expectations.
- **`docs/providers/openai.md`** — API-key (platform.openai.com), ChatGPT SSO browser-callback, `org_id` config note, example agent file.
- **`docs/providers/gemini.md`** — API-key (AI Studio), Google OAuth SSO, Vertex AI region note, example agent file.
- **`docs/providers/copilot.md`** — GitHub device-code walkthrough (mirrors `gh auth login` UX), Copilot subscription requirement, baked-in model list disclaimer + custom-entry instructions, example agent file.
- **`CHANGELOG.md`** — Slice B-1 release notes.
- **In-code doc comments** — every public type in `rupu-providers` and `rupu-auth` carries at minimum a one-line summary; non-trivial APIs include a usage example.

## 12. Out of scope

Explicit non-goals to keep B-1 the size of Slice A's foundation plan:

- Local-model provider (Ollama, llama.cpp, vLLM endpoints) — deferred.
- `rupu usage` aggregation subcommand — backlog item; usage data captured in transcripts now, dashboard built in Slice D.
- OpenAI device-code flow — browser callback only for OpenAI in B-1.
- IDE plugin or non-CLI client documentation.
- Agent migration tooling — backwards-compat at parse time is sufficient.
- Cost calculation in dollars — token counts only; price-per-token table is Slice D scope.
- Cross-provider model aliases (`model: smart` resolving to per-provider best-in-class).
- Concurrency tuning UI / live rate-limit observability.
- Supply-chain hardening of SSO flow beyond standard OAuth PKCE.
- First-class surface for vendor-specific model features (Anthropic prompt caching toggles, OpenAI structured-output mode, Gemini grounding) — adapters expose them as opaque pass-through where natural.

## 13. Risks

- **Vendor SSO endpoint drift.** OAuth flows are vendor-controlled and can change. Mitigation: live integration tests (10d) catch breakage nightly; recorded fixtures (10a) keep the parser stable independently.
- **Keyring feature-flag regression.** The Slice A keychain bug (PR #13) was a missing crate feature flag causing silent mock fallback. Mitigation: tests in 10b verify native persistence with platform-specific queries (`security find-generic-password`), not just keyring round-trips.
- **PKCE / state validation correctness.** Browser callback flow has security-sensitive validation. Mitigation: explicit unit tests for state mismatch, replay attempt, malformed code, missing PKCE.
- **Headless environment confusion.** SSO-default-precedence on a server/CI box without a browser would fail at run-time. Mitigation: clear error message + headless detection at login; documented in `docs/providers.md` troubleshooting.
- **Refresh-token expiry causing mid-workflow failure.** A long-running workflow could outlive an SSO refresh window. Mitigation: pre-emptive refresh at 60s-before-expiry; surface clear actionable error if refresh itself fails.

## 14. Success criteria

- `rupu auth login --provider <X> --mode <Y>` succeeds for all 4×2 = 8 combinations with credentials persisted to the platform keychain (verified end-to-end, not just round-tripped through the keyring crate).
- `rupu run` works against a sample agent for each provider, with both API-key and SSO auth, streaming and non-streaming.
- Agent file with no `auth:` field correctly applies SSO > API-key default precedence.
- `rupu auth status` renders the four-provider × two-auth-mode matrix correctly.
- `rupu models list` resolves custom + cached + baked-in entries across providers.
- Transcript JSONL contains `Event::Usage` per response.
- Live integration tests (when env vars present) pass against real APIs nightly.
- Existing Slice A agent files load unmodified.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` all green.
