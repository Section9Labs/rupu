# Generic OpenAI-compatible providers (Oracle GenAI)

**Date:** 2026-06-29
**Status:** Design — pending review
**Author:** matt + Claude

## Motivation

Oracle gave us an inference endpoint that speaks the standard OpenAI
chat-completions wire protocol:

```bash
curl -sS http://192.29.35.246:8080/v1/chat/completions \
    -H 'Content-Type: application/json' \
    -H 'Authorization: Bearer <API_KEY>' \
    -d '{
      "model": "/raid/models/zai-org/GLM-5.2-FP8",
      "messages": [{"role": "user", "content": "What is 17 * 23?"}],
      "max_tokens": 2048,
      "temperature": 0.6
    }'
```

This is not a branded cloud service with a stable URL and a bespoke auth
dance — it is a **self-hosted, OpenAI-compatible server at a
per-deployment address with a static Bearer key**. That shape is a
reusable primitive: the same client covers vLLM, Together, Fireworks,
OpenRouter, LM Studio, and any future self-hosted endpoint. We therefore
build **one generic `openai-compatible` provider driven by config +
`auth.json`**, with Oracle as its first named instance — not a
hardcoded `Oracle` provider.

## Goals

- Add a `rupu` provider that talks OpenAI `/v1/chat/completions`,
  including **function/tool calling** (rupu is an agentic loop; text-only
  is useless), with a **configurable base URL** and a **static Bearer
  key**.
- Declare instances purely in config: `[providers.<name>]` with
  `kind = "openai-compatible"`, `base_url`, `default_model`, and a
  `models` list. No new Rust per endpoint.
- Store the secret in `auth.json` under the provider name (consistent
  with every other provider, mode-0600), with an env-var fallback for
  headless/CI use.

## Non-goals

- OCI Generative AI **managed** service with OCI request-signing. The
  endpoint we were given is a plain Bearer-key OpenAI server; the managed
  signed-request variant is out of scope for this slice.
- OS-keychain storage for these providers. Keys land in `auth.json` (the
  fallback file backend) for now; keychain integration for arbitrary
  named providers is a possible later follow-up.
- A fully open credential namespace. A provider name is only honored if
  it is declared in the `[providers.*]` config map; unknown names are
  still rejected.

## Current architecture (what we reuse)

Three dispatch points sit between a provider-name string and a live client:

1. **`rupu-providers::ProviderId`** (enum) — used by `ProviderRegistry`,
   but the runtime factory dispatches on a **string**, so this enum is
   effectively bypassable for the new path.
2. **`rupu-runtime::provider_factory::build_for_provider_with_config`** —
   `match name { "anthropic" => …, _ => UnknownProvider }`. This is where
   we add the generic branch.
3. **`rupu-auth`** — its own closed `ProviderId` enum
   (`backend.rs`), with the keychain/json keys derived from
   `ProviderId::as_str()`. The resolver's `parse_provider()` bails on
   unknown names. This is the one seam that genuinely resists arbitrary
   names today.

Reusable assets:

- **`github_copilot.rs`** is the template: OpenAI chat/completions
  request build, Bearer auth, and full tool-call parsing for both
  non-streaming responses and streaming SSE (`tool_calls` accumulation).
  Its only extra baggage is the GitHub→Copilot token exchange, which
  Oracle does not need.
- **`local.rs`** (`LocalModelProvider`) already has the configurable
  endpoint + model shape but is **text-only** (no tools, no auth) — it is
  the right *shape* but the wrong *capability*; we do not extend it
  (keeps "local inference server" distinct from "remote keyed endpoint").
- **`ProviderConfig`** (`provider_config.rs`) already carries `base_url`,
  `default_model`, `timeout_ms`, `max_retries`, `max_concurrency`, and a
  `models: Vec<CustomModel>` list. The top-level `Config.providers` is a
  `BTreeMap<String, ProviderConfig>` keyed by arbitrary name.
- **`auth.json`** (`json_file.rs`) is a flat `BTreeMap<String, String>`
  on disk — already string-keyed; only the typed `AuthBackend` API on top
  is enum-gated.

## Design

### 1. New client: `OpenAiCompatibleClient` (rupu-providers)

A new module `crates/rupu-providers/src/openai_compatible.rs`,
implementing `LlmProvider`. Constructed from explicit parameters (no
hardcoded URLs):

```rust
pub struct OpenAiCompatibleClient {
    base_url: String,        // e.g. "http://192.29.35.246:8080"
    api_key: String,         // static Bearer
    default_model: String,   // e.g. "/raid/models/zai-org/GLM-5.2-FP8"
    models: Vec<ModelInfo>,  // from config `models` list
    stream: bool,            // default true; false → non-streaming POST
    client: reqwest::Client,
}
```

- **Endpoint:** `{base_url}/v1/chat/completions`. `base_url` is trimmed
  of any trailing slash; `/v1` is appended only if absent (tolerate both
  `http://host:8080` and `http://host:8080/v1`).
- **Auth:** `Authorization: Bearer {api_key}` on every request.
- **Request build:** port `build_request_body` from `github_copilot.rs`
  — system + messages, `tools` array (OpenAI `{"type":"function", …}`
  shape), `max_tokens`, `temperature`. `model` is passed through
  **verbatim** (server path strings are valid model ids here).
- **Tool-call parsing:** reuse copilot's non-streaming
  (`choices[].message.tool_calls`) and streaming
  (`delta.tool_calls` index-accumulation) parsers, including
  `finish_reason == "tool_calls"` → `StopReason::ToolUse`.
- **`stream`:** when `true`, POST with `"stream": true` and parse SSE;
  when `false`, single POST and parse the one JSON body. Default `true`;
  config can force `false` for servers without SSE.
- **`list_models()`:** return the config-declared `models` (falling back
  to a single entry for `default_model`). A best-effort `GET
  {base_url}/v1/models` MAY enrich this, but config is authoritative and
  the call must not fail provider construction.
- **`default_model()` / `provider_id()`:** return the configured default
  and a stable id. `provider_id()` needs a value of the
  `rupu-providers::ProviderId` enum; we add an
  `OpenAiCompatible` variant there for telemetry/pricing-key purposes
  (the *name* string carries the actual instance identity through usage
  records).

The factored-out request/response/SSE helpers shared with
`github_copilot.rs` move to a small internal `openai_wire` helper module
so both clients call one implementation rather than copy-pasting (DRY;
copilot keeps its token-exchange layer on top).

### 2. Config schema

Reuse the existing `[providers.<name>]` map. Add one field to
`ProviderConfig`:

```toml
[providers.oracle]
kind = "openai-compatible"                       # NEW discriminator
base_url = "http://192.29.35.246:8080"
default_model = "/raid/models/zai-org/GLM-5.2-FP8"
stream = true                                    # optional, default true
api_key_env = "ORACLE_API_KEY"                   # optional env fallback name

  [[providers.oracle.models]]
  id = "/raid/models/zai-org/GLM-5.2-FP8"
  context_window = 131072
  max_output = 8192
```

- `kind: Option<String>` — `None` means "knob overrides for a built-in
  provider" (today's behavior, unchanged). `Some("openai-compatible")`
  marks a generic endpoint. Explicit and self-documenting; no inference
  from the presence of `base_url`.
- `stream: Option<bool>` and `api_key_env: Option<String>` added to
  `ProviderConfig`; both optional.
- Existing `base_url`, `default_model`, `models` are reused as-is.

### 3. Secrets in `auth.json` + env fallback

`auth.json`'s on-disk format already accepts arbitrary string keys. We
add a **string-keyed credential path parallel to the closed enum**:

- `AuthBackend` gains `store_named(&str, &str)` / `retrieve_named(&str)`
  / `forget_named(&str)`. For `JsonFileBackend` these operate directly on
  the `BTreeMap<String,String>` (trivial). For the keychain backend they
  key on `(service = "rupu", account = <name>)`.
- The resolver's `get()` (`resolver.rs`): when `parse_provider(name)`
  does not match a built-in, fall through to the **named** path —
  retrieve `name` from the backend; if absent, try the env var
  (`api_key_env` if the caller passed one, else `RUPU_<UPPER_NAME>_API_KEY`).
  Return `AuthMode::ApiKey`.
- The resolver stays **config-agnostic**: it never reads `config.toml`.
  Legitimacy of the name (declared vs. typo) is enforced upstream by the
  factory (§4), not by the resolver.

### 4. Wiring: factory + auth CLI

- **`build_for_provider_with_config`** gains a fallback arm. Before
  `_ => UnknownProvider`, if `name` is present in the config
  `providers` map with `kind == "openai-compatible"`, build an
  `OpenAiCompatibleClient` from that `ProviderConfig` (base_url,
  default_model, models, stream) + the key from the resolver. This means
  the factory must **receive the relevant `ProviderConfig`** — we extend
  its `ProviderConfig` param (or pass the resolved entry) so the
  endpoint/model/stream values flow in. Unknown-and-undeclared names
  still return `UnknownProvider`.
- The factory performs the **name-legitimacy check**: only a name that
  appears in `[providers.*]` with the right `kind` reaches the resolver's
  named path. A typo'd name fails fast with `UnknownProvider`.
- **`rupu auth login --provider <name> --mode api-key`**,
  `auth status`, `auth logout` learn to accept a config-declared
  openai-compatible name and route to the named backend path. `status`
  lists configured openai-compatible providers and whether a key is
  present (auth.json or env).

### 5. Models / pricing

- Model id is passed through verbatim; the `models` list supplies
  `context_window` / `max_output`. A sane default context window
  (e.g. 32k) applies when a model is used without a declared entry.
- Pricing: `lookup()` already falls back to `0`/unknown for unlisted
  `(provider, model)` pairs; users may add `[pricing.oracle."…"]`
  entries. No special-casing required.

## Data flow

```
rupu run --provider oracle
  → load Config (global + project layer)
  → factory.build_for_provider_with_config("oracle", model, …, &cfg.providers)
      → name "oracle" found in providers map, kind = openai-compatible   (legitimacy check)
      → resolver.get("oracle")                                            (config-agnostic)
          → parse_provider("oracle") miss → named path
          → auth.json["oracle"]  (else RUPU_ORACLE_API_KEY / api_key_env)
      → OpenAiCompatibleClient::new(base_url, key, default_model, models, stream)
  → agent loop: send()/stream() → POST {base_url}/v1/chat/completions  (Bearer, tools)
```

## Error handling

- Missing key: `MissingCredential` with the existing hint, plus mention
  of the env-var fallback name and `rupu auth login --provider <name>`.
- Declared `kind = "openai-compatible"` but no `base_url`: config
  validation error at load (`base_url` is required for that kind).
- Endpoint unreachable / non-2xx: surface status + body snippet via
  `ProviderError`, same as the other HTTP clients.
- SSE not supported by server (e.g. 400 on `stream:true`): documented
  remedy is `stream = false`; we do not auto-detect in this slice.

## Testing

- **Unit (rupu-providers):** request-body build (system/messages/tools,
  verbatim model, max_tokens/temperature); non-streaming tool-call parse;
  streaming SSE tool-call accumulation; base_url normalization
  (`/v1` present vs absent, trailing slash). Mirror the existing
  `github_copilot.rs` test shapes.
- **Unit (rupu-config):** round-trip `kind`/`stream`/`api_key_env`;
  `kind="openai-compatible"` without `base_url` is rejected.
- **Unit (rupu-auth):** `store_named`/`retrieve_named`/`forget_named`
  round-trip on the JSON backend; resolver named fallback (auth.json hit,
  env-var hit, both-miss error).
- **Unit (rupu-runtime):** factory builds an `OpenAiCompatibleClient` for
  a declared name; rejects an undeclared name; the mock-script seam still
  short-circuits.
- **Live smoke (ignored by default):** behind `RUPU_LIVE_OPENAI_COMPAT`
  env, hit a real endpoint for a single non-streaming completion. Not run
  in CI.

## Open questions

None blocking. Keychain storage for named providers and an optional
`GET /v1/models` enrichment are deliberate follow-ups, not part of this
slice.
