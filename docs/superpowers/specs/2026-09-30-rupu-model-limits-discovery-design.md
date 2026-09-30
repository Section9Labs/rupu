# rupu model-limits discovery — design

**Date:** 2026-09-30
**Status:** Design approved; plan written (`docs/superpowers/plans/2026-09-30-rupu-model-limits-discovery.md`)
**Scope:** `rupu-providers`, `rupu-runtime`, `rupu-agent`, `rupu-orchestrator`, `rupu-cli`, `rupu-cp` (API + web). The macOS app is deprecated and out of scope.
**Companion:** the response-outcomes spec (refusals, every stop/finish reason, unrecognized replies) is a separate design, brainstormed after this one. §9 lists what moved there.

## 1. Problem

rupu knows nothing about a model's real limits at run time:

- **Output cap.** Every agent without `maxTokens` gets `DEFAULT_MAX_TOKENS = 8192` (`crates/rupu-agent/src/runner.rs:86`). Current models allow 128K, and adaptive thinking draws from the same budget, so long outputs get cut off.
- **Context window.** Without `contextWindowTokens` in frontmatter, proactive compaction is off (`runner.rs:432`). The only recovery is the reactive trim loop (`runner.rs:1250`), which deletes the oldest exchange with no summary, up to 64 times, after the provider has already rejected the request.
- **The data is there.** Most providers publish these limits on their model-list endpoint. rupu already calls several of those endpoints, then discards everything except the model id. For example, Anthropic's `list_models` hardcodes `context_window: 0, max_output_tokens: 0` (`crates/rupu-providers/src/anthropic.rs:2366`).
- **The existing 1M opt-in is stale.** It uses the `contextWindow: 1m` field and the `[1m]` model suffix to send `context-1m-2025-08-07`. But 1M has been the default without a beta header on Opus 4.6 / Sonnet 4.6 and newer since 2026-03-13.

## 2. Goal

Every run uses the real limits of the model it talks to, discovered from the provider and cached for an hour, unless the agent definition pins a value. Each limit records where it came from. When a limit can't be known, the run says so explicitly instead of quietly guessing.

Non-goals:
- Handling refusals, `pause_turn`, `model_context_window_exceeded`, and the other stop and finish reasons. These belong to the response-outcomes spec (§9).
- Removing the `contextWindow: 1m` / `[1m]` beta-header gating. That's a follow-up. Discovery never changes which headers are sent.
- Per-host refetch from the CP.
- `local.rs` (llama.cpp / Ollama).

## 3. Provider-reported limits

`ModelInfo` (`crates/rupu-providers/src/model_pool.rs:70`) keeps its shape: `context_window: u32` and `max_output_tokens: u32`, with 0 meaning unknown (the existing convention, already honored by `rupu models list`). There are 28 construction sites, and none of them change.

`context_window` is redefined as **the number of input tokens this model will accept**. Each provider maps its native fields onto that meaning, so the agent layer never learns provider quirks:

| Provider (auth) | Endpoint | `context_window` = | `max_output_tokens` = |
|---|---|---|---|
| Anthropic (api-key + OAuth) | `GET /v1/models?limit=1000`, following `has_more` / `after_id` | `max_input_tokens` (null → 0) | `max_tokens` (null → 0) |
| Codex / ChatGPT (OAuth) | `GET /backend-api/codex/models?client_version=…` (already called) | `context_window × effective_context_window_percent / 100`; if `context_window` is absent, `max_context_window` | 0; OpenAI Responses omits it (§6.3) |
| OpenAI (api-key) | first `GET chatgpt.com/backend-api/codex/models` with the API key, keeping only `supported_in_api: true`; if that fails, `GET /v1/models` for ids only | same as the row above | 0 |
| GitHub Copilot | live `GET {api}/models` (new; the built-in list becomes an offline fallback with limits of 0) | `capabilities.limits.max_prompt_tokens`; if absent, `max_context_window_tokens` | `capabilities.limits.max_output_tokens` |
| Gemini AI Studio (api-key) | `GET /v1beta/models`, following `nextPageToken` (new); strip the `models/` prefix from `name` | `inputTokenLimit` | `outputTokenLimit` |
| Gemini CLI / Antigravity (OAuth) | none; Code Assist has no listing method | 0 | 0 |
| OpenAI-compatible (vLLM) | live `GET {base}/v1/models` fills in fields config didn't set | `max_model_len`; a LoRA entry with null falls back to its `parent` | 0; the server caps output to fit |

Notes:
- **Why `max_prompt_tokens` for Copilot.** It's often much smaller than the window (for example 128K vs 400K). VS Code Copilot uses it for the same reason.
- **Codex's `max_context_window`** is only a ceiling for a user's own `config.toml` override. The wire has no window field, so rupu uses `context_window`.
- **Made-up defaults are removed.** `DEFAULT_OAI_CONTEXT_WINDOW = 32_768` and `DEFAULT_OAI_MAX_OUTPUT = 8_192` (`crates/rupu-runtime/src/provider_factory.rs:64`) invent limits for unconfigured OpenAI-compatible models. Unset now means unknown, then live `max_model_len`.
- **New trait method:** `LlmProvider::fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError>`. Its default returns `Err(NotImplemented)`, meaning the provider exposes no listing. It exists alongside the legacy `list_models(&self)`, which hides failures as an empty list, for two reasons: `&mut self` lets OAuth providers refresh an expired token before listing (Anthropic, Copilot), and a real error lets the resolver tell "fetch failed, use the stale cache" apart from "model not listed". The `tuned.rs` decorators forward it.
- **New trait method:** `LlmProvider::output_shares_context(&self) -> bool`, default `true`. It says whether generated output counts against the same window as the input. Anthropic, Codex and OpenAI-compatible return `true`; Copilot and Gemini return `false`, because their input and output budgets are independent. It feeds the headroom rule in §6.4.
- **Fetch timeout.** Model-list fetches use a 10s timeout, the same as the existing Anthropic models request (`anthropic.rs:1477`).

## 4. Registry cache v2

`ModelRegistry` (`crates/rupu-providers/src/model_registry.rs`) keeps its sources and order: Custom (config), then Live (cache, 1h TTL), then BakedIn (Copilot only).

- **v2 cache file.** `~/.rupu/cache/models/<provider>.json` becomes `{ "schema": 2, "fetched_at", "models": [{ "id", "context_window", "max_output_tokens" }] }`.
- **v1 files are stale.** A v1 file (ids only, no `schema`) is treated as stale and refetched, never read as "limits 0".
- **Atomic writes.** Writes go to a temp file in the same directory, then rename, so fan-out units launching together can't tear the file. Two units refetching at the same moment is acceptable.
- **Cache key.** The cache is keyed by the **configured provider name** (`anthropic`, `openai`, a custom alias), never by `LlmProvider::provider_id()`. `provider_id()` is known to be wrong for `local.rs` and `broker_client.rs`, which both return `Anthropic`. That bug is tracked in the response-outcomes spec.
- **Config entries.** `[[providers.X.models]]` entries (`CustomModel { id, context_window, max_output }`, `crates/rupu-config/src/provider_config.rs:44`) stay the Custom source. When both exist for the same id, the config value wins over the live value, field by field.

## 5. Resolver — `rupu_runtime::model_limits`

The refresh logic moves out of the CLI (`crates/rupu-cli/src/cmd/models.rs:285`, a CLAUDE.md rule-2 violation) into a new `rupu-runtime` module. The CLI, the CP and every run launch share it.

```rust
pub struct ModelLimits {
    pub input: Limit,              // usable input tokens (§3 semantics)
    pub output: Limit,             // max output tokens
    pub compact_at_percent: u8,    // agent value, else 80; clamped to [10, 95]
    pub output_shares_context: bool,
}
pub struct Limit { pub tokens: Option<u32>, pub source: LimitSource }
pub enum LimitSource {
    Agent,                                    // agent frontmatter
    Config,                                   // [[providers.X.models]]
    Live { fetched_at: DateTime<Utc>, stale: bool },
    Observed,                                 // learned from a provider error (§7)
    Unknown,
}

pub async fn refresh(cfg: &Config, provider: Option<&str>) -> Vec<RefreshOutcome>;
pub async fn resolve(
    overrides: LimitOverrides,   // spec.context_window_tokens / max_tokens / compact_at_percent
    provider_name: &str,
    model: &str,
    provider: &mut dyn LlmProvider,  // the run's own instance: same auth, same host
    cfg: &Config,
    cache_dir: &Path,
) -> ModelLimits;
```

- **Precedence**, per field: agent frontmatter, then config, then live cache, then unknown.
- **Refetching.** If the cache is stale or missing, `resolve` refetches through `provider` (§3 timeout). If the fetch fails, it uses the stale entry with `stale: true`. If there's no entry at all, the limit is `Unknown`. `resolve` never errors: an unknown limit is a valid, reported outcome.
- **Model lookup:**
  1. Strip a `[1m]` suffix, case-insensitive.
  2. Try an exact id match.
  3. Otherwise match an entry whose id is `<model>-<YYYYMMDD>` (a dated snapshot), taking the newest date. So `claude-haiku-4-5` resolves to `claude-haiku-4-5-20251001`.
- **Crate placement.** `ModelLimits`, `Limit` and `LimitSource` live in `rupu-providers`, next to `ModelInfo`, so `rupu-agent` can hold them without depending on `rupu-runtime`. `resolve` and `refresh` live in `rupu-runtime`.

## 6. Run wiring

### 6.1 `AgentRunOpts`

`AgentRunOpts.max_tokens: u32`, `context_window_tokens: Option<u32>` and `compact_at_percent: Option<u8>` are replaced by `limits: ModelLimits`. Because the field is required, a launch site that forgets to resolve fails to compile. These sites call `resolve` once:

- `rupu run`: `crates/rupu-cli/src/cmd/run.rs:948`
- sub-agent dispatch: `crates/rupu-cli/src/cmd/dispatch.rs:419`, resolving the **child's** own provider and model
- session start: `crates/rupu-cli/src/cmd/session.rs:1633`
- the orchestrator step factory: `crates/rupu-orchestrator/src/step_factory.rs:498`

Test harnesses and the runner's internal summary calls (`runner.rs:1839` etc.) use `ModelLimits::unknown()` or `ModelLimits::fixed(input, output)`.

Placed and remote units run `rupu run` on their host, so they resolve there with that host's login and cache. Nothing special is needed.

### 6.2 `LlmRequest.max_tokens` → `Option<u32>`

"No cap" is represented explicitly rather than faked with a number. The change to every `LlmRequest` construction site is mechanical.

### 6.3 On the wire

| Provider | `max_tokens` = `Some(n)` | `None` (output unknown and not pinned) |
|---|---|---|
| Anthropic | `max_tokens: n` | `max_tokens: 8192` (required by the API); the run-start notice says so |
| Codex / OpenAI Responses | `max_output_tokens: n`, still subject to the existing per-model gate (`openai_codex.rs:522`) | field omitted, so the model's own max applies |
| Copilot / OpenAI-compatible (chat completions) | `max_tokens: n` | omitted |
| Gemini | `generationConfig.maxOutputTokens: n` | omitted |

When the output limit is known, the runner sends `limits.output.tokens` on every turn.

Safety of the Anthropic max, verified against the API docs:
- `max_tokens` doesn't count toward output rate limits (OTPM), which count only generated tokens.
- On Claude 4.5+, input + `max_tokens` larger than the window is accepted.

### 6.4 Compaction threshold

The threshold is based on `input` and `compact_at_percent`:

```
input unknown                        → compaction off (today's behavior), stated in the notice
output unknown or !shares_context    → threshold = input × pct / 100
otherwise                            → threshold = min(input × pct / 100, input − output)
```

The `input − output` headroom rule compacts early enough that a full-length reply still fits.
- **Haiku 4.5 (200K / 64K):** the threshold is 136K, not 160K. Without the rule, raising `max_tokens` to 64K would let a reply hit the window before the response-outcomes spec exists to handle `model_context_window_exceeded`.
- **1M models (1M / 128K):** the percentage wins (800K). The rule never triggers.

### 6.5 Sessions

The resolved `ModelLimits`, including sources, is stored on the session record on the session's **first turn**. `session start` builds no provider, so the first `_run-turn` resolves and writes it. This is an additive serde field: an older record without it resolves on its next turn. Later turns reuse the stored value; it never refetches in the middle of a session. After each turn, the run's final limits are written back (`RunResult.final_limits`), so a limit learned from an overflow error (§7) persists.

### 6.6 Run-start notice

Each run writes one `Event::Notice { kind: "model_limits", message }` before the first turn. For example:

`input 1,000,000 · output 128,000 · compact at 800,000 (80%) — anthropic /v1/models, cached 12m ago`

or

`input unknown · output unknown (8192 fallback) · compaction off — gemini-cli exposes no model limits; set contextWindowTokens/maxTokens on the agent or [[providers.gemini.models]]`

## 7. Learning the real limit from overflow errors

The live value describes the model, not the account's entitlement. For example, an OAuth account without extra-usage billing can be refused long context with a 429 "Extra usage is required for long context requests" (`anthropic.rs:230`).

`is_context_overflow` (`runner.rs:159`) becomes `parse_context_overflow(err: &str) -> Option<Overflow { tokens: Option<u32>, max: Option<u32> }>`. It's driven by a table of observed formats:

| Provider | Observed format |
|---|---|
| Anthropic | `prompt is too long: N tokens > M maximum` |
| OpenAI / Copilot | `maximum context length is M tokens … resulted in N tokens` |
| vLLM | `Input length (N) exceeds model's maximum context length (M)` |
| Gemini | `input token count (N) exceeds the maximum number of tokens allowed (M)` |

Plus today's three phrases, which match overflow but carry no max.

None of these formats are documented, so each is pinned by a fixture test. OpenAI's wording doesn't match any of today's three phrases (a gap the parsing audit found), and this table closes it.

When `max` is parsed and is below `limits.input` (or `limits.input` is unknown):
1. Set `limits.input = max` with source `Observed`, for the rest of the run. Sessions persist it (§6.5).
2. Write `Notice { kind: "model_limits_clamped" }`, e.g. `input 1,000,000 → 200,000 (provider error)`.
3. **Compact with a summary** at the recomputed threshold, then retry.
4. The delete-oldest trim loop stays as the last resort: no `max` parsed, compaction failed, or the request still overflows.

The observed value is **not** written to the model cache, because it reflects the account, not the model. If the clamp recurs, the notice suggests pinning `contextWindowTokens` on the agent.

## 8. Surfaces

### 8.1 CLI

- `rupu models list` gains `output` and `fetched` columns next to the existing `context` column. The CSV and JSON reports get matching fields.
- `rupu models refresh` keeps its interface, now backed by `rupu_runtime::model_limits::refresh`.

### 8.2 CP backend

rupu-cp stays read-only. It gets a new port, following the `RepoLister` / `AgentLauncher` pattern:

```rust
#[async_trait]
pub trait ModelCatalog: Send + Sync {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError>;
    async fn refresh(&self, provider: Option<String>) -> Result<Vec<RefreshOutcome>, ModelCatalogError>;
}
```

`rupu cp serve` (`crates/rupu-cli/src/cmd/cp.rs`) implements it by delegating to `rupu_runtime::model_limits`. Without the port, both endpoints return 501, the existing pattern.

- **`GET /api/models`** returns `[{ provider, fetched_at, stale, models: [{ id, input_tokens, output_tokens, source }] }]`. Here `source` is `live | custom | baked-in`, and a limit is null when unknown.
- **`POST /api/models/refresh`**, body `{ provider?: string }`, returns `[{ provider, ok, count, error? }]`.
  - It uses the same auth as the other CP mutation endpoints.
  - It's synchronous: providers are fetched in parallel, each with the 10s timeout. So a 200 means refreshed, unlike run mutations, where a 200 only means recorded.
  - One provider failing doesn't fail the others.

### 8.3 CP web — Settings → Models tab

A new tab on the global Settings page (`crates/rupu-cp/web/src/pages/Settings.tsx`). It does not go in `ProvidersTab`, which is a config editor shared with project config (`components/project/ProjectConfigTab.tsx`).

- Models are grouped by provider, showing id, input limit, output cap and source. Unknown limits render as "—".
- Each provider has a header, "fetched 12m ago" (plus a "stale" badge when stale), with a **Refetch** button for that provider. A **Refetch all** button sits at the top.
- Refresh errors render inline per provider, e.g. `copilot: 401 — not logged in`.

Refetch refreshes the CP machine's cache only. Remote hosts keep their own caches and refresh at launch.

## 9. Deferred to the response-outcomes spec

These came up here and belong to the companion spec:

- **Stop reasons:** `StopReason::ContextWindowExceeded` (compact and continue), `pause_turn`, `refusal` with Anthropic server-side `fallbacks`, and every other provider's finish, safety and incomplete signals. No unidentified reply: anything rupu doesn't recognise becomes a surfaced error that carries the raw payload.
- **Runner success rule:** today a run is `Ok` whenever a response has no tool calls, whatever the stop reason (`runner.rs:1700`).
- **Error bodies:** structured parsing instead of flattened strings. After that, §7's string table can read error fields directly.
- **Wrong `provider_id()`:** `local.rs` and `broker_client.rs` both return `Anthropic`.

**Interim risk, until that spec lands:**
- A `max_tokens` stop still ends a run as `Ok`. That's pre-existing, and it becomes rarer, because the cap rises from 8192 to the model max.
- On shared-window providers, `model_context_window_exceeded` is prevented by §6.4's headroom rule rather than handled.
- `--no-stream` still fails to decode an Anthropic response with an unrecognized stop reason.

## 10. Testing

- **Provider parsers** (httpmock):
  - Anthropic: multiple pages, null limit fields, and both auth headers.
  - Codex: OAuth and api-key paths, the `supported_in_api` filter, the 95% calculation, and the `max_context_window` fallback.
  - Copilot: `max_prompt_tokens` wins over the window size, and the offline fallback to the built-in list.
  - Gemini AI Studio: multiple pages and the `models/` prefix strip.
  - vLLM: `max_model_len`, including a LoRA entry with a null value.
- **Registry:** a v1 file counts as stale; v2 round-trips; config beats live field by field; atomic-write rename.
- **Resolver:** a precedence table (agent / config / live / unknown for each field), dated-snapshot matching, `[1m]` stripping, a failed fetch falling back to the stale entry, and a missing cache resolving to `Unknown`.
- **Runner** (`MockProvider`):
  - threshold math for all three branches of §6.4, including the headroom rule;
  - each overflow fixture in §7 lowers the limit and compacts, rather than trimming;
  - a string that doesn't parse still falls back to trimming;
  - the `model_limits` and `model_limits_clamped` notices are written.
- **Request bodies:** Anthropic `max_tokens` is the resolved value or the 8192 fallback; Codex, Copilot and Gemini omit the field when `None`.
- **Sessions:** resolved limits persist, and resume doesn't refetch.
- **CP:** `GET` and `POST` endpoint tests (501 without the port, the list shape, a partial-failure refresh), plus vitest for the Models tab (refetch buttons send the POST; per-provider errors render).
- **Live check, owed with matt's approval before merge:** one real refresh against the Anthropic OAuth path, to confirm `max_input_tokens` and `max_tokens` are returned there and what `claude-sonnet-4-6` reports. The research found the fields documented only for API keys.
- **GUI check:** matt runs the CP Models tab before the UI merges.

## 11. Docs

- **`docs/agent-format.md`:** `contextWindowTokens` / `maxTokens` / `compactAtPercent` are now optional overrides of discovered limits.
- **`docs/providers.md`, `docs/providers/gemini.md`:** which providers report limits, the Gemini CLI gap, and declaring limits in `[[providers.X.models]]`.
- **CLAUDE.md:** add a `rupu-runtime` crate entry (`provider_factory`, `model_limits`).

## 12. Plans

One implementation plan: `docs/superpowers/plans/2026-09-30-rupu-model-limits-discovery.md`, delivered as a single PR.
