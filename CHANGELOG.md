# Changelog

## v0.1.1 — SSO hotfix (2026-05-03)

### Fixed

- **Anthropic SSO** now succeeds against `claude.ai/oauth/authorize`. The previous scope set mixed Console-flow scopes (`org:create_api_key`) into the claude.ai authorize call, which `claude.ai` rejected with "Invalid request format". The new scope set is the full Claude Code request shape (`user:inference`, `user:profile`, `user:sessions:claude_code`, `user:mcp_servers`) — matches what users see on the consent screen since we use Claude Code's OAuth client_id, and avoids re-login when we eventually surface session/MCP features.
- **OpenAI SSO** now matches the Codex CLI request shape verified against `openai/codex codex-rs/login/src/server.rs`:
  - `token_url` corrected from `console.anthropic.com/v1/oauth/token` (a copy-paste bug from Plan 2 Task 4) to `auth.openai.com/oauth/token`.
  - Redirect URI uses fixed ports `1455` (with `1457` fallback) on `localhost`, path `/auth/callback` — these are pinned by OpenAI's Hydra registration for the `app_EMoamEEZ73f0CkXaXp7hrann` client.
  - Scopes extended with `api.connectors.read api.connectors.invoke`.
  - Authorize URL now sends the Codex CLI extras: `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator=codex_cli_rs`.

### Internal

- `ProviderOAuth` (`crates/rupu-auth/src/oauth/providers.rs`) gains three new fields — `redirect_host`, `fixed_ports`, and `extra_authorize_params` — so each provider can declare its specific redirect-URI shape and additional authorize-query parameters without per-provider branching in the callback flow.
- The redirect listener (`oauth/callback.rs`) walks `fixed_ports` in order before falling back; `None` keeps the original OS-assigned port-0 behavior.

### Honest acknowledgements

We currently impersonate Claude Code's and Codex CLI's OAuth clients. The consent screen reads "Claude Code wants access ..." and "Codex CLI wants access ..." rather than "rupu wants ...". This is necessary while we use their pre-registered redirect URIs and scope sets; the long-term fix (registering rupu-specific OAuth clients with each vendor) is tracked in `TODO.md`.

## v0.1.0 — Slice B-1: Multi-provider wiring (2026-05-02)

### Added

- **OpenAI, Gemini, GitHub Copilot provider adapters wired end-to-end.** Anthropic remains the most exercised; Gemini API-key path via AI Studio is deferred to a follow-up (see `TODO.md`).
- **SSO authentication for all four providers:**
  - Browser-callback (PKCE) for Anthropic, OpenAI, Gemini.
  - GitHub device-code for Copilot (mirrors `gh auth login` UX).
- **`CredentialResolver` trait + `KeychainResolver` impl** with refresh-on-expiry. Per-credential keychain entries (`rupu/<provider>/<api-key|sso>`).
- **Default auth precedence:** SSO wins when both modes configured. Override by setting `auth: api-key` or `auth: sso` in agent frontmatter.
- **`rupu auth login --mode <api-key|sso>`.**
- **`rupu auth logout --provider X [--mode <m>]`** and **`rupu auth logout --all [--yes]`**.
- **`rupu auth status`** two-column rendering: `PROVIDER  API-KEY  SSO  (expires in Yd)`.
- **`rupu models list [--provider X]`** — custom + live-fetched + baked-in entries with source labels.
- **`rupu models refresh [--provider X]`** — re-fetch `/models` for each configured provider; cache at `~/.rupu/cache/models/<provider>.json` (TTL 1h).
- **`[providers.<name>]` config block** in `~/.rupu/config.toml`: `base_url`, `org_id`, `region`, `timeout_ms`, `max_retries`, `max_concurrency`, `default_model`, `[[providers.X.models]]`.
- **`Event::Usage { provider, model, input_tokens, output_tokens, cached_tokens }`** written to JSONL transcripts per response.
- **Anthropic prompt-cache** integration: `cache_read_input_tokens` decoded into `Usage.cached_tokens`.
- **`rupu run` header** (`agent: X  provider: Y/Z  model: M`) and **footer** (`Total: I input / O output tokens`).
- **`--no-stream`** flag on `rupu run` (default is streaming with on-the-fly TextDelta print to stdout).
- **Documentation:** `docs/providers.md` canonical reference + four `docs/providers/<name>.md` per-provider walkthroughs.
- **Nightly live-integration test workflow** gated by `RUPU_LIVE_TESTS=1`. Anthropic / OpenAI / Copilot covered; Gemini deferred.
- **Per-provider concurrency semaphore** (`Anthropic 4, OpenAI 8, Gemini 4, Copilot 4` defaults; configurable). Rate-limit isolation across vendors.
- **Per-vendor `classify_error()`** pure functions mapping HTTP status + body + vendor code → structured `ProviderError` variants (`RateLimited`, `Unauthorized`, `QuotaExceeded`, `ModelUnavailable`, `BadRequest`, `Transient`, `Other`).

### Changed

- **`AgentSpec` frontmatter** now accepts optional `auth: <api-key|sso>` field.
- **`provider_factory`** consults `CredentialResolver` instead of `AuthBackend` directly. Slice A's env-var fallback (`ANTHROPIC_API_KEY` etc.) is dropped at this layer; explicit `rupu auth login` is the documented path. The nightly live-test suite re-introduces env-var support behind `RUPU_LIVE_TESTS` for CI only.
- **Sample agents** in `.rupu/agents/` updated to demonstrate `auth:` (`sample-openai.md`, `sample-gemini.md`, `sample-copilot.md`, `sample-anthropic-sso.md`).

### Backward-compatible

- **Existing Slice A agent files** (`provider: anthropic` only) load unchanged. Missing `auth:` triggers the default-precedence path.
- **Legacy keychain entries** (Slice A's `rupu/<provider>` shape) are still readable by the resolver as API-key on first lookup.

### Deferred (see `TODO.md`)

- macOS keychain code-signing + notarization (highest-impact UX bug; track via TODO.md).
- `rupu usage` aggregation subcommand (Slice D).
- Gemini API-key path via AI Studio.
- Copilot `default_model` literal alignment.
- `classify::truncate` UTF-8 walk-back regression test gap.

## v0.0.3-cli — Slice A (2026-04-XX)

Initial single-binary release: Anthropic provider, agent file format, JSONL transcripts, action protocol, permission resolver, linear workflow runner, OS keychain auth backend.
