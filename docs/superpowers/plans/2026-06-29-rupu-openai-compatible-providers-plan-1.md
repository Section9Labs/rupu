# Generic OpenAI-compatible Providers — Plan 1 (client, config, auth, `rupu run` path)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user declare an OpenAI-compatible inference endpoint (Oracle GenAI, vLLM, etc.) in config, store its Bearer key in `auth.json`, and run a `rupu` agent against it with full tool-calling.

**Architecture:** A new `OpenAiCompatibleClient` in `rupu-providers` speaks OpenAI `/v1/chat/completions` (request/response/SSE helpers shared with the existing Copilot client via a new `openai_wire` module). Instances are declared in the existing `[providers.<name>]` config map with `kind = "openai-compatible"` + `base_url`. The static Bearer key lives in `auth.json` under the provider name (a string-keyed credential path added parallel to `rupu-auth`'s closed `ProviderId` enum), with a `RUPU_<NAME>_API_KEY` env fallback. The runtime factory grows an openai-compatible branch driven by config knobs, and `rupu run` resolves the provider name against config before building.

**Tech Stack:** Rust 2021, tokio, reqwest, serde_json, async-trait, thiserror, anyhow (CLI), insta (n/a here).

**Spec:** `docs/superpowers/specs/2026-06-29-rupu-openai-compatible-providers-design.md`

## Global Constraints

- Workspace deps only — never add a version to a crate `Cargo.toml`; all versions pinned in root `Cargo.toml`. (No new external crates are introduced by this plan.)
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-cli` stays thin: arg parsing + delegation only; no business logic.
- Hexagonal separation: the agent runtime knows only traits; the new client implements `rupu_providers::provider::LlmProvider`.
- Errors: `thiserror` in libraries, `anyhow` in the CLI binary.
- Per the rustfmt-drift memo: never run workspace-wide `cargo fmt`; format only files you touched (`cargo fmt -p <crate>` is fine, or `rustfmt <file>`).
- The resolver stays **config-agnostic** — it never reads `config.toml`. Name legitimacy is enforced by the caller/factory.
- **Out of scope for Plan 1 (→ Plan 2):** workflow-step (`step_factory.rs`), subagent-dispatch (`dispatch.rs`), and definition-generator (`generate.rs`) paths, which call `build_for_provider` without a loaded `Config`. They will return `UnknownProvider` for an openai-compatible name until Plan 2 threads config in. Also deferred: OS-keychain UX polish for named providers (the code path works via the default JSON backend), and a config-named `api_key_env` override (Plan 1 uses the `RUPU_<NAME>_API_KEY` convention).

---

### Task 1: Extract `openai_wire` — shared OpenAI chat-completions helpers

Factor the generic (non-Copilot-specific) request/response/SSE logic out of `github_copilot.rs` into a new module so the new client reuses one implementation. Pure functions; Copilot keeps its token-exchange + headers on top.

**Files:**
- Create: `crates/rupu-providers/src/openai_wire.rs`
- Modify: `crates/rupu-providers/src/lib.rs` (add `pub mod openai_wire;`)
- Modify: `crates/rupu-providers/src/github_copilot.rs` (delete the moved items; call the shared helpers)

**Interfaces:**
- Produces (all `pub(crate)`):
  - `fn build_chat_request_body(request: &LlmRequest, stream: bool) -> serde_json::Value`
  - `fn parse_chat_completion(json: &serde_json::Value) -> Result<LlmResponse, ProviderError>`
  - `struct ToolCallAcc { id: String, name: String, arguments: String }` (derives `Default`)
  - `struct CompletionAccumulator { … }` with `fn new() -> Self`, `fn into_response(self) -> Option<LlmResponse>`
  - `fn process_completion_sse(event: &crate::sse::SseEvent, acc: &mut CompletionAccumulator, on_event: &mut (dyn FnMut(StreamEvent) + Send)) -> Result<(), ProviderError>`

- [ ] **Step 1: Create `openai_wire.rs` by moving the generic helpers verbatim from `github_copilot.rs`**

Create `crates/rupu-providers/src/openai_wire.rs` with the module doc and the items below. This is the existing Copilot code, moved unchanged except: `build_request_body` becomes a free function `build_chat_request_body` (it never used `self`), and `process_completion_sse`'s `on_event` parameter is widened to the trait-object form `&mut (dyn FnMut(StreamEvent) + Send)` so both clients can call it.

```rust
//! Shared OpenAI `/v1/chat/completions` wire helpers.
//!
//! Pure request-build / response-parse / SSE-accumulate logic used by every
//! provider that speaks the OpenAI chat-completions dialect (GitHub Copilot,
//! the generic OpenAI-compatible client). Provider-specific concerns —
//! base URL, auth headers, token exchange — stay in the individual clients.

use crate::error::ProviderError;
use crate::types::{ContentBlock, LlmRequest, LlmResponse, Role, StopReason, StreamEvent, Usage};

/// Build an OpenAI chat-completions request body from an `LlmRequest`.
pub(crate) fn build_chat_request_body(request: &LlmRequest, stream: bool) -> serde_json::Value {
    let mut messages = Vec::new();

    if let Some(system) = &request.system {
        messages.push(serde_json::json!({"role": "system", "content": system}));
    }

    for msg in &request.messages {
        match &msg.content[..] {
            [ContentBlock::Text { text }] => {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                messages.push(serde_json::json!({"role": role, "content": text}));
            }
            [ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            }] => {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
            blocks => {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => text_parts.push(text.clone()),
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": { "name": name, "arguments": input.to_string() }
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content,
                            }));
                            continue;
                        }
                    }
                }

                if !text_parts.is_empty() || !tool_calls.is_empty() {
                    let mut msg_json =
                        serde_json::json!({"role": role, "content": text_parts.join("\n")});
                    if !tool_calls.is_empty() {
                        msg_json["tool_calls"] = serde_json::json!(tool_calls);
                    }
                    messages.push(msg_json);
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
        "stream": stream,
    });

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
        body["tool_choice"] = serde_json::json!("auto");
    }

    if let Some(level) = &request.thinking {
        use crate::model_tier::ThinkingLevel;
        let effort = match level {
            ThinkingLevel::Auto => None,
            ThinkingLevel::Minimal => Some("minimal"),
            ThinkingLevel::Low => Some("low"),
            ThinkingLevel::Medium => Some("medium"),
            ThinkingLevel::High => Some("high"),
            ThinkingLevel::Max => Some("xhigh"),
        };
        if let Some(e) = effort {
            body["reasoning_effort"] = serde_json::json!(e);
        }
    }

    body
}

/// Parse a non-streaming chat-completions response into an `LlmResponse`.
pub(crate) fn parse_chat_completion(
    json: &serde_json::Value,
) -> Result<LlmResponse, ProviderError> {
    let id = json["id"].as_str().unwrap_or("").to_string();
    let model = json["model"].as_str().unwrap_or("").to_string();

    let mut content = Vec::new();
    let mut stop_reason = Some(StopReason::EndTurn);

    if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(text) = choice
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }
            }

            if let Some(tool_calls) = choice
                .get("message")
                .and_then(|m| m.get("tool_calls"))
                .and_then(|t| t.as_array())
            {
                for tc in tool_calls {
                    let tc_id = tc["id"].as_str().unwrap_or("").to_string();
                    let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let input: serde_json::Value =
                        serde_json::from_str(args_str).map_err(|e| {
                            ProviderError::Json(format!(
                                "malformed tool arguments for '{name}': {e}"
                            ))
                        })?;
                    content.push(ContentBlock::ToolUse {
                        id: tc_id,
                        name,
                        input,
                    });
                    stop_reason = Some(StopReason::ToolUse);
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                stop_reason = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                });
            }
        }
    }

    let usage = if let Some(u) = json.get("usage") {
        Usage {
            input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        }
    } else {
        Usage::default()
    };

    Ok(LlmResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

#[derive(Default)]
pub(crate) struct ToolCallAcc {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub(crate) struct CompletionAccumulator {
    pub id: String,
    pub model: String,
    pub text: String,
    pub tool_calls: Vec<ToolCallAcc>,
    pub stop_reason: Option<StopReason>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl CompletionAccumulator {
    pub(crate) fn new() -> Self {
        Self {
            id: String::new(),
            model: String::new(),
            text: String::new(),
            tool_calls: Vec::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    pub(crate) fn into_response(self) -> Option<LlmResponse> {
        if self.id.is_empty() && self.text.is_empty() && self.tool_calls.is_empty() {
            return None;
        }
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        for tc in self.tool_calls {
            if tc.name.is_empty() {
                continue;
            }
            let input: serde_json::Value =
                serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.name,
                input,
            });
        }
        Some(LlmResponse {
            id: self.id,
            model: self.model,
            content,
            stop_reason: self.stop_reason,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                ..Default::default()
            },
        })
    }
}

/// Fold one chat-completions SSE event into the accumulator.
pub(crate) fn process_completion_sse(
    event: &crate::sse::SseEvent,
    acc: &mut CompletionAccumulator,
    on_event: &mut (dyn FnMut(StreamEvent) + Send),
) -> Result<(), ProviderError> {
    if event.data == "[DONE]" {
        return Ok(());
    }
    let data: serde_json::Value = serde_json::from_str(&event.data)?;

    if let Some(id) = data["id"].as_str() {
        if acc.id.is_empty() {
            acc.id = id.to_string();
        }
    }
    if let Some(model) = data["model"].as_str() {
        if acc.model.is_empty() {
            acc.model = model.to_string();
        }
    }

    if let Some(choices) = data.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            let delta = &choice["delta"];

            if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                acc.text.push_str(text);
                on_event(StreamEvent::TextDelta(text.to_string()));
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                            let tc_id = tc["id"].as_str().unwrap_or("").to_string();
                            while acc.tool_calls.len() <= idx {
                                acc.tool_calls.push(ToolCallAcc::default());
                            }
                            acc.tool_calls[idx].id = tc_id.clone();
                            acc.tool_calls[idx].name = name.to_string();
                            on_event(StreamEvent::ToolUseStart {
                                id: tc_id,
                                name: name.to_string(),
                            });
                        }
                        if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                            while acc.tool_calls.len() <= idx {
                                acc.tool_calls.push(ToolCallAcc::default());
                            }
                            acc.tool_calls[idx].arguments.push_str(args);
                            on_event(StreamEvent::InputJsonDelta(args.to_string()));
                        }
                    }
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                acc.stop_reason = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::EndTurn,
                });
            }
        }
    }

    if let Some(u) = data.get("usage") {
        if let Some(input) = u.get("prompt_tokens").and_then(|v| v.as_u64()) {
            acc.input_tokens = input as u32;
        }
        if let Some(output) = u.get("completion_tokens").and_then(|v| v.as_u64()) {
            acc.output_tokens = output as u32;
        }
        on_event(StreamEvent::UsageSnapshot(Usage {
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cached_tokens: 0,
        }));
    }

    Ok(())
}
```

- [ ] **Step 2: Register the module**

In `crates/rupu-providers/src/lib.rs`, add `pub mod openai_wire;` in alphabetical position (immediately after `pub mod model_tier;` / before `pub mod openai_codex;`).

- [ ] **Step 3: Refactor `github_copilot.rs` to use the shared helpers**

In `crates/rupu-providers/src/github_copilot.rs`:
1. Delete the now-moved items: the `build_request_body` method, the free `parse_chat_completion` fn, the `process_completion_sse` fn, and the `ToolCallAcc` + `CompletionAccumulator` structs/impls.
2. Replace the two `self.build_request_body(request, false/true)` call sites in `send`/`stream` with `crate::openai_wire::build_chat_request_body(request, false/true)`.
3. In `send`, replace `parse_chat_completion(&json)` with `crate::openai_wire::parse_chat_completion(&json)`.
4. In `stream`, replace `let mut acc = CompletionAccumulator::new();` with `let mut acc = crate::openai_wire::CompletionAccumulator::new();` and `process_completion_sse(&event, &mut acc, on_event)?` with `crate::openai_wire::process_completion_sse(&event, &mut acc, on_event)?`. Note `stream`'s `on_event` is already `&mut (dyn FnMut(StreamEvent) + Send)` at the trait-impl boundary; the inherent `stream` method's `on_event: &mut (impl FnMut(StreamEvent) + Send + ?Sized)` coerces to the `dyn` param.
5. Keep `build_headers`, `ensure_valid_token`, the token-exchange code, `make_model_info`, and the copilot-specific tests unchanged.

- [ ] **Step 4: Verify the crate compiles and Copilot tests still pass**

Run: `cargo test -p rupu-providers github_copilot`
Expected: PASS — the existing copilot request-body / parse / SSE tests are now exercising the shared `openai_wire` code unchanged.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p rupu-providers
git add crates/rupu-providers/src/openai_wire.rs crates/rupu-providers/src/lib.rs crates/rupu-providers/src/github_copilot.rs
git commit -m "refactor(providers): extract openai_wire chat-completions helpers from copilot"
```

---

### Task 2: Add `ProviderId::OpenaiCompatible` variant

The `LlmProvider` trait requires `fn provider_id(&self) -> ProviderId`, so the new client needs a stable enum identity. Add the variant and update all exhaustive matches.

**Files:**
- Modify: `crates/rupu-providers/src/provider_id.rs`
- Modify: `crates/rupu-providers/src/registry.rs`
- Modify: `crates/rupu-providers/src/router.rs`
- Modify: `crates/rupu-providers/src/model_tier.rs`
- Modify: `crates/rupu-providers/src/auth/discovery.rs`

**Interfaces:**
- Produces: `ProviderId::OpenaiCompatible` (lowercase `a`, matching the existing `OpenaiCodex` sibling so serde kebab-case → `"openai-compatible"`, consistent with `auth_key()` / `from_str`).

- [ ] **Step 1: Add the variant and update the three `provider_id.rs` matches**

In `crates/rupu-providers/src/provider_id.rs`:

Add to the enum (after `GithubCopilot`):
```rust
    GithubCopilot,
    OpenaiCompatible,
}
```

Add to `ALL`:
```rust
        ProviderId::GithubCopilot,
        ProviderId::OpenaiCompatible,
    ];
```

`auth_key` arm:
```rust
            ProviderId::GithubCopilot => "github-copilot",
            ProviderId::OpenaiCompatible => "openai-compatible",
```

`env_var_name` arm:
```rust
            ProviderId::GithubCopilot => "GITHUB_TOKEN",
            ProviderId::OpenaiCompatible => "OPENAI_COMPATIBLE_API_KEY",
```

`from_str` arm (before the `_` catch-all):
```rust
            "github-copilot" | "copilot" => Ok(ProviderId::GithubCopilot),
            "openai-compatible" => Ok(ProviderId::OpenaiCompatible),
```

- [ ] **Step 2: Update the `registry.rs` exhaustive match**

The OAuth-credential registry does not build openai-compatible clients (they need a `base_url` that the `CredentialSource` doesn't carry — they are built by the runtime factory). Add an arm to `create_provider`'s `match id` that returns a clear error. Add before the closing `}` of the match:
```rust
            ProviderId::OpenaiCompatible => Err(ProviderError::NotImplemented {
                provider: "openai-compatible (built via runtime provider factory, not the OAuth registry)"
                    .to_string(),
            }),
```

- [ ] **Step 3: Update `router.rs::select_model`**

OpenAI-compatible model ids are arbitrary server paths; always pass the requested model through verbatim. Add to the `match provider.provider_id()`:
```rust
            ProviderId::GithubCopilot => true,
            ProviderId::OpenaiCompatible => true,
```

- [ ] **Step 4: Update `model_tier.rs::for_provider`**

Tier→model mapping is unknown for arbitrary endpoints; the configured `default_model` is used directly elsewhere, so the static tier names here are placeholders never consulted for this provider. Add an arm:
```rust
            ProviderId::OpenaiCompatible => Self {
                provider: id,
                fast: "",
                default: "",
                deep_think: "",
                code: "",
            },
```

- [ ] **Step 5: Update `auth/discovery.rs::discover`**

No filesystem auto-discovery for openai-compatible providers (keys come from `auth.json`/env via the resolver). Add:
```rust
            ProviderId::GithubCopilot => discover_github_copilot(),
            ProviderId::OpenaiCompatible => None,
```

- [ ] **Step 6: Verify compilation and existing provider tests**

Run: `cargo test -p rupu-providers provider_id`
Expected: PASS — including the existing `test_serde_roundtrip` / `test_from_str_*` tests, which now also cover the new variant via `ALL`.

- [ ] **Step 7: Commit**

```bash
cargo fmt -p rupu-providers
git add crates/rupu-providers/src/provider_id.rs crates/rupu-providers/src/registry.rs crates/rupu-providers/src/router.rs crates/rupu-providers/src/model_tier.rs crates/rupu-providers/src/auth/discovery.rs
git commit -m "feat(providers): add ProviderId::OpenaiCompatible variant"
```

---

### Task 3: Implement `OpenAiCompatibleClient`

The new provider client: configurable base URL + static Bearer + verbatim model + tool-calling, using `openai_wire`.

**Files:**
- Create: `crates/rupu-providers/src/openai_compatible.rs`
- Modify: `crates/rupu-providers/src/lib.rs` (module + re-export)

**Interfaces:**
- Consumes: `openai_wire::{build_chat_request_body, parse_chat_completion, process_completion_sse, CompletionAccumulator}`; `sse::SseParser`.
- Produces:
  - `pub struct OpenAiCompatibleModel { pub id: String, pub context_window: u32, pub max_output: u32 }`
  - `pub struct OpenAiCompatibleClient`
  - `impl OpenAiCompatibleClient { pub fn new(base_url: &str, api_key: &str, default_model: &str, models: Vec<OpenAiCompatibleModel>, stream: bool) -> Self }`

- [ ] **Step 1: Write the failing unit tests**

Create `crates/rupu-providers/src/openai_compatible.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmRequest, Message, ToolDefinition};

    fn client() -> OpenAiCompatibleClient {
        OpenAiCompatibleClient::new(
            "http://192.29.35.246:8080/",
            "sk-test",
            "/raid/models/zai-org/GLM-5.2-FP8",
            vec![OpenAiCompatibleModel {
                id: "/raid/models/zai-org/GLM-5.2-FP8".into(),
                context_window: 131072,
                max_output: 8192,
            }],
            true,
        )
    }

    #[test]
    fn base_url_normalizes_trailing_slash_and_appends_v1() {
        let c = client();
        assert_eq!(c.completions_url(), "http://192.29.35.246:8080/v1/chat/completions");
    }

    #[test]
    fn base_url_tolerates_explicit_v1() {
        let c = OpenAiCompatibleClient::new(
            "http://host:8080/v1",
            "k",
            "m",
            vec![],
            true,
        );
        assert_eq!(c.completions_url(), "http://host:8080/v1/chat/completions");
    }

    #[test]
    fn request_body_passes_model_verbatim_and_tools() {
        let c = client();
        let req = LlmRequest {
            model: "/raid/models/zai-org/GLM-5.2-FP8".into(),
            messages: vec![Message::user("hi")],
            max_tokens: 2048,
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "read".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            ..Default::default()
        };
        let body = c.request_body(&req, false);
        assert_eq!(body["model"], "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn default_model_and_provider_id() {
        let c = client();
        assert_eq!(c.default_model(), "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(c.provider_id(), crate::provider_id::ProviderId::OpenaiCompatible);
    }

    #[tokio::test]
    async fn list_models_returns_configured_models() {
        let c = client();
        let models = c.list_models().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "/raid/models/zai-org/GLM-5.2-FP8");
        assert_eq!(models[0].context_window, 131072);
        assert_eq!(models[0].provider, crate::provider_id::ProviderId::OpenaiCompatible);
    }
}
```

- [ ] **Step 2: Run the tests to confirm they fail to compile**

Run: `cargo test -p rupu-providers openai_compatible`
Expected: FAIL — `OpenAiCompatibleClient` / `OpenAiCompatibleModel` not defined.

- [ ] **Step 3: Implement the client (above the test module)**

Prepend to `crates/rupu-providers/src/openai_compatible.rs`:

```rust
//! Generic OpenAI-compatible provider.
//!
//! Speaks the OpenAI `/v1/chat/completions` API against a configurable base
//! URL with a static Bearer key. Covers self-hosted vLLM, Oracle GenAI,
//! Together, Fireworks, OpenRouter, and similar endpoints. Wire-format logic
//! is shared with the Copilot client via [`crate::openai_wire`].

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;

use crate::error::ProviderError;
use crate::model_pool::{ModelCapability, ModelCost, ModelInfo, ModelState, ModelStatus};
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::{LlmRequest, LlmResponse, StreamEvent};

/// A model offered by an OpenAI-compatible endpoint, declared in config.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleModel {
    pub id: String,
    pub context_window: u32,
    pub max_output: u32,
}

/// Client for an OpenAI-compatible `/v1/chat/completions` endpoint.
pub struct OpenAiCompatibleClient {
    base_url: String,
    api_key: String,
    default_model: String,
    models: Vec<OpenAiCompatibleModel>,
    stream: bool,
    client: Client,
}

impl OpenAiCompatibleClient {
    /// * `base_url` — endpoint root, with or without a trailing `/v1`
    ///   (e.g. `http://192.29.35.246:8080` or `…/v1`).
    /// * `api_key` — static Bearer key.
    /// * `default_model` — model id sent when the request doesn't override it.
    /// * `models` — config-declared models, surfaced via `list_models`.
    /// * `stream` — when false, never request SSE (servers without it).
    pub fn new(
        base_url: &str,
        api_key: &str,
        default_model: &str,
        models: Vec<OpenAiCompatibleModel>,
        stream: bool,
    ) -> Self {
        // Normalize: strip trailing slashes, then strip a trailing `/v1`
        // so we hold the bare root and append `/v1/...` consistently.
        let trimmed = base_url.trim_end_matches('/');
        let root = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        Self {
            base_url: root.to_string(),
            api_key: api_key.to_string(),
            default_model: default_model.to_string(),
            models,
            stream,
            client: Client::new(),
        }
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    fn request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        crate::openai_wire::build_chat_request_body(request, stream)
    }

    fn headers(&self) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let auth_val = format!("Bearer {}", self.api_key).parse().map_err(|_| {
            ProviderError::AuthConfig("api key contains invalid header characters".into())
        })?;
        headers.insert(reqwest::header::AUTHORIZATION, auth_val);
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "text/event-stream".parse().unwrap(),
        );
        Ok(headers)
    }

    async fn send_inner(&self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let body = self.request_body(request, false);
        let response = self
            .client
            .post(self.completions_url())
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: text.chars().take(500).collect(),
            });
        }
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Json(e.to_string()))?;
        crate::openai_wire::parse_chat_completion(&json)
    }

    async fn stream_inner(
        &self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        let body = self.request_body(request, true);
        let response = self
            .client
            .post(self.completions_url())
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: text.chars().take(500).collect(),
            });
        }
        let mut parser = SseParser::new();
        let mut acc = crate::openai_wire::CompletionAccumulator::new();
        let mut bytes_stream = response.bytes_stream();
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            for event in parser.feed(&chunk)? {
                crate::openai_wire::process_completion_sse(&event, &mut acc, on_event)?;
            }
        }
        acc.into_response().ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn model_info(&self, m: &OpenAiCompatibleModel) -> ModelInfo {
        ModelInfo {
            id: m.id.clone(),
            provider: ProviderId::OpenaiCompatible,
            context_window: m.context_window,
            max_output_tokens: m.max_output,
            capabilities: vec![ModelCapability::ToolUse, ModelCapability::Streaming],
            cost: ModelCost {
                input_per_million: 0.0,
                output_per_million: 0.0,
            },
            status: ModelStatus {
                state: ModelState::Available,
                utilization: None,
                quota_reset: None,
                last_success: None,
                last_error: None,
                consecutive_failures: 0,
            },
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.send_inner(request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        if self.stream {
            self.stream_inner(request, on_event).await
        } else {
            // Server doesn't support SSE — do a blocking send and emit the
            // text as a single delta so callers see consistent events.
            let resp = self.send_inner(request).await?;
            if let Some(text) = resp.text() {
                on_event(StreamEvent::TextDelta(text.to_string()));
            }
            Ok(resp)
        }
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::OpenaiCompatible
    }

    async fn list_models(&self) -> Vec<ModelInfo> {
        self.models.iter().map(|m| self.model_info(m)).collect()
    }
}
```

Note: `completions_url` and `request_body` are `fn` (not `pub`) but referenced from the in-file test module, which can see private items.

- [ ] **Step 4: Register the module and re-export the public types**

In `crates/rupu-providers/src/lib.rs`:
- Add `pub mod openai_compatible;` (alphabetical: after `pub mod openai_codex;`).
- Add re-export near the other `pub use` lines:
  ```rust
  pub use openai_compatible::{OpenAiCompatibleClient, OpenAiCompatibleModel};
  ```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-providers openai_compatible`
Expected: PASS (all 6 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p rupu-providers
git add crates/rupu-providers/src/openai_compatible.rs crates/rupu-providers/src/lib.rs
git commit -m "feat(providers): add OpenAiCompatibleClient (chat/completions + tools + bearer)"
```

---

### Task 4: Config schema — `kind` / `stream` fields + validation

**Files:**
- Modify: `crates/rupu-config/src/provider_config.rs`
- Modify: `crates/rupu-config/src/config.rs` (add `Config::validate`)
- Modify: `crates/rupu-config/src/layer.rs` (call `validate` in `layer_files`)

**Interfaces:**
- Produces: `ProviderConfig.kind: Option<String>`, `ProviderConfig.stream: Option<bool>`; `Config::validate(&self) -> Result<(), LayerError>`.

- [ ] **Step 1: Write failing tests**

Add to `crates/rupu-config/src/provider_config.rs` (in a `#[cfg(test)] mod tests`, creating it if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_kind_and_stream() {
        let toml = r#"
kind = "openai-compatible"
base_url = "http://192.29.35.246:8080"
default_model = "/raid/models/zai-org/GLM-5.2-FP8"
stream = true

[[models]]
id = "/raid/models/zai-org/GLM-5.2-FP8"
context_window = 131072
max_output = 8192
"#;
        let cfg: ProviderConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.kind.as_deref(), Some("openai-compatible"));
        assert_eq!(cfg.stream, Some(true));
        assert_eq!(cfg.base_url.as_deref(), Some("http://192.29.35.246:8080"));
        assert_eq!(cfg.models.len(), 1);
    }
}
```

Add to `crates/rupu-config/src/config.rs` test module:

```rust
    #[test]
    fn validate_rejects_openai_compatible_without_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "oracle".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[test]
    fn validate_accepts_openai_compatible_with_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "oracle".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }
```

(If `config.rs` has no test module yet, add `#[cfg(test)] mod tests { use super::*; … }`.)

- [ ] **Step 2: Run tests to confirm failure**

Run: `cargo test -p rupu-config provider_config && cargo test -p rupu-config validate_`
Expected: FAIL — `kind`/`stream` fields and `validate` don't exist.

- [ ] **Step 3: Add the fields**

In `crates/rupu-config/src/provider_config.rs`, add to `ProviderConfig` (after `base_url`):
```rust
    /// Discriminates a generic OpenAI-compatible endpoint
    /// (`"openai-compatible"`) from per-provider knob overrides (`None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Request SSE streaming. `None`/`Some(true)` → stream; `Some(false)`
    /// forces non-streaming for servers without SSE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
```

- [ ] **Step 4: Add `Config::validate`**

In `crates/rupu-config/src/config.rs`, add an `impl Config` block:
```rust
impl Config {
    /// Validate cross-field invariants not expressible in serde.
    pub fn validate(&self) -> Result<(), crate::layer::LayerError> {
        for (name, p) in &self.providers {
            if p.kind.as_deref() == Some("openai-compatible") && p.base_url.is_none() {
                return Err(crate::layer::LayerError::Invalid(format!(
                    "provider '{name}': kind=\"openai-compatible\" requires base_url"
                )));
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Add the `LayerError::Invalid` variant and call `validate` in `layer_files`**

In `crates/rupu-config/src/layer.rs`:
- Add a variant to `LayerError`:
  ```rust
      #[error("invalid config: {0}")]
      Invalid(String),
  ```
- In `layer_files`, after `let cfg: Config = merged.try_into().map_err(...)?;` and before `Ok(cfg)`, insert:
  ```rust
      cfg.validate()?;
  ```

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p rupu-config`
Expected: PASS (new tests + existing config tests).

- [ ] **Step 7: Commit**

```bash
cargo fmt -p rupu-config
git add crates/rupu-config/src/provider_config.rs crates/rupu-config/src/config.rs crates/rupu-config/src/layer.rs
git commit -m "feat(config): openai-compatible provider kind/stream fields + validation"
```

---

### Task 5: Resolver named-credential fallback (auth.json + env)

Generalize `KeychainResolver` so an undeclared name resolves to `auth.json["<name>/api-key"]` (or legacy `["<name>"]`), then `RUPU_<UPPER_NAME>_API_KEY`. Refactor `read` to an account-string core so both backends are reused.

**Files:**
- Modify: `crates/rupu-auth/src/resolver.rs`

**Interfaces:**
- Produces (on `KeychainResolver`):
  - `pub async fn store_named(&self, name: &str, mode: AuthMode, sc: &StoredCredential) -> Result<()>`
  - `pub async fn forget_named(&self, name: &str, mode: AuthMode) -> Result<()>`
  - `pub async fn peek_named(&self, name: &str, mode: AuthMode) -> bool`
  - extended `get()` fallback for names `parse_provider` rejects.

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/rupu-auth/src/resolver.rs`:

```rust
    #[tokio::test]
    async fn named_provider_reads_from_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{ "oracle/api-key": "sk-oracle-123" }"#).unwrap();
        std::env::set_var("RUPU_AUTH_FILE", &path);
        std::env::set_var("RUPU_AUTH_BACKEND", "file");
        let r = KeychainResolver::new();
        let (mode, creds) = r.get("oracle", None).await.unwrap();
        assert_eq!(mode, rupu_providers::AuthMode::ApiKey);
        match creds {
            rupu_providers::auth::AuthCredentials::ApiKey { key } => {
                assert_eq!(key, "sk-oracle-123")
            }
            _ => panic!("expected api key"),
        }
        std::env::remove_var("RUPU_AUTH_FILE");
        std::env::remove_var("RUPU_AUTH_BACKEND");
    }

    #[tokio::test]
    async fn named_provider_falls_back_to_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::env::set_var("RUPU_AUTH_FILE", &path);
        std::env::set_var("RUPU_AUTH_BACKEND", "file");
        std::env::set_var("RUPU_ACME_API_KEY", "sk-env-456");
        let r = KeychainResolver::new();
        let (_mode, creds) = r.get("acme", None).await.unwrap();
        match creds {
            rupu_providers::auth::AuthCredentials::ApiKey { key } => {
                assert_eq!(key, "sk-env-456")
            }
            _ => panic!("expected api key"),
        }
        std::env::remove_var("RUPU_AUTH_FILE");
        std::env::remove_var("RUPU_AUTH_BACKEND");
        std::env::remove_var("RUPU_ACME_API_KEY");
    }
```

(Confirm `tempfile` is already a dev-dependency of `rupu-auth`; the existing resolver tests use temp dirs. If absent, it is a workspace dev-dep — add `tempfile.workspace = true` under `[dev-dependencies]` in `crates/rupu-auth/Cargo.toml`.)

- [ ] **Step 2: Run tests to confirm failure**

Run: `cargo test -p rupu-auth named_provider`
Expected: FAIL — `get("oracle")` currently bails with "unknown provider".

- [ ] **Step 3: Refactor `read` onto an account-string core**

In `crates/rupu-auth/src/resolver.rs`, add a method that takes the account base string instead of a `ProviderId`, and re-express `read` in terms of it. Insert alongside `read`:

```rust
    /// Read a credential by its account *base* string (e.g. `"oracle"` or a
    /// built-in's `as_str()`), composing the `<base>/<mode>` keychain account
    /// exactly like [`key_for`]. The `legacy_base` (if any) is the bare
    /// account tried for api-key entries written before the mode suffix.
    fn read_account(
        &self,
        account_base: &str,
        legacy_base: Option<&str>,
        mode: AuthMode,
    ) -> Result<Option<StoredCredential>> {
        let account = format!("{account_base}/{}", mode.as_str());
        match &self.storage {
            Storage::Keyring { service } => {
                #[cfg(target_os = "macos")]
                {
                    match rupu_keychain_acl::get_generic_password(service, &account) {
                        Ok(bytes) => {
                            let s = String::from_utf8(bytes)
                                .map_err(|e| anyhow::anyhow!("keychain read: {e}"))?;
                            Ok(Some(parse_stored_credential(&s, mode)?))
                        }
                        Err(rupu_keychain_acl::AclError::NotFound { .. }) => {
                            if mode == AuthMode::ApiKey {
                                if let Some(lb) = legacy_base {
                                    if let Ok(bytes) =
                                        rupu_keychain_acl::get_generic_password(service, lb)
                                    {
                                        let s = String::from_utf8(bytes).map_err(|e| {
                                            anyhow::anyhow!("keychain legacy read: {e}")
                                        })?;
                                        return Ok(Some(StoredCredential::api_key(s)));
                                    }
                                }
                            }
                            Ok(None)
                        }
                        Err(e) => Err(anyhow::anyhow!("keychain read: {e}")),
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let key = KeychainKey {
                        service: service.clone(),
                        account: account.clone(),
                    };
                    match self.entry(&key)?.get_password() {
                        Ok(s) => Ok(Some(parse_stored_credential(&s, mode)?)),
                        Err(keyring::Error::NoEntry) => {
                            if mode == AuthMode::ApiKey {
                                if let Some(lb) = legacy_base {
                                    let lk = KeychainKey {
                                        service: service.clone(),
                                        account: lb.to_string(),
                                    };
                                    if let Ok(s) = self.entry(&lk)?.get_password() {
                                        return Ok(Some(StoredCredential::api_key(s)));
                                    }
                                }
                            }
                            Ok(None)
                        }
                        Err(e) => Err(anyhow::anyhow!("keychain read: {e}")),
                    }
                }
            }
            Storage::JsonFile { path } => {
                let map = Self::read_file_map(path)?;
                if let Some(s) = map.get(&account) {
                    return Ok(Some(parse_stored_credential(s, mode)?));
                }
                if mode == AuthMode::ApiKey {
                    if let Some(lb) = legacy_base {
                        if let Some(legacy) = map.get(lb) {
                            return Ok(Some(StoredCredential::api_key(legacy.clone())));
                        }
                    }
                }
                Ok(None)
            }
        }
    }
```

Then change the body of `read(&self, p, mode)` to delegate:
```rust
    fn read(&self, p: ProviderId, mode: AuthMode) -> Result<Option<StoredCredential>> {
        let legacy = legacy_key_for(p);
        self.read_account(p.as_str(), Some(&legacy.account), mode)
    }
```

(`KeychainKey` is already in scope via `use ... keychain_layout::{key_for, legacy_key_for, KeychainKey}`; confirm the import and add `KeychainKey` if missing.)

- [ ] **Step 4: Add the named store/forget/peek helpers and the `get` fallback**

Add to the inherent `impl KeychainResolver`:

```rust
    /// Store an api-key/SSO credential under an arbitrary provider *name*
    /// (used for config-declared OpenAI-compatible providers).
    pub async fn store_named(
        &self,
        name: &str,
        mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        let payload = serde_json::to_string(sc)?;
        self.write_account(&account, &payload)
    }

    /// Forget a named credential. No-op if absent.
    pub async fn forget_named(&self, name: &str, mode: AuthMode) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        self.delete_account(&account)
    }

    /// True if a named credential exists for `name`/`mode`.
    pub async fn peek_named(&self, name: &str, mode: AuthMode) -> bool {
        self.read_account(name, None, mode)
            .map(|o| o.is_some())
            .unwrap_or(false)
    }
```

If `write_account` / `delete_account` helpers don't already exist, add them by generalizing the existing `store`/`forget` write paths (the existing `store(p, mode, sc)` and `forget` methods compute `key_for(p, mode).account`; extract their inner write/delete to take an `&account: &str`). Then re-express `store`/`forget` to call `write_account(&key_for(p,mode).account, …)` / `delete_account(&key_for(p,mode).account)`.

Extend `get`'s opening so a name that isn't a built-in falls through to the named path. Replace:
```rust
        let p = Self::parse_provider(provider)?;
```
with:
```rust
        let p = match Self::parse_provider(provider) {
            Ok(p) => p,
            Err(_) => return self.get_named(provider).await,
        };
```

And add the helper:
```rust
    /// Resolve a config-declared provider name to an api-key credential:
    /// `auth.json["<name>/api-key"]` (or legacy `["<name>"]`), then
    /// `RUPU_<UPPER_NAME>_API_KEY`.
    async fn get_named(&self, provider: &str) -> Result<(AuthMode, AuthCredentials)> {
        if let Some(sc) = self.read_account(provider, Some(provider), AuthMode::ApiKey)? {
            return Ok((AuthMode::ApiKey, sc.credentials));
        }
        let env_name = format!("RUPU_{}_API_KEY", provider.to_ascii_uppercase());
        if let Ok(key) = std::env::var(&env_name) {
            if !key.is_empty() {
                return Ok((AuthMode::ApiKey, AuthCredentials::ApiKey { key }));
            }
        }
        anyhow::bail!(
            "no credentials for '{provider}'. Run: rupu auth login --provider {provider} \
             --mode api-key, or set {env_name}"
        )
    }
```

(`AuthCredentials` is already imported in this file via the `get` return type; confirm `use rupu_providers::auth::AuthCredentials;` exists at the top — it is used by the trait signature — and add if needed.)

- [ ] **Step 5: Run tests to verify pass + no regressions**

Run: `cargo test -p rupu-auth`
Expected: PASS — the two new tests plus all existing resolver tests (built-in providers still go through `read`→`read_account` unchanged).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p rupu-auth
git add crates/rupu-auth/src/resolver.rs crates/rupu-auth/Cargo.toml
git commit -m "feat(auth): named-provider credential fallback (auth.json + RUPU_<NAME>_API_KEY)"
```

---

### Task 6: Factory openai-compatible branch + config resolution helper

Give the factory the data to build the new client and a helper that turns a `ProviderConfig` entry into client params.

**Files:**
- Modify: `crates/rupu-runtime/src/provider_factory.rs`

**Interfaces:**
- Consumes: `rupu_providers::{OpenAiCompatibleClient, OpenAiCompatibleModel}`; `rupu_config::ProviderConfig`.
- Produces:
  - `ProviderConfig` (the factory's knobs struct) gains `pub openai_compatible: Option<OpenAiCompatibleParams>`.
  - `pub struct OpenAiCompatibleParams { pub base_url: String, pub default_model: String, pub stream: bool, pub models: Vec<rupu_providers::OpenAiCompatibleModel> }`
  - `pub fn openai_compatible_params(name: &str, providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>) -> Option<OpenAiCompatibleParams>`
  - `pub fn is_builtin_provider(name: &str) -> bool`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/rupu-runtime/src/provider_factory.rs`:

```rust
    #[test]
    fn resolves_openai_compatible_params_from_config() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://192.29.35.246:8080".into()),
                default_model: Some("/raid/models/zai-org/GLM-5.2-FP8".into()),
                stream: Some(false),
                models: vec![rupu_config::CustomModel {
                    id: "/raid/models/zai-org/GLM-5.2-FP8".into(),
                    context_window: Some(131072),
                    max_output: Some(8192),
                }],
                ..Default::default()
            },
        );
        let p = openai_compatible_params("oracle", &providers).unwrap();
        assert_eq!(p.base_url, "http://192.29.35.246:8080");
        assert_eq!(p.default_model, "/raid/models/zai-org/GLM-5.2-FP8");
        assert!(!p.stream);
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].context_window, 131072);
        // A name without kind=openai-compatible yields None.
        assert!(openai_compatible_params("anthropic", &providers).is_none());
    }

    #[test]
    fn is_builtin_recognizes_known_names() {
        assert!(is_builtin_provider("anthropic"));
        assert!(is_builtin_provider("copilot"));
        assert!(!is_builtin_provider("oracle"));
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p rupu-runtime provider_factory`
Expected: FAIL — symbols not defined.

- [ ] **Step 3: Add the params struct, knobs field, and helpers**

In `crates/rupu-runtime/src/provider_factory.rs`:

Add the field to the existing `ProviderConfig`:
```rust
pub struct ProviderConfig {
    pub anthropic_oauth_system_prefix: Option<bool>,
    /// Present when the provider name resolves to a config-declared
    /// OpenAI-compatible endpoint. Populated by callers that have a
    /// loaded `rupu_config::Config` (e.g. `rupu run`).
    pub openai_compatible: Option<OpenAiCompatibleParams>,
}
```

Add the params + helpers:
```rust
/// Everything the factory needs to build an `OpenAiCompatibleClient`,
/// resolved from a `[providers.<name>]` config entry.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleParams {
    pub base_url: String,
    pub default_model: String,
    pub stream: bool,
    pub models: Vec<rupu_providers::OpenAiCompatibleModel>,
}

const DEFAULT_OAI_CONTEXT_WINDOW: u32 = 32_768;
const DEFAULT_OAI_MAX_OUTPUT: u32 = 8_192;

/// Resolve `[providers.<name>]` into params iff it declares
/// `kind = "openai-compatible"` with a `base_url`. Returns `None` otherwise.
pub fn openai_compatible_params(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> Option<OpenAiCompatibleParams> {
    let p = providers.get(name)?;
    if p.kind.as_deref() != Some("openai-compatible") {
        return None;
    }
    let base_url = p.base_url.clone()?;
    let default_model = p.default_model.clone().unwrap_or_default();
    let models = p
        .models
        .iter()
        .map(|m| rupu_providers::OpenAiCompatibleModel {
            id: m.id.clone(),
            context_window: m.context_window.unwrap_or(DEFAULT_OAI_CONTEXT_WINDOW),
            max_output: m.max_output.unwrap_or(DEFAULT_OAI_MAX_OUTPUT),
        })
        .collect();
    Some(OpenAiCompatibleParams {
        base_url,
        default_model,
        stream: p.stream.unwrap_or(true),
        models,
    })
}

/// True for provider names the factory builds directly (not openai-compatible).
pub fn is_builtin_provider(name: &str) -> bool {
    matches!(
        name,
        "anthropic"
            | "openai"
            | "openai_codex"
            | "codex"
            | "gemini"
            | "google_gemini"
            | "copilot"
            | "github_copilot"
    )
}
```

Because the knobs `ProviderConfig` now has a non-`Option`-defaulted field added, confirm every constructor in the codebase uses struct-update syntax or update them. The known constructors set only `anthropic_oauth_system_prefix`; update each to add `openai_compatible: None`. Sites (from the call-site survey): `run.rs:133`, `session.rs:6068`, `session.rs:6360`, `session.rs:6623`. (Task 7 changes `run.rs` to populate it; the three `session.rs` sites get `openai_compatible: None`.) Alternatively add `#[derive(Default)]`-friendly construction — but explicit is clearer here.

- [ ] **Step 4: Add the build branch**

In `build_for_provider_with_config`, change the final wildcard arm of `match name`:
```rust
        "local" => return Err(FactoryError::NotWiredInV0("local".to_string())),
        _ => {
            if let Some(params) = &config.openai_compatible {
                let key = match &creds {
                    rupu_providers::auth::AuthCredentials::ApiKey { key } => key.clone(),
                    rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access.clone(),
                };
                Box::new(rupu_providers::OpenAiCompatibleClient::new(
                    &params.base_url,
                    &key,
                    &params.default_model,
                    params.models.clone(),
                    params.stream,
                )) as Box<dyn LlmProvider>
            } else {
                return Err(FactoryError::UnknownProvider(name.to_string()));
            }
        }
```

- [ ] **Step 5: Update the three `session.rs` knobs constructors**

In `crates/rupu-cli/src/cmd/session.rs` at lines ~6068, ~6360, ~6623, add `openai_compatible: None,` to each `provider_factory::ProviderConfig { … }` literal.

- [ ] **Step 6: Run tests + build dependents**

Run: `cargo test -p rupu-runtime provider_factory && cargo build -p rupu-cli`
Expected: PASS / builds (session.rs literals now complete).

- [ ] **Step 7: Commit**

```bash
cargo fmt -p rupu-runtime
git add crates/rupu-runtime/src/provider_factory.rs crates/rupu-cli/src/cmd/session.rs
git commit -m "feat(runtime): factory branch + config resolution for openai-compatible providers"
```

---

### Task 7: Wire `rupu run` — resolve provider name against config

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs` (around lines 126-141)

**Interfaces:**
- Consumes: `provider_factory::{openai_compatible_params, is_builtin_provider, ProviderConfig, OpenAiCompatibleParams}`; `cfg.providers`.

- [ ] **Step 1: Populate the knobs and validate the name**

In `crates/rupu-cli/src/cmd/run.rs`, replace the block at lines ~126-141 (`let provider_name = …` through the factory call) with:

```rust
    let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
    let oai_params = provider_factory::openai_compatible_params(&provider_name, &cfg.providers);
    if !provider_factory::is_builtin_provider(&provider_name) && oai_params.is_none() {
        anyhow::bail!(
            "provider '{provider_name}' is not a built-in provider and is not declared as \
             [providers.{provider_name}] with kind = \"openai-compatible\" in config.toml"
        );
    }
    // For an openai-compatible provider, prefer its configured default_model
    // when the agent/spec didn't pin one.
    let model = spec
        .model
        .clone()
        .or_else(|| cfg.default_model.clone())
        .or_else(|| oai_params.as_ref().map(|p| p.default_model.clone()))
        .unwrap_or_else(|| "claude-sonnet-4-6".into());
    let auth_hint = spec.auth;
    let provider_config = provider_factory::ProviderConfig {
        anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
        openai_compatible: oai_params,
    };
    let (_resolved_auth, provider) = provider_factory::build_for_provider_with_config(
        &provider_name,
        &model,
        auth_hint,
        &resolver,
        &provider_config,
    )
    .await?;
```

- [ ] **Step 2: Build**

Run: `cargo build -p rupu-cli`
Expected: builds clean.

- [ ] **Step 3: Manual smoke against a mock provider script (no network)**

The factory honors `RUPU_MOCK_PROVIDER_SCRIPT` and short-circuits before credential/branch logic, so this verifies the run path wiring without an endpoint. Create a minimal agent + config under a temp `RUPU_HOME` and confirm `rupu run` reaches the agent loop:

Run:
```bash
cargo run -p rupu-cli -- --help | grep -A2 "run"
```
Expected: the `run` subcommand is listed (sanity that the binary built and dispatches). Full end-to-end against a live Oracle endpoint is exercised in Task 9.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p rupu-cli
git add crates/rupu-cli/src/cmd/run.rs
git commit -m "feat(cli): rupu run resolves openai-compatible providers from config"
```

---

### Task 8: `rupu auth` accepts config-declared openai-compatible names

Let `login` / `logout` / `status` operate on a config-declared openai-compatible provider via the resolver's named path.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`

**Interfaces:**
- Consumes: `KeychainResolver::{store_named, forget_named, peek_named}`; `rupu_config::layer_files`; `provider_factory::openai_compatible_params`.

- [ ] **Step 1: Add a config loader + openai-compatible name check helper**

In `crates/rupu-cli/src/cmd/auth.rs`, add:
```rust
/// Load layered config and return true if `name` is a declared
/// openai-compatible provider.
fn is_openai_compatible_name(name: &str) -> bool {
    let Ok(global) = crate::paths::global_dir() else {
        return false;
    };
    let global_cfg = global.join("config.toml");
    let cfg = match rupu_config::layer_files(Some(&global_cfg), None) {
        Ok(c) => c,
        Err(_) => return false,
    };
    rupu_runtime::provider_factory::openai_compatible_params(name, &cfg.providers).is_some()
}
```

(Confirm `rupu-cli` already depends on `rupu-runtime` and `rupu-config` — it does, per existing call sites. No `Cargo.toml` change.)

- [ ] **Step 2: Route `login` for named providers**

In `login`, before `let pid = parse_provider(provider)?;`, branch:
```rust
    if is_openai_compatible_name(provider) {
        let AuthModeArg::ApiKey = mode else {
            anyhow::bail!("openai-compatible providers only support --mode api-key");
        };
        let secret = match key {
            Some(k) => k.to_string(),
            None => {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf.trim().to_string()
            }
        };
        if secret.is_empty() {
            anyhow::bail!("empty API key");
        }
        let resolver = rupu_auth::resolver::KeychainResolver::new();
        let sc = rupu_auth::stored::StoredCredential::api_key(secret);
        resolver
            .store_named(provider, rupu_providers::AuthMode::ApiKey, &sc)
            .await?;
        println!("rupu: stored {provider} api-key credential");
        return Ok(());
    }
    let pid = parse_provider(provider)?;
```

- [ ] **Step 3: Route `logout` for named providers**

In `logout`, after resolving `provider` (the non-`--all` branch) and before `let pid = parse_provider(provider)?;`, add:
```rust
    if is_openai_compatible_name(provider) {
        let resolver = rupu_auth::resolver::KeychainResolver::new();
        resolver
            .forget_named(provider, rupu_providers::AuthMode::ApiKey)
            .await?;
        println!("rupu: forgot credential(s) for {provider}");
        return Ok(());
    }
    let pid = parse_provider(provider)?;
```

- [ ] **Step 4: Surface openai-compatible providers in `status`**

In `status`, after the built-in `rows` loop, append rows for declared openai-compatible providers:
```rust
    if let Ok(global) = crate::paths::global_dir() {
        let global_cfg = global.join("config.toml");
        if let Ok(cfg) = rupu_config::layer_files(Some(&global_cfg), None) {
            for (name, p) in &cfg.providers {
                if p.kind.as_deref() != Some("openai-compatible") {
                    continue;
                }
                let present = resolver
                    .peek_named(name, rupu_providers::AuthMode::ApiKey)
                    .await
                    || std::env::var(format!("RUPU_{}_API_KEY", name.to_ascii_uppercase()))
                        .map(|v| !v.is_empty())
                        .unwrap_or(false);
                rows.push(AuthStatusRow {
                    provider: name.clone(),
                    api_key: present,
                    sso: String::new(),
                });
            }
        }
    }
```

- [ ] **Step 5: Build + smoke**

Run: `cargo build -p rupu-cli && cargo run -p rupu-cli -- auth status`
Expected: builds; `auth status` runs and lists built-ins (plus any configured openai-compatible providers).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p rupu-cli
git add crates/rupu-cli/src/cmd/auth.rs
git commit -m "feat(cli): rupu auth login/logout/status support openai-compatible providers"
```

---

### Task 9: Docs, sample config, and live smoke

**Files:**
- Create: `crates/rupu-providers/tests/openai_compatible_live.rs`
- Modify: `README.md` (or `docs/` providers section — match where other providers are documented)
- Modify: `.rupu/config.toml` sample / docs (add a commented `[providers.oracle]` example)

- [ ] **Step 1: Add an ignored live smoke test**

Create `crates/rupu-providers/tests/openai_compatible_live.rs`:
```rust
//! Live smoke for an OpenAI-compatible endpoint. Ignored by default; run with:
//!   RUPU_LIVE_OAI_BASE_URL=http://host:8080 \
//!   RUPU_LIVE_OAI_KEY=sk-... \
//!   RUPU_LIVE_OAI_MODEL=/raid/models/zai-org/GLM-5.2-FP8 \
//!   cargo test -p rupu-providers --test openai_compatible_live -- --ignored --nocapture

use rupu_providers::provider::LlmProvider;
use rupu_providers::types::{LlmRequest, Message};

#[tokio::test]
#[ignore]
async fn live_non_streaming_completion() {
    let base = std::env::var("RUPU_LIVE_OAI_BASE_URL").expect("RUPU_LIVE_OAI_BASE_URL");
    let key = std::env::var("RUPU_LIVE_OAI_KEY").expect("RUPU_LIVE_OAI_KEY");
    let model = std::env::var("RUPU_LIVE_OAI_MODEL").expect("RUPU_LIVE_OAI_MODEL");

    let mut client =
        rupu_providers::OpenAiCompatibleClient::new(&base, &key, &model, vec![], false);
    let req = LlmRequest {
        model: model.clone(),
        messages: vec![Message::user("What is 17 * 23? Reply with just the number.")],
        max_tokens: 64,
        ..Default::default()
    };
    let resp = client.send(&req).await.expect("send");
    let text = resp.text().unwrap_or("");
    println!("model={} reply={text}", resp.model);
    assert!(text.contains("391"), "expected 391 in reply, got: {text}");
}
```

- [ ] **Step 2: Run the ignored test is skipped by default**

Run: `cargo test -p rupu-providers --test openai_compatible_live`
Expected: `0 passed; 0 failed; 1 ignored` (no network used).

- [ ] **Step 3: Document the provider + sample config**

Add a docs subsection "OpenAI-compatible providers (Oracle GenAI, vLLM, …)" where providers are documented, including:

```toml
# ~/.rupu/config.toml
default_provider = "oracle"

[providers.oracle]
kind = "openai-compatible"
base_url = "http://192.29.35.246:8080"
default_model = "/raid/models/zai-org/GLM-5.2-FP8"
stream = true   # set false if the server has no SSE endpoint

  [[providers.oracle.models]]
  id = "/raid/models/zai-org/GLM-5.2-FP8"
  context_window = 131072
  max_output = 8192
```

Document the key flow:
```text
# Store the Bearer key (auth.json, mode 0600):
rupu auth login --provider oracle --mode api-key   # prompts/reads stdin
# …or via env for CI:
export RUPU_ORACLE_API_KEY=sk-...
# Then run an agent whose frontmatter has `provider: oracle`:
rupu run --agent my-agent
```

Add a note: workflow steps and subagents will gain openai-compatible support in Plan 2; for now they use built-in providers.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-providers/tests/openai_compatible_live.rs README.md
git commit -m "docs(providers): document openai-compatible config + add live smoke test"
```

---

### Task 10: Workspace verification

- [ ] **Step 1: Full build + clippy + test**

Run:
```bash
cargo build --workspace
cargo clippy --workspace --all-targets
cargo test -p rupu-providers -p rupu-config -p rupu-auth -p rupu-runtime
```
Expected: build clean; clippy clean for touched crates (note the known toolchain-mismatch memo — `rupu-cli` may show pre-existing drift unrelated to this work; compare against `main`); the four library crates' tests pass.

- [ ] **Step 2: Confirm the `rupu run` + `rupu auth` paths build into the binary**

Run: `cargo build -p rupu-cli`
Expected: clean.

- [ ] **Step 3: Final commit (if any fmt/clippy fixups)**

```bash
git add -A
git commit -m "chore: openai-compatible providers Plan 1 — workspace verification fixups"
```

---

## Self-Review

**Spec coverage:**
- Goal "configurable base URL + Bearer + tools" → Task 3 (`OpenAiCompatibleClient`), Task 1 (`openai_wire` tool-calling). ✓
- "Declare instances purely in config" → Task 4 (`kind`/`stream` + `models`), Task 6 (`openai_compatible_params`). ✓
- "Secret in auth.json + env fallback" → Task 5 (resolver named path), Task 8 (`auth login`). ✓
- Spec §1 client details (base_url normalization, verbatim model, stream default true, list_models) → Task 3. ✓
- Spec §2 config schema → Task 4. ✓
- Spec §3 string-keyed credential path + env fallback → Task 5. ✓
- Spec §4 factory branch + name-legitimacy check → Tasks 6 (branch) + 7 (run.rs name validation). ✓
- Spec §4 auth CLI → Task 8. ✓
- Spec §5 models/pricing defaults → Task 6 (`DEFAULT_OAI_*`), pricing falls back to 0 (no code needed). ✓
- Spec testing section → Tasks 3/4/5/6 unit tests + Task 9 live smoke. ✓
- Spec error-handling (missing key, kind-without-base_url, non-2xx, SSE remedy) → Task 5 error msg, Task 4 validate, Task 3 `ProviderError::Api`, Task 9 docs note. ✓
- Deliberate deviations from the spec, called out in Global Constraints: (a) `api_key_env` config override deferred — Plan 1 uses the `RUPU_<NAME>_API_KEY` convention to keep the resolver config-agnostic; (b) workflow/subagent/generate factory paths deferred to Plan 2 (no loaded `Config`).

**Placeholder scan:** No "TBD"/"handle errors"/"similar to" — each code step carries full code. The only "confirm X exists" notes (tempfile dev-dep, `KeychainKey`/`AuthCredentials` imports) are explicit verification steps with the fix inline.

**Type consistency:** `OpenAiCompatibleModel { id, context_window, max_output }` consistent across Tasks 3/6. `openai_compatible_params`/`is_builtin_provider`/`OpenAiCompatibleParams` names consistent Tasks 6/7/8. `read_account(account_base, legacy_base, mode)` / `get_named` / `store_named` / `forget_named` / `peek_named` consistent in Task 5 and consumed in Task 8. Factory knobs field `openai_compatible: Option<OpenAiCompatibleParams>` set in Tasks 6/7 and `None` in the three session.rs sites (Task 6 Step 5).
