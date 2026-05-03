# Changelog

## v0.1.0 — Slice B-1: Multi-provider wiring (TBD release date)

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
