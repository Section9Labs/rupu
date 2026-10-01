use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use serde::Deserialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, info, warn};

use crate::auth::credential_store::resolve_provider_auth;
use crate::auth::{is_token_expired, AuthCredentials, AuthFile, AuthMethod};
use crate::error::ProviderError;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::*;

/// JSON Schema keywords that Anthropic's structured-outputs subset rejects
/// on numeric (`integer`/`number`) nodes. Present on such a node, the API
/// 400s (e.g. "output_config.format.schema: For 'integer' type, property
/// 'minimum' is not supported"). These are the numeric range/step
/// constraints; extend the list only when a keyword is *confirmed*
/// unsupported, so we never silently weaken a schema Anthropic accepts.
const ANTHROPIC_UNSUPPORTED_NUMERIC_KEYWORDS: &[&str] = &[
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
];

/// Down-convert an agent's `outputSchema` to Anthropic's structured-outputs
/// subset in place, returning how many keywords were removed.
///
/// Walks the schema recursively and strips
/// [`ANTHROPIC_UNSUPPORTED_NUMERIC_KEYWORDS`] from any node typed
/// `integer`/`number`. The strip is gated on the node's own `type` so a
/// property *named* `minimum` (a key in a `properties` map) is preserved —
/// only the constraint *keyword* is removed. Non-numeric nodes are left
/// untouched, since that is the exact surface the API rejects.
fn sanitize_output_schema_for_anthropic(value: &mut serde_json::Value) -> usize {
    let mut removed = 0;
    match value {
        serde_json::Value::Object(map) => {
            if schema_node_is_numeric(map) {
                for kw in ANTHROPIC_UNSUPPORTED_NUMERIC_KEYWORDS {
                    if map.remove(*kw).is_some() {
                        removed += 1;
                    }
                }
            }
            for child in map.values_mut() {
                removed += sanitize_output_schema_for_anthropic(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items.iter_mut() {
                removed += sanitize_output_schema_for_anthropic(child);
            }
        }
        _ => {}
    }
    removed
}

/// Does this schema node declare a numeric `type` (`integer`/`number`),
/// including the `type: [..]` union form?
fn schema_node_is_numeric(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    match map.get("type") {
        Some(serde_json::Value::String(t)) => matches!(t.as_str(), "integer" | "number"),
        Some(serde_json::Value::Array(types)) => types.iter().any(|t| {
            t.as_str()
                .is_some_and(|s| matches!(s, "integer" | "number"))
        }),
        _ => false,
    }
}

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// Canonical provider tag stamped on Reasoning blocks and used as the echo gate.
pub(crate) const PROVIDER_TAG: &str = "anthropic";

// ── Tool-name sanitization (mirrors openai_codex.rs) ─────────────────────────
//
// Anthropic's `/v1/messages` endpoint validates each custom tool's `name`
// against `^[a-zA-Z0-9_-]{1,128}$` and rejects anything containing `.`.
// Our MCP tool catalog (and any user MCP tools) uses dot-separated names
// like `scm.repos.list` and `issues.create`. We escape `.` → `__dot__` at
// the wire boundary and reverse the substitution on the way back so the
// dispatcher sees the canonical name. The escape is round-trip safe: the
// 7-char marker is illegal as part of a real tool name (no double
// underscore + literal `dot` followed by double underscore would clash
// in practice).

const ANTHROPIC_TOOL_NAME_DOT_ESCAPE: &str = "__dot__";

fn sanitize_anthropic_tool_name(name: &str) -> String {
    name.replace('.', ANTHROPIC_TOOL_NAME_DOT_ESCAPE)
}

fn desanitize_anthropic_tool_name(name: &str) -> String {
    name.replace(ANTHROPIC_TOOL_NAME_DOT_ESCAPE, ".")
}

/// Walk a serialized `messages` JSON array and rewrite every
/// `tool_use` block's `name` field through `sanitize_anthropic_tool_name`.
/// Used by `build_request_body` to ensure echoed conversation history
/// also uses the wire-safe form.
fn sanitize_messages_tool_names(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(arr) = value.as_array_mut() {
        for msg in arr.iter_mut() {
            if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
                for block in content.iter_mut() {
                    let is_tool_use = block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "tool_use")
                        .unwrap_or(false);
                    if !is_tool_use {
                        continue;
                    }
                    let Some(name_str) = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    block["name"] =
                        serde_json::Value::String(sanitize_anthropic_tool_name(&name_str));
                }
            }
        }
    }
    value
}

/// Rewrite internal `Reasoning` blocks into Anthropic's own wire shape, and
/// drop blocks that must never reach the wire.
///
/// `build_request_body` serializes `request.messages` with generic serde, which
/// works only because our Text/ToolUse/ToolResult shapes coincide with
/// Anthropic's. `Reasoning` is internal-only, so it is restored from its `raw`
/// payload here — byte-exact, since the API rejects *modified* blocks (reading
/// `text` is fine; it just never goes on the wire).
///
/// Blocks produced by another provider are dropped: a foreign continuity token
/// (e.g. a Gemini `thoughtSignature`) is an alien wire format. Blocks from this
/// provider are echoed regardless of model and regardless of whether their text
/// is empty — thinking blocks are not origin-locked, they replay across models
/// fine, and *stripping* them is what triggers ordering/signature 400s. The gate
/// is the provider tag only; do not add a model check here.
///
/// `Unknown` blocks are dropped too: they serialize as `{"type":"Unknown"}`
/// (pinned by a test in `types.rs`), which no provider accepts.
fn restore_reasoning_blocks(messages: &mut serde_json::Value, self_tag: &str) {
    let Some(msgs) = messages.as_array_mut() else {
        return;
    };
    for msg in msgs.iter_mut() {
        let Some(blocks) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        let mut restored = Vec::with_capacity(blocks.len());
        for block in blocks.iter() {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("reasoning") => {
                    let same_provider =
                        block.get("provider").and_then(|v| v.as_str()) == Some(self_tag);
                    if same_provider {
                        // `raw` has no `skip_serializing_if`, so `block.get("raw")`
                        // is always `Some` — a plain `if let` here would be
                        // defensive in appearance only. Require `raw` to actually
                        // look like a content block (an object with a `type`)
                        // before echoing it: a `null`, scalar, or `{}` `raw` would
                        // otherwise be pushed onto the wire as a malformed content
                        // block and 400 the request.
                        let raw = block.get("raw");
                        let looks_like_block =
                            raw.is_some_and(|r| r.is_object() && r.get("type").is_some());
                        if looks_like_block {
                            restored.push(raw.expect("checked Some above").clone());
                        } else {
                            debug!(raw = ?raw, "dropping reasoning block with malformed raw payload");
                        }
                    }
                    // else: foreign provider — drop.
                }
                Some("Unknown") => {} // never goes on the wire
                _ => restored.push(block.clone()),
            }
        }
        *blocks = restored;
    }
}

/// Content-block types that may carry `cache_control`. An allowlist, not a
/// denylist: `thinking` / `redacted_thinking` must never be marked, and they
/// are the only types `restore_reasoning_blocks` ever echoes (Anthropic
/// `Reasoning` blocks are captured exclusively from those two wire types), so
/// a restored raw block can never become a marker target — and neither can a
/// block type nobody has vetted yet.
const CACHEABLE_BLOCK_TYPES: &[&str] = &["text", "image", "document", "tool_use", "tool_result"];

/// Whether a `text`-shaped string carries anything (not empty / whitespace).
fn has_text(s: Option<&str>) -> bool {
    s.is_some_and(|t| !t.trim().is_empty())
}

/// Whether `block` can carry `cache_control`: a cacheable type, and not an
/// empty (or whitespace-only) text block, which cannot carry a marker.
///
/// A `tool_result` with no real content — absent, an empty / whitespace
/// string, an empty array, or an array of only empty / whitespace text
/// blocks — is treated as uncacheable too. Every silent `bash` call yields
/// exactly that as the final message's only block, and whether the API
/// accepts `cache_control` there is unverified; the marker walks back instead
/// (see [`apply_cache_breakpoints`]).
fn is_cacheable_block(block: &serde_json::Value) -> bool {
    let Some(ty) = block.get("type").and_then(|t| t.as_str()) else {
        return false;
    };
    if !CACHEABLE_BLOCK_TYPES.contains(&ty) {
        return false;
    }
    match ty {
        "text" => has_text(block.get("text").and_then(|t| t.as_str())),
        "tool_result" => match block.get("content") {
            Some(serde_json::Value::String(s)) => has_text(Some(s)),
            // Any non-text inner block (image, document, …) is content; text
            // blocks count only when non-empty.
            Some(serde_json::Value::Array(inner)) => inner.iter().any(|b| {
                b.get("type").and_then(|t| t.as_str()) != Some("text")
                    || has_text(b.get("text").and_then(|t| t.as_str()))
            }),
            _ => false,
        },
        _ => true,
    }
}

/// How many messages BEFORE the final one the rolling breakpoint may walk
/// back to when the final message has no cacheable block. Bounded so the
/// marker never lands deep in history — beyond this, no message-level marker
/// is placed at all.
const MAX_BREAKPOINT_WALK_BACK: usize = 3;

/// The one breakpoint value ever emitted: the 5-minute ephemeral cache.
fn ephemeral_marker() -> serde_json::Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// Mark the last cacheable block of `msg`, converting non-empty string
/// content to a single text block first (only when that block is the one
/// being marked). Returns whether a marker was placed.
fn mark_last_cacheable_block(msg: &mut serde_json::Value) -> bool {
    if let Some(text) = msg
        .get("content")
        .and_then(|c| c.as_str())
        .map(str::to_string)
    {
        if text.trim().is_empty() {
            return false;
        }
        msg["content"] = serde_json::json!([{ "type": "text", "text": text }]);
    }
    match msg
        .get_mut("content")
        .and_then(|c| c.as_array_mut())
        .and_then(|blocks| blocks.iter_mut().rev().find(|b| is_cacheable_block(b)))
    {
        Some(block) => {
            block["cache_control"] = ephemeral_marker();
            true
        }
        None => false,
    }
}

/// Explicit prompt-cache breakpoints (spec 2026-09-29 §9.3). Two markers:
/// (a) the last `system` block — caches tools + system (tools render first);
///     with no (non-empty) system block, the last tool definition instead;
/// (b) the last cacheable block of the final message — the rolling
///     conversation breakpoint, so each turn re-reads all prior history.
///     When the final message has no cacheable block (e.g. a silent `bash`
///     call's empty `tool_result`), walk back — at most
///     [`MAX_BREAKPOINT_WALK_BACK`] messages — and mark the last cacheable
///     block of the nearest message that has one (typically the preceding
///     assistant's `tool_use`). Cache reads are anchored at explicit
///     breakpoints, so a request with no message-level marker reads no
///     history from the cache at all; moving a marker onto a history block is
///     not a history edit. Mirrors the API's own automatic caching, which
///     walks backward to the nearest eligible block.
///
/// Explicit `{"type": "ephemeral"}` markers only (5-minute TTL — no `ttl`, no
/// top-level automatic caching), so at most 2 of the API's 4 breakpoints.
/// Never marks thinking / redacted_thinking / empty-text / empty-tool_result
/// blocks. Runs LAST in `build_request_body`, after tool-name sanitizing and
/// reasoning restoration, and only touches the marked blocks' `cache_control`
/// key — except that a message with string content is converted to a single
/// text block, and only when that block is the one being marked.
///
/// A prefix shorter than the model's minimum cacheable length (512–4096
/// tokens) is silently not cached by the API — no error, just no cache usage.
fn apply_cache_breakpoints(body: &mut serde_json::Value) {
    // (a) The prefix breakpoint: system, else tools.
    let mut marked_prefix = false;
    if let Some(block) = body
        .get_mut("system")
        .and_then(|s| s.as_array_mut())
        .and_then(|sys| sys.iter_mut().rev().find(|b| is_cacheable_block(b)))
    {
        block["cache_control"] = ephemeral_marker();
        marked_prefix = true;
    }
    if !marked_prefix {
        if let Some(last_tool) = body
            .get_mut("tools")
            .and_then(|t| t.as_array_mut())
            .and_then(|t| t.last_mut())
        {
            last_tool["cache_control"] = ephemeral_marker();
        }
    }

    // (b) The rolling conversation breakpoint: the final message, else the
    // nearest of the preceding MAX_BREAKPOINT_WALK_BACK messages.
    let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    for msg in messages.iter_mut().rev().take(1 + MAX_BREAKPOINT_WALK_BACK) {
        if mark_last_cacheable_block(msg) {
            return;
        }
    }
}

/// Anthropic API version. Update when new SSE event types or features are needed.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Page size for the `GET /v1/models` catalog listing (spec 2026-09-30 §3).
/// The API's maximum, so the whole catalog is normally one request.
const MODELS_PAGE_LIMIT: u32 = 1000;
// ─────────────────────────────────────────────────────────────────────
// Claude Code OAuth wire-shape pins
//
// The constants below were captured byte-for-byte from a MITM session
// of `claude --print "say hi"` on 2026-05-04. Anthropic's WAF / billing
// layer fingerprints OAuth `/v1/messages` traffic against the recognized
// claude-code request shape; mismatching values silently route the
// request into a stricter pool that returns 429 with an empty body and
// no `anthropic-ratelimit-*` headers (the exact symptom we hit before
// these were pinned).
//
// TODO(durable-fix): ALL of these are impersonation pins of upstream
// Claude Code values. They will go stale as upstream drifts. Watch for:
//   * UA version (`2.1.126`) — `claude --version`
//   * Stainless package + runtime versions — bundled in claude-cli
//   * `anthropic-beta` CSV — Anthropic adds/removes betas
//   * `cc_version` / `cch` in the billing-header system block
// If we start 429ing on a previously-working build, the first thing to
// re-MITM is `claude --print "say hi"` and diff against the values here.
// Long-term cure is the rupu-registered first-party OAuth client_id
// (TODO.md → "Register rupu-specific OAuth clients with each vendor").
// ─────────────────────────────────────────────────────────────────────

/// `User-Agent` sent on `/v1/messages`. Note `sdk-cli` (not `cli`) — the
/// `@anthropic-ai/sdk` sets `CLAUDE_CODE_ENTRYPOINT=sdk-cli` for the
/// messages-create call path (different from the bootstrap path's
/// `claude-code/<ver>` UA). Pin to the upstream release.
const RUPU_USER_AGENT: &str = "claude-cli/2.1.126 (external, sdk-cli)";

// `anthropic-beta` CSV is built per-model — see [`build_oauth_beta_csv`]
// below. Static pinning was rejected with a 429
// `"Extra usage is required for long context requests"` on accounts that
// hadn't enabled extra-usage billing, because the static CSV included
// `context-1m-2025-08-07` for *every* request (even ones using a 200K
// model like `claude-sonnet-4-6`). Reference: claude-cli's
// `getAllModelBetas` (utils/betas.ts) only emits `context-1m-2025-08-07`
// when `has1mContext(model)` returns true — i.e. when the model string
// carries the `[1m]` suffix.

/// `@anthropic-ai/sdk` (Stainless-generated) telemetry headers. Values
/// match SDK v0.81.0 on Node v24.3.0 / macOS arm64 — what claude-cli
/// 2.1.126 ships with. Update when upstream rev drifts.
const STAINLESS_HEADERS: &[(&str, &str)] = &[
    ("X-Stainless-Arch", "arm64"),
    ("X-Stainless-Lang", "js"),
    ("X-Stainless-OS", "MacOS"),
    ("X-Stainless-Package-Version", "0.81.0"),
    ("X-Stainless-Retry-Count", "0"),
    ("X-Stainless-Runtime", "node"),
    ("X-Stainless-Runtime-Version", "v24.3.0"),
    ("X-Stainless-Timeout", "600"),
];

/// First text block of `system[]` on every OAuth `/v1/messages` call.
/// NOT an HTTP header — Anthropic parses this string out of the system
/// content and uses it for billing attribution. Without this exact
/// shape the request is not tagged as Claude-Code-billable and lands
/// in the empty-body 429 pool. Captured from MITM 2026-05-04.
///
/// TODO(reverse-engineer): the `cch=0ab17` token may be a checksum of
/// some other request fields. Right now we send the captured value
/// statically; if Anthropic begins strict validation (a sudden return
/// to 429 with this PR shipped is the signal), we'll need to figure
/// out what input bytes produce that hash and compute it dynamically
/// per request. The full upstream string lives in
/// `services/api/claude.ts` of the claude-cli source under the
/// `x-anthropic-billing-header` token name.
const ANTHROPIC_BILLING_HEADER_BLOCK: &str =
    "x-anthropic-billing-header: cc_version=2.1.126.125; cc_entrypoint=sdk-cli; cch=0ab17;";

/// Second `system[]` block — claude-cli always emits this Claude Agent
/// SDK self-description right after the billing block.
const ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

/// Maximum retries for 429 rate-limit responses.
/// Per-request 429 retries. Set to 1 (one retry) so the ProviderRouter can
/// handle cross-provider fallback quickly. When used without a router, the
/// single retry handles brief transient rate limits.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;
/// Initial backoff for 429 retries (doubles each attempt).
const INITIAL_BACKOFF_MS: u64 = 2000;
/// Idle timeout for a streaming response: if no bytes arrive for this long the
/// model is considered stalled (a connection is open but the server stopped
/// emitting — observed with some preview models during long prefill). The
/// request is aborted and re-sent. This is an *idle* (per-chunk) timeout, not a
/// total one, so legitimately-long generations are never cut off mid-stream.
const STREAM_IDLE_TIMEOUT_SECS: u64 = 120;
/// How many times to re-send a stalled stream before giving up. Retries only
/// happen on a *prefill* stall (before any output) to avoid duplicating a
/// partially-streamed response.
const MAX_STREAM_IDLE_RETRIES: u32 = 10;
/// Total timeout for a one-shot (non-streaming) `send` request. Generous —
/// covers prefill + full generation — but bounded so a hung connection can't
/// block forever.
const SEND_TOTAL_TIMEOUT_SECS: u64 = 600;

/// I-84: whether `stream()`'s outer idle-restart loop should draw down the
/// shared retry budget and re-send, or give up. `total_retry_budget` is the
/// SUM of the idle-restart and 429-retry ceilings
/// (`MAX_STREAM_IDLE_RETRIES` plus `max_rate_limit_retries`) — before this
/// fix the two loops were bounded independently, so the worst case (a 429
/// on every idle-restart pass, each eventually succeeding then stalling
/// again) multiplied attempts instead of adding them. Extracted as a pure
/// function so that "sum, not product" is directly unit-testable without
/// simulating a real 120s idle stall over the network. A mid-response
/// stall (`emitted_content`) is never retried, budget or not — re-sending
/// would duplicate already-streamed output.
fn should_retry_idle_stall(
    emitted_content: bool,
    retries_used: u32,
    total_retry_budget: u32,
) -> bool {
    !emitted_content && retries_used < total_retry_budget
}

const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Build the netflow-instrumented client used for Anthropic requests,
/// bound to `sink`.
///
/// Forces HTTP/1.1: MITM capture of the first-party Claude Code binary
/// shows it negotiates HTTP/1.1 for `/v1/messages`. Matching that on
/// the wire keeps rupu's requests in the same Cloudflare/WAF
/// classification bucket as claude-code traffic.
fn build_http_client(
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> (ClientWithMiddleware, Arc<dyn rupu_netflow::FlowSink>) {
    build_http_client_with_timeout(None, sink)
}

/// `build_http_client` with an optional inactivity deadline from
/// `[providers.anthropic].timeout_ms` (ISSUES.md I-9).
///
/// Applied as connect + read timeouts, never as reqwest's total `timeout`:
/// a total deadline would abort a long generation mid-stream. `None` keeps
/// the historical no-deadline client (used by every non-factory constructor).
///
/// No run context is available at this layer — every client built here is
/// stamped `FlowCtx::system(Origin::Provider("anthropic"))`. Plan 2 threads
/// the real run id through once the provider factory is touched.
///
/// The builder below is never `.build()`'d directly — it flows straight
/// into `rupu_netflow::http::client_with`, the sanctioned pattern (see
/// Task 11's `clippy.toml`); only `ClientBuilder::build()` is
/// clippy-disallowed, and this function never calls it.
///
/// There is deliberately no process-global sink to fall back to: `sink`
/// is the only source of truth. Returns the same `Arc<dyn FlowSink>` back
/// alongside the client so callers can store both — `AnthropicClient`
/// does, and its two-phase streaming completion (`FlowCompletionGuard`)
/// calls `complete` on this stored sink directly, guaranteeing the `Flow`
/// and its `Complete` always travel together instead of either one
/// silently re-targeting some other sink at completion time. See the
/// 2026-08-03 whole-branch review, Fix 1.
///
/// A `client_with` failure here means the process's TLS backend / DNS
/// resolver setup failed — a process-wide condition, not something this
/// function's own (infallible) config assignments can trigger. There is
/// no infallible `reqwest::Client` constructor to fall back to, so this
/// panics rather than silently returning an uninstrumented client.
fn build_http_client_with_timeout(
    timeout: Option<std::time::Duration>,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> (ClientWithMiddleware, Arc<dyn rupu_netflow::FlowSink>) {
    // Shared pool (see `rupu_netflow::http::shared_client`): one connection
    // pool per transport shape process-wide, so concurrent agent runs don't
    // each hold their own idle sockets.
    let transport = rupu_netflow::http::Transport {
        http1_only: true,
        timeout,
    };
    let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider(PROVIDER_TAG.into()));
    let client = rupu_netflow::http::shared_client(ctx, transport, sink.clone())
        .expect("reqwest TLS backend failed to initialise; no HTTP client can be built");
    (client, sink)
}

/// Whether the model string carries the explicit 1M-context opt-in
/// suffix (`[1m]`, case-insensitive). Mirrors claude-cli's
/// `has1mContext` (`utils/context.ts:35`). The server gates the
/// `context-1m-2025-08-07` beta on extra-usage billing; sending it on a
/// stock 200K model produces a 429 `"Extra usage is required for long
/// context requests"`.
fn model_has_1m_suffix(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("[1m]")
}

/// Whether the model supports interleaved thinking. Mirrors claude-cli's
/// `modelSupportsISP` (`utils/betas.ts:92`) for the firstParty path:
/// any 4-tier (or newer) Claude model.
fn model_supports_interleaved_thinking(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower.contains("claude-opus-4")
        || lower.contains("claude-sonnet-4")
        || lower.contains("claude-haiku-4")
}

/// Whether the model supports server-side context management. Mirrors
/// claude-cli's `modelSupportsContextManagement` (`utils/betas.ts:125`):
/// 4-tier and newer.
fn model_supports_context_management(model: &str) -> bool {
    model_supports_interleaved_thinking(model)
}

/// Build the `anthropic-beta` CSV value for an OAuth `/v1/messages`
/// request. Order matches what claude-cli's `getAllModelBetas`
/// (`utils/betas.ts:234`) produces; gating logic mirrors the same
/// function so we don't ship betas the model can't honor.
///
/// Always-on:
///   * `oauth-2025-04-20` — OAuth-attribution beta (Claude.ai
///     subscriber path).
///   * `prompt-caching-scope-2026-01-05` — first-party only; no-op
///     without an explicit scope field but always sent.
///   * `advisor-tool-2026-03-01`, `advanced-tool-use-2025-11-20`,
///     `effort-2025-11-24`, `cache-diagnosis-2026-04-07` — verbatim
///     from MITM capture; not visibly model-gated upstream.
///
/// Gated:
///   * `claude-code-20250219` — non-Haiku models only.
///   * `context-1m-2025-08-07` — only when the model string carries the
///     `[1m]` suffix.
///   * `interleaved-thinking-2025-05-14` — 4-tier+ models.
///   * `context-management-2025-06-27` — 4-tier+ models OR when the
///     caller explicitly opts in via `wants_context_management`.
fn build_oauth_beta_csv(model: &str, wants_context_management: bool) -> String {
    let is_haiku = model.to_ascii_lowercase().contains("haiku");
    let mut betas: Vec<&str> = Vec::with_capacity(10);
    if !is_haiku {
        betas.push("claude-code-20250219");
    }
    betas.push("oauth-2025-04-20");
    if model_has_1m_suffix(model) {
        betas.push("context-1m-2025-08-07");
    }
    if model_supports_interleaved_thinking(model) {
        betas.push("interleaved-thinking-2025-05-14");
    }
    if model_supports_context_management(model) || wants_context_management {
        betas.push("context-management-2025-06-27");
    }
    betas.push("prompt-caching-scope-2026-01-05");
    betas.push("advisor-tool-2026-03-01");
    betas.push("advanced-tool-use-2025-11-20");
    betas.push("effort-2025-11-24");
    betas.push("cache-diagnosis-2026-04-07");
    betas.join(",")
}

/// Resolve authentication for Anthropic.
/// Delegates to resolve_provider_auth, with legacy Pi format fallback.
/// Search order: auth_json_path -> cortex/auth.json -> ~/.pi/agent/auth.json -> ANTHROPIC_API_KEY
pub fn resolve_anthropic_auth(
    auth_json_path: Option<&Path>,
    cortex_dir: Option<&Path>,
) -> Result<AuthMethod, ProviderError> {
    // Primary path: use the generalized provider auth resolver
    if let Ok(creds) = resolve_provider_auth(ProviderId::Anthropic, auth_json_path, cortex_dir) {
        return Ok(creds.into_anthropic_auth_method());
    }

    // Fallback: legacy Pi format support (load_auth_json handles non-tagged JSON)
    let mut paths_to_try: Vec<PathBuf> = Vec::new();
    if let Some(p) = auth_json_path {
        paths_to_try.push(p.to_path_buf());
    } else {
        if let Some(cortex) = cortex_dir {
            paths_to_try.push(cortex.join("auth.json"));
        }
        if let Ok(home) = std::env::var("HOME") {
            paths_to_try.push(PathBuf::from(home).join(".pi/agent/auth.json"));
        }
    }

    for path in &paths_to_try {
        if path.exists() {
            if let Ok(Some(method)) = load_auth_json(path) {
                info!(path = %path.display(), "loaded auth from auth.json (legacy format)");
                return Ok(method);
            }
        }
    }

    // Fallback: read from Claude Code's macOS Keychain entry
    #[cfg(target_os = "macos")]
    if let Some(method) = load_claude_code_keychain() {
        info!("loaded auth from Claude Code keychain");
        return Ok(method);
    }

    // Final fallback: env var with AuthMethod::detect (handles OAuth prefix)
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => {
            info!("using ANTHROPIC_API_KEY from environment");
            Ok(AuthMethod::detect(&key))
        }
        _ => Err(ProviderError::MissingAuth {
            provider: "anthropic".into(),
            env_hint: "ANTHROPIC_API_KEY".into(),
        }),
    }
}

/// Load Anthropic credentials from an auth.json file.
/// Supports both tagged enum format ({"type":"oauth",...}) and legacy Pi format.
pub(crate) fn load_auth_json(path: &Path) -> Result<Option<AuthMethod>, ProviderError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot read {}: {e}", path.display())))?;

    // Try new tagged enum format first
    if let Ok(auth) = serde_json::from_str::<AuthFile>(&content) {
        return match auth.get("anthropic") {
            Some(AuthCredentials::OAuth {
                access,
                refresh,
                expires,
                ..
            }) => Ok(Some(AuthMethod::OAuth {
                access_token: access.clone(),
                refresh_token: refresh.clone(),
                expires_ms: *expires,
            })),
            Some(AuthCredentials::ApiKey { key }) => Ok(Some(AuthMethod::ApiKey(key.clone()))),
            None => Ok(None),
        };
    }

    // Fallback: try legacy Pi format (has "type" field as plain string, not serde tag)
    #[derive(Deserialize)]
    struct LegacyCredentials {
        #[serde(rename = "type", default)]
        auth_type: String,
        #[serde(default)]
        access: String,
        #[serde(default)]
        refresh: String,
        #[serde(default)]
        expires: u64,
    }
    type LegacyAuthFile = HashMap<String, LegacyCredentials>;

    let legacy: LegacyAuthFile = serde_json::from_str(&content)
        .map_err(|e| ProviderError::AuthConfig(format!("invalid auth.json: {e}")))?;

    if let Some(creds) = legacy.get("anthropic") {
        if creds.auth_type == "oauth" || !creds.access.is_empty() {
            return Ok(Some(AuthMethod::OAuth {
                access_token: creds.access.clone(),
                refresh_token: creds.refresh.clone(),
                expires_ms: creds.expires,
            }));
        }
    }

    Ok(None)
}

/// Load Anthropic OAuth tokens from Claude Code's macOS Keychain.
///
/// Claude Code stores credentials in the macOS Keychain under the service
/// name "Claude Code-credentials" as hex-encoded JSON. The JSON contains
/// a `claudeAiOauth` key with `accessToken`, `refreshToken`, and `expiresAt`.
#[cfg(target_os = "macos")]
pub(crate) fn load_claude_code_keychain() -> Option<AuthMethod> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if raw.is_empty() {
        return None;
    }

    // Claude Code stores as either raw JSON or hex-encoded JSON
    let parsed: serde_json::Value = if raw.starts_with('{') {
        serde_json::from_str(&raw).ok()?
    } else {
        let json_bytes = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(raw.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()?;
        serde_json::from_slice(&json_bytes).ok()?
    };
    let oauth = parsed.get("claudeAiOauth")?;

    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = oauth.get("expiresAt")?.as_u64()?;

    if access_token.is_empty() {
        return None;
    }

    Some(AuthMethod::OAuth {
        access_token,
        refresh_token,
        expires_ms: expires_at,
    })
}

/// Refresh an Anthropic OAuth token. Returns updated AuthMethod.
/// Uses application/x-www-form-urlencoded as required by the token endpoint.
pub async fn refresh_anthropic_token(
    client: &ClientWithMiddleware,
    refresh_token: &str,
) -> Result<AuthMethod, ProviderError> {
    info!("refreshing Anthropic OAuth token");

    let response = client
        .post(ANTHROPIC_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", ANTHROPIC_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::TokenRefreshFailed(format!(
            "HTTP {status}: {body}"
        )));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| ProviderError::TokenRefreshFailed("missing access_token".into()))?
        .to_string();

    let new_refresh = body["refresh_token"]
        .as_str()
        .unwrap_or(refresh_token)
        .to_string();

    let expires_in_secs = body["expires_in"].as_u64().unwrap_or(3600);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let expires_ms = now_ms + (expires_in_secs * 1000);

    info!("token refreshed, expires in {expires_in_secs}s");

    Ok(AuthMethod::OAuth {
        access_token,
        refresh_token: new_refresh,
        expires_ms,
    })
}

/// Write updated credentials back to auth.json (preserving other providers).
/// Uses atomic write (temp + rename) with 0o600 permissions set on temp BEFORE rename.
pub fn save_auth_json(path: &Path, auth_method: &AuthMethod) -> Result<(), ProviderError> {
    use fs2::FileExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ProviderError::AuthConfig(format!("cannot create dir: {e}")))?;
    }

    // File-locked read-modify-write to prevent race with CredentialStore
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::File::create(&lock_path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot create lock: {e}")))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| ProviderError::AuthConfig(format!("cannot acquire lock: {e}")))?;

    let content = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut auth: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    match auth_method {
        AuthMethod::OAuth {
            access_token,
            refresh_token,
            expires_ms,
        } => {
            auth["anthropic"] = serde_json::json!({
                "type": "oauth",
                "access": access_token,
                "refresh": refresh_token,
                "expires": expires_ms,
            });
        }
        AuthMethod::ApiKey(key) => {
            auth["anthropic"] = serde_json::json!({
                "type": "api_key",
                "key": key,
            });
        }
    }

    let updated = serde_json::to_string_pretty(&auth)
        .map_err(|e| ProviderError::AuthConfig(e.to_string()))?;

    let temp = path.with_extension(format!("tmp.{:?}", std::thread::current().id()));
    std::fs::write(&temp, updated.as_bytes())
        .map_err(|e| ProviderError::AuthConfig(format!("cannot write {}: {e}", temp.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ProviderError::AuthConfig(format!("cannot set permissions: {e}")))?;
    }

    std::fs::rename(&temp, path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot rename: {e}")))?;

    drop(lock_file);
    let _ = std::fs::remove_file(&lock_path);

    info!(path = %path.display(), "auth.json updated");
    Ok(())
}

/// Anthropic Messages API client with SSE streaming.
/// Supports both API key (`x-api-key`) and OAuth (`Authorization: Bearer`) auth.
pub struct AnthropicClient {
    client: ClientWithMiddleware,
    /// The exact sink `client` was bound to. Stored alongside the client
    /// (there is no process-global sink to re-resolve — see Task 4) so
    /// `FlowCompletionGuard`'s two-phase streaming completion always
    /// targets the same ledger its `Flow` line was written to. See
    /// `build_http_client_with_timeout`'s docstring.
    sink: Arc<dyn rupu_netflow::FlowSink>,
    auth: AuthMethod,
    api_url: String,
    /// Path to auth.json for writing refreshed tokens back (legacy).
    auth_json_path: Option<std::path::PathBuf>,
    /// Credential store for persisting refreshed tokens (preferred over auth_json_path).
    credential_store: Option<std::sync::Arc<dyn crate::credential_source::CredentialSource>>,
    /// Anthropic OAuth account UUID, captured from the token-exchange
    /// response and persisted in `AuthCredentials::OAuth.extra`. Sent as
    /// `metadata.user_id.account_uuid` so the request binds to the
    /// caller's Pro/Max quota; without it Anthropic falls back to a
    /// stricter rate-limit pool.
    oauth_account_uuid: Option<String>,
    /// Whether to prepend the canonical "You are Claude Code, …" system-
    /// prompt prefix on OAuth requests. Defaults to true; per-agent
    /// opt-out via `anthropicOauthPrefix: false` in agent frontmatter.
    /// Has no effect on api-key requests.
    oauth_system_prefix_enabled: bool,
    /// Whether `build_request_body` places the explicit prompt-cache
    /// breakpoints (see [`apply_cache_breakpoints`]). Defaults to true in
    /// every constructor; opt out via `[providers.<name>] prompt_cache =
    /// false` or agent frontmatter `anthropicPromptCache: false` (the agent
    /// wins), threaded through [`AnthropicClient::with_prompt_cache`].
    prompt_cache_enabled: bool,
    /// Per-request 429 retry budget. Defaults to [`MAX_RATE_LIMIT_RETRIES`];
    /// `[providers.anthropic].max_retries` overrides it via
    /// [`AnthropicClient::with_tuning`] (ISSUES.md I-10).
    max_rate_limit_retries: u32,
    /// Concurrency limiter (I-75). `None` for clients built directly by
    /// tests/callers that never call `with_tuning` — preserves their
    /// existing unthrottled behavior exactly. `with_tuning` (the path
    /// every factory-built client goes through) sets this to
    /// `Some(tuning.semaphore("anthropic"))`. Acquired fresh per HTTP
    /// attempt (see `acquire_permit`) rather than held for a whole
    /// `send`/`stream` call, so the permit is released across this
    /// client's own 429 backoff sleep instead of starving every other
    /// concurrent Anthropic call for the whole ladder.
    semaphore: Option<Arc<Semaphore>>,
}

impl AnthropicClient {
    /// Create a new client from a resolved AuthMethod.
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn from_auth(auth: AuthMethod, sink: Arc<dyn rupu_netflow::FlowSink>) -> Self {
        let (client, sink) = build_http_client(sink);
        Self {
            client,
            sink,
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: None,
            credential_store: None,
            oauth_account_uuid: None,
            oauth_system_prefix_enabled: true,
            prompt_cache_enabled: true,
            max_rate_limit_retries: MAX_RATE_LIMIT_RETRIES,
            semaphore: None,
        }
    }

    /// Apply `[providers.anthropic]` tuning: rebuild the HTTP client with the
    /// configured inactivity deadline (I-9), adopt the configured 429 retry
    /// budget (I-10), and adopt the configured concurrency limit (I-11/I-75).
    /// The factory calls this for every client it builds, which is why the
    /// Anthropic client is the one provider NOT wrapped in `RetryingProvider`
    /// (its own in-client retry loop makes stacking one multiply the budget)
    /// NOR `ThrottledProvider` (its own per-attempt semaphore acquisition
    /// below makes stacking one either double-count permits or deadlock
    /// outright at `max_concurrency == 1`).
    pub fn with_tuning(mut self, tuning: &crate::tuning::ProviderTuning) -> Self {
        let (client, sink) =
            build_http_client_with_timeout(Some(tuning.timeout), self.sink.clone());
        self.client = client;
        self.sink = sink;
        self.max_rate_limit_retries = tuning.max_retries;
        self.semaphore = Some(tuning.semaphore("anthropic"));
        self
    }

    /// The 429 retry budget this client will actually spend.
    pub fn max_rate_limit_retries(&self) -> u32 {
        self.max_rate_limit_retries
    }

    /// Acquire one permit from the configured concurrency semaphore, or
    /// `None` when no tuning was ever applied (unthrottled — the historical
    /// behavior for clients built directly rather than through the
    /// factory). Called fresh for every individual HTTP attempt so the
    /// permit can be dropped (by letting the returned guard go out of
    /// scope) before a 429 backoff sleep, instead of being held for an
    /// entire `send`/`stream` call the way `ThrottledProvider` holds it for
    /// every other provider (I-75).
    async fn acquire_permit(&self) -> Result<Option<OwnedSemaphorePermit>, ProviderError> {
        match &self.semaphore {
            Some(sem) => {
                let permit = sem.clone().acquire_owned().await.map_err(|e| {
                    ProviderError::Other(anyhow::anyhow!("provider semaphore closed: {e}"))
                })?;
                Ok(Some(permit))
            }
            None => Ok(None),
        }
    }

    /// Set the OAuth account UUID. Used by the factory after reading it
    /// from the resolved credential's `extra` map. Returns `self` for
    /// builder-style chaining.
    pub fn with_oauth_account_uuid(mut self, uuid: Option<String>) -> Self {
        self.oauth_account_uuid = uuid;
        self
    }

    /// Toggle the canonical "You are Claude Code, …" system-prompt
    /// prefix on OAuth requests. Default is enabled. Per-agent opt-out
    /// flows here from the agent file's `anthropicOauthPrefix: false`
    /// frontmatter — useful when the prefix corrupts agent persona.
    pub fn with_oauth_system_prefix(mut self, enabled: bool) -> Self {
        self.oauth_system_prefix_enabled = enabled;
        self
    }

    /// Explicit prompt-cache breakpoints (default ON). `false` from
    /// `[providers.<name>] prompt_cache = false` or agent frontmatter
    /// `anthropicPromptCache: false` — e.g. for an Anthropic-compatible
    /// gateway that rejects `cache_control`.
    pub fn with_prompt_cache(mut self, enabled: bool) -> Self {
        self.prompt_cache_enabled = enabled;
        self
    }

    /// Create a client with an auth.json path for persisting refreshed tokens.
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn from_auth_with_path(
        auth: AuthMethod,
        auth_json_path: std::path::PathBuf,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let (client, sink) = build_http_client(sink);
        Self {
            client,
            sink,
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: Some(auth_json_path),
            credential_store: None,
            oauth_account_uuid: None,
            oauth_system_prefix_enabled: true,
            prompt_cache_enabled: true,
            max_rate_limit_retries: MAX_RATE_LIMIT_RETRIES,
            semaphore: None,
        }
    }

    /// Create a client backed by a CredentialStore for token persistence.
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn from_auth_with_store(
        auth: AuthMethod,
        store: std::sync::Arc<dyn crate::credential_source::CredentialSource>,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let (client, sink) = build_http_client(sink);
        Self {
            client,
            sink,
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: None,
            credential_store: Some(store),
            oauth_account_uuid: None,
            oauth_system_prefix_enabled: true,
            prompt_cache_enabled: true,
            max_rate_limit_retries: MAX_RATE_LIMIT_RETRIES,
            semaphore: None,
        }
    }

    /// Create a new client. Reads `ANTHROPIC_API_KEY` from environment.
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn from_env(sink: Arc<dyn rupu_netflow::FlowSink>) -> Result<Self, ProviderError> {
        let auth = resolve_anthropic_auth(None, None)?;
        Ok(Self::from_auth(auth, sink))
    }

    /// Create a client with an explicit API key (for testing).
    pub fn new(api_key: String, sink: Arc<dyn rupu_netflow::FlowSink>) -> Self {
        Self::from_auth(AuthMethod::ApiKey(api_key), sink)
    }

    /// Create a client pointing at a custom URL (for testing with mock servers).
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn with_url(
        api_key: String,
        api_url: String,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let (client, sink) = build_http_client(sink);
        Self {
            client,
            sink,
            auth: AuthMethod::ApiKey(api_key),
            api_url,
            auth_json_path: None,
            credential_store: None,
            oauth_account_uuid: None,
            oauth_system_prefix_enabled: true,
            prompt_cache_enabled: true,
            max_rate_limit_retries: MAX_RATE_LIMIT_RETRIES,
            semaphore: None,
        }
    }

    /// Create a client from an explicit `AuthMethod` pointing at a custom URL
    /// (for testing OAuth flows against mock servers — the api-key-only
    /// `with_url` cannot exercise the OAuth header path).
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn from_auth_with_url(
        auth: AuthMethod,
        api_url: String,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        let (client, sink) = build_http_client(sink);
        Self {
            client,
            sink,
            auth,
            api_url,
            auth_json_path: None,
            credential_store: None,
            oauth_account_uuid: None,
            oauth_system_prefix_enabled: true,
            prompt_cache_enabled: true,
            max_rate_limit_retries: MAX_RATE_LIMIT_RETRIES,
            semaphore: None,
        }
    }

    /// Ensure the OAuth token is still valid, refreshing if expired.
    /// Persists refreshed tokens via CredentialStore (preferred) or save_auth_json (legacy).
    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        if let AuthMethod::OAuth {
            refresh_token,
            expires_ms,
            ..
        } = &self.auth
        {
            if !refresh_token.is_empty() && is_token_expired(*expires_ms) {
                info!("OAuth token expired, refreshing");
                let new_auth = refresh_anthropic_token(&self.client, refresh_token).await?;

                // Persist via CredentialStore (file-locked, preserves other providers)
                if let Some(store) = &self.credential_store {
                    let creds = match &new_auth {
                        AuthMethod::OAuth {
                            access_token,
                            refresh_token,
                            expires_ms,
                        } => AuthCredentials::OAuth {
                            access: access_token.clone(),
                            refresh: refresh_token.clone(),
                            expires: *expires_ms,
                            extra: std::collections::HashMap::new(),
                        },
                        AuthMethod::ApiKey(key) => AuthCredentials::ApiKey { key: key.clone() },
                    };
                    if let Err(e) = store.update(crate::provider_id::ProviderId::Anthropic, creds) {
                        warn!(error = %e, "failed to persist refreshed token via credential store");
                    }
                } else if let Some(path) = &self.auth_json_path {
                    // Legacy fallback
                    if let Err(e) = save_auth_json(path, &new_auth) {
                        warn!(error = %e, "failed to save refreshed token to auth.json");
                    }
                }

                self.auth = new_auth;
            }
        }
        Ok(())
    }

    /// Apply auth headers to a request builder based on auth method.
    ///
    /// Both api-key and OAuth paths get a fresh `x-anthropic-api-request-id`
    /// (UUID v4) on every call — claude-cli sets this whenever the request
    /// targets `api.anthropic.com` and we mirror that behavior; the value
    /// is also useful for support tickets when correlated with the
    /// server-returned `request-id` header.
    ///
    /// The OAuth path's `anthropic-beta` is a CSV of feature flags, not a
    /// single value. Reference clients always include `claude-code-20250219`
    /// alongside `oauth-2025-04-20` on Sonnet/Opus models — sending only
    /// `oauth-2025-04-20` (the previous behavior) is what an unaffiliated
    /// OAuth integration would do, which is the opposite of the signal we
    /// want for first-party-quota attribution. Haiku models drop the
    /// `claude-code-` flag because the reference client gates that beta to
    /// non-Haiku tiers.
    fn apply_auth_headers(
        &self,
        builder: RequestBuilder,
        model: &str,
        context_window: Option<crate::model_tier::ContextWindow>,
        wants_context_management: bool,
    ) -> RequestBuilder {
        // Headers verbatim from MITM capture of real `claude --print`
        // against api.anthropic.com. Order is preserved to match the
        // fingerprint. Anthropic's WAF / OAuth-quota router checks for
        // the X-Stainless-* + X-Claude-Code-Session-Id + the full
        // `anthropic-beta` CSV; missing any of these silently routes
        // the request into a stricter pool that returns the empty-body
        // 429 we kept seeing.
        let mut b = builder
            .header("Accept", "application/json")
            .header("User-Agent", RUPU_USER_AGENT)
            .header("X-Claude-Code-Session-Id", uuid::Uuid::new_v4().to_string())
            .header("x-client-request-id", uuid::Uuid::new_v4().to_string())
            .header("anthropic-dangerous-direct-browser-access", "true")
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Connection", "keep-alive");
        // Note: Accept-Encoding intentionally omitted — reqwest's `gzip`
        // feature isn't enabled on this build, so a compressed response
        // would fail SSE parsing. The server falls back to identity
        // encoding when this header is absent.
        for (name, value) in STAINLESS_HEADERS {
            b = b.header(*name, *value);
        }
        match &self.auth {
            AuthMethod::ApiKey(key) => {
                let mut b = b.header("x-api-key", key);
                // 1M context on the api-key path is gated on the
                // `context-1m-2025-08-07` beta. OAuth path always sends
                // it via the static beta CSV; the api-key path opts in
                // per-request based on `LlmRequest.context_window`.
                if matches!(
                    context_window,
                    Some(crate::model_tier::ContextWindow::OneMillion)
                ) {
                    b = b.header("anthropic-beta", "context-1m-2025-08-07");
                }
                b
            }
            AuthMethod::OAuth { access_token, .. } => {
                // Per-model `anthropic-beta` CSV — see `build_oauth_beta_csv`.
                // The `[1m]` suffix on the model string toggles
                // `context-1m-2025-08-07`; `LlmRequest.context_window` is
                // an api-key-path-only opt-in (the OAuth path uses the
                // suffix because that's what claude-cli does and what the
                // server gates on).
                let _ = context_window;
                let beta_csv = build_oauth_beta_csv(model, wants_context_management);
                b.header("Authorization", format!("Bearer {access_token}"))
                    .header("anthropic-beta", beta_csv)
                    .header("x-app", "cli")
            }
        }
    }
}

/// Build the CSV value for the `anthropic-beta` header on OAuth requests.
///
/// Always includes `oauth-2025-04-20`. For non-Haiku models also includes
/// `claude-code-20250219` — the reference client gates this beta to the
/// Sonnet/Opus tiers; sending it on Haiku produces a 400.
/// Whether the model supports `thinking: {"type":"adaptive"}`. Mirrors
/// claude-cli's `modelSupportsAdaptiveThinking` heuristic: Opus / Sonnet
/// 4-tier and newer support adaptive; Haiku and pre-4 models do not.
fn model_supports_adaptive_thinking(model: &str) -> bool {
    if model.contains("haiku") {
        return false;
    }
    // claude-{opus,sonnet}-4-…  (4-tier and newer). Pre-4 models have
    // numerical major in {3, 3-5}; we only enable adaptive on 4+.
    model.contains("-4-") || model.contains("-4.")
}

/// Whether a stream event counts as "content emitted" for `stream()`'s
/// idle-stall guard. Once any of these fire, a stall can no longer be
/// silently retried — retrying would duplicate already-streamed output.
/// `ReasoningDelta` counts: thinking tokens are real generated output,
/// just like text or tool-input deltas.
fn stream_event_counts_as_emitted_content(ev: &StreamEvent) -> bool {
    matches!(
        ev,
        StreamEvent::TextDelta(_)
            | StreamEvent::ToolUseStart { .. }
            | StreamEvent::InputJsonDelta(_)
            | StreamEvent::ReasoningDelta(_)
    )
}

/// Guards one flow attempt's completion against `stream()` being cancelled
/// mid-flight.
///
/// `rupu-agent`'s runner races `stream()` against a pause signal in a
/// `tokio::select!` and drops the losing branch outright — plain sequential
/// code placed after an `.await` (like an unconditional `complete()` call at
/// the end of the chunk loop) simply never runs in that case, silently
/// discarding every byte already accounted for. This guard closes that gap:
/// the normal path calls [`Self::complete`] explicitly, which disarms
/// `Drop` *before* its own `.await` so a cancellation during that very call
/// can never cause a second, conflicting completion. If the guard is
/// dropped while still armed — the enclosing future was cancelled — `Drop`
/// spawns a best-effort completion with whatever byte count had been
/// observed so far. `Drop` cannot `await`, so this only fires when a tokio
/// runtime is still current; outside one it logs at debug and gives up,
/// matching this subsystem's rule that capture may degrade but must never
/// panic or fail the request that produced it.
///
/// Carries the `Arc<dyn FlowSink>` the client was built with (rather than
/// completing through the free-standing `rupu_netflow::http::complete`,
/// which re-resolves the process-global sink) so this guard's `Complete`
/// line is guaranteed to land in the same ledger as the `Flow` line the
/// middleware wrote for this same `id` — see Fix 1 of the 2026-08-03
/// whole-branch review.
///
/// Only ever constructed AFTER `.send()` has returned `Ok`: a `?` on the
/// `.send()` future itself must never drop an armed guard, or its `Drop`
/// impl fabricates a `bytes_in: Some(0)` completion for a request whose
/// body was never observed — the middleware's own `Err` arm already wrote
/// the honest record (`bytes_in: None, body_complete: true`) in that case.
/// See Fix 2 of the same review.
struct FlowCompletionGuard {
    id: rupu_netflow::FlowId,
    started: std::time::Instant,
    bytes: u64,
    fired: bool,
    sink: Arc<dyn rupu_netflow::FlowSink>,
}

impl FlowCompletionGuard {
    fn new(
        id: rupu_netflow::FlowId,
        started: std::time::Instant,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
        Self {
            id,
            started,
            bytes: 0,
            fired: false,
            sink,
        }
    }

    /// Accumulate observed body bytes as they arrive, so `Drop` has an
    /// accurate count even if the explicit completion below never runs.
    fn add_bytes(&mut self, n: u64) {
        self.bytes += n;
    }

    /// Explicit, precise completion on the normal path. Disarms `Drop`
    /// first — not after — so a cancellation mid-`.await` here cannot also
    /// trigger `Drop`'s fallback completion for the same flow.
    async fn complete(mut self) {
        self.fired = true;
        self.sink
            .complete(
                self.id,
                self.bytes,
                self.started.elapsed().as_millis() as u64,
            )
            .await;
    }
}

impl Drop for FlowCompletionGuard {
    fn drop(&mut self) {
        if self.fired {
            return;
        }
        let id = self.id;
        let bytes = self.bytes;
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let sink = self.sink.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    sink.complete(id, bytes, elapsed_ms).await;
                });
            }
            Err(_) => {
                debug!(
                    ?id,
                    bytes,
                    "netflow: flow dropped mid-flight with no tokio runtime current; \
                     record left incomplete"
                );
            }
        }
    }
}

impl AnthropicClient {
    /// Build and send the authenticated `GET /v1/models` request, asking for
    /// `limit` entries per page (`after_id` continues a listing).
    ///
    /// Shared by the catalog listing ([`LlmProvider::list_models`] /
    /// [`LlmProvider::fetch_models`], via `fetch_all_models`, `limit` =
    /// [`MODELS_PAGE_LIMIT`]) and [`LlmProvider::probe`] (`limit` = 1: the
    /// status is the whole answer, so it must not download the catalog).
    /// Kept as one function so the two can never drift apart in auth handling
    /// — a probe that authenticated differently from the real call would be
    /// testing the wrong thing.
    ///
    /// Deliberately does NOT go through `apply_auth_headers`: that injects
    /// `X-Stainless-*` + `X-Claude-Code-Session-Id` plus a per-request beta
    /// CSV, some of which get this endpoint to reject the request.
    async fn models_request(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> Result<reqwest::Response, ProviderError> {
        // Strip the `/v1/messages` (and optional `?beta=true`) suffix off
        // `api_url` to get the API root, then append `/v1/models`.
        let base = self
            .api_url
            .split('?')
            .next()
            .unwrap_or(&self.api_url)
            .trim_end_matches("/v1/messages")
            .trim_end_matches('/');
        let url = format!("{base}/v1/models");
        let limit = limit.to_string();
        let mut query: Vec<(&str, &str)> = vec![("limit", limit.as_str())];
        if let Some(a) = after_id {
            query.push(("after_id", a));
        }

        let mut req = self
            .client
            .get(&url)
            .query(&query)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Accept", "application/json");
        match &self.auth {
            AuthMethod::ApiKey(key) => {
                req = req.header("x-api-key", key);
            }
            AuthMethod::OAuth { access_token, .. } => {
                req = req
                    .header("Authorization", format!("Bearer {access_token}"))
                    // The `oauth-2025-04-20` beta is what the first-party
                    // Claude client sends on every OAuth request; the
                    // discovery endpoint accepts it and (in some quotas)
                    // requires it.
                    .header("anthropic-beta", "oauth-2025-04-20");
            }
        }

        req.send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))
    }

    /// One-shot, best-effort GET to `/api/claude_cli/bootstrap`. The
    /// reference Claude Code client makes this call once on session
    /// startup; it appears to register the session with Anthropic's
    /// session-management layer and pre-warm the OAuth-quota router. On
    /// rupu we run it from the provider factory immediately after
    /// constructing an OAuth client so the first user-visible
    /// `messages.create` lands with the session already attributed.
    ///
    /// Failure is non-fatal: a warn-level log line is emitted and the
    /// caller proceeds. The call is a no-op for api-key clients (the
    /// bootstrap endpoint is OAuth-only).
    pub async fn bootstrap_oauth_session(&self) {
        if !self.auth.is_oauth() {
            return;
        }
        // Derive the bootstrap URL from `api_url` so test seams
        // (`RUPU_ANTHROPIC_BASE_URL_OVERRIDE`) point at the mock server
        // rather than punching through to api.anthropic.com.
        let bootstrap_url = match self.api_url.find("/v1/messages") {
            Some(idx) => format!("{}/api/claude_cli/bootstrap", &self.api_url[..idx]),
            None => "https://api.anthropic.com/api/claude_cli/bootstrap".to_string(),
        };
        let model_for_betas = "claude-sonnet-4-6";
        let req = self.client.get(&bootstrap_url);
        let req = self.apply_auth_headers(req, model_for_betas, None, false);
        // Cap this best-effort warmup at 10s so a slow / hanging
        // bootstrap endpoint can't strand the agent at the spinner
        // before its first message turn. The shared HTTP client
        // doesn't have a default timeout (streams need to wait
        // arbitrary durations between SSE chunks), so the cap has to
        // be set per-request here. On timeout we log + continue,
        // matching the existing best-effort error semantics — the
        // first /v1/messages call will surface any real auth issue.
        let req = req.timeout(std::time::Duration::from_secs(10));
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!(url = %bootstrap_url, "bootstrap OK");
            }
            Ok(resp) => {
                warn!(
                    status = resp.status().as_u16(),
                    "bootstrap returned non-success; continuing"
                );
            }
            Err(e) if e.is_timeout() => {
                warn!(
                    url = %bootstrap_url,
                    "bootstrap timed out after 10s; continuing without warmup"
                );
            }
            Err(e) => {
                warn!(error = %e, "bootstrap request failed; continuing");
            }
        }
    }

    /// Send a message and get the complete response (non-streaming).
    /// Retries with exponential backoff on 429 rate-limit responses.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request, false);

        let mut last_err = None;
        for attempt in 0..=self.max_rate_limit_retries {
            if attempt > 0 {
                let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                warn!(
                    attempt,
                    backoff_ms = backoff,
                    "rate-limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }

            // Acquired fresh each attempt (I-75): dropped at the end of
            // this loop iteration (including via `continue` on a 429), so
            // the backoff sleep above — on the NEXT iteration — always
            // runs with no permit held.
            let _permit = self.acquire_permit().await?;

            let builder = self
                .client
                .post(&self.api_url)
                .header("Content-Type", "application/json")
                .json(&body);
            let send_fut = self
                .apply_auth_headers(
                    builder,
                    &request.model,
                    request.context_window,
                    request.anthropic_context_management.is_some(),
                )
                .send();
            // Total timeout: a one-shot request that never responds (hung
            // connection / stalled model) must not block forever.
            let response = match tokio::time::timeout(
                std::time::Duration::from_secs(SEND_TOTAL_TIMEOUT_SECS),
                send_fut,
            )
            .await
            {
                Ok(r) => r?,
                Err(_elapsed) => {
                    return Err(ProviderError::Transient(anyhow::anyhow!(
                        "send request timed out after {SEND_TOTAL_TIMEOUT_SECS}s (model stalled)"
                    )))
                }
            };

            let status = response.status();
            if status.as_u16() == 429 {
                let text = response.text().await.unwrap_or_default();
                warn!(
                    attempt,
                    body = text.as_str(),
                    "429 response from Anthropic API"
                );
                last_err = Some(ProviderError::Api {
                    status: 429,
                    message: text,
                });
                continue;
            }
            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                let truncated = if text.len() > 4096 {
                    format!("{}... (truncated)", &text[..4096])
                } else {
                    text
                };
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: truncated,
                });
            }

            let api_response: AnthropicResponse = response.json().await?;
            return Ok(api_response.into_llm_response());
        }

        Err(last_err.unwrap_or_else(|| ProviderError::Api {
            status: 429,
            message: "rate-limited after max retries".into(),
        }))
    }

    /// Send a message with SSE streaming. Calls `on_event` for each stream event.
    /// Returns the complete response after the stream ends.
    ///
    /// The callback is `Send` to support async consumers that may hold the callback
    /// across `.await` boundaries.
    ///
    /// **Note on content block ordering**: Text deltas are accumulated into a single
    /// text block placed first in the response, followed by tool_use blocks. If the
    /// model interleaves text and tool_use, the original ordering is not preserved.
    /// Use `response.text()` and `response.tool_calls()` for access — they don't
    /// depend on ordering.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        mut on_event: impl FnMut(StreamEvent) + Send,
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request, true);

        // I-84: idle-stall resends and 429 resends used to be independently
        // bounded nested loops (an outer `idle_attempt` loop wrapping an
        // inner 429-retry loop), so the worst-case request count was their
        // PRODUCT: (MAX_STREAM_IDLE_RETRIES + 1) * (max_rate_limit_retries +
        // 1) — e.g. 11 * 2 = 22 requests for one `stream()` call at
        // defaults, if every idle-restart's stream also 429ed once before
        // succeeding and then stalled again.
        //
        // Fix: every actual resend — whether it's a 429 retry or an
        // idle-restart — draws down one shared budget,
        // `total_retry_budget` (the SUM, not product, of the two
        // individual ceilings). The inner 429 loop keeps its own
        // `self.max_rate_limit_retries` per-pass cap (so a persistent
        // 429 storm still fails fast, unchanged from before — it never
        // reaches the outer idle logic), but each retry it actually takes
        // also spends shared budget, so a stream that alternates stalls
        // and 429s across multiple passes can't multiply past
        // `total_retry_budget + 1` total requests.
        let total_retry_budget = MAX_STREAM_IDLE_RETRIES + self.max_rate_limit_retries;
        let mut retries_used = 0u32;

        // Outer retry loop: if the model stalls (no bytes for
        // STREAM_IDLE_TIMEOUT_SECS) during prefill — connection open, server
        // silent — abort and re-send, drawing from the shared budget above.
        let mut idle_attempt = 0u32;
        loop {
            if idle_attempt > 0 {
                warn!(
                    attempt = idle_attempt,
                    idle_secs = STREAM_IDLE_TIMEOUT_SECS,
                    "stream stalled (no bytes); re-sending request"
                );
            }

            // Retry loop for 429 rate-limits. Bounded by its own per-pass
            // cap (self.max_rate_limit_retries) exactly as before — a
            // persistent 429 storm fails here without ever touching the
            // outer idle-restart logic. Each retry ALSO spends shared
            // budget so it can't multiply against outer idle-restarts.
            //
            // `_stream_permit` (I-75) carries the successful attempt's
            // concurrency permit out of this block so it stays held for
            // the SSE-reading phase below — matching how `ThrottledProvider`
            // holds a permit for a whole call for every other provider —
            // while every *failed* attempt's permit is acquired fresh and
            // dropped before its own backoff sleep, exactly like `send()`.
            // Never read directly; kept alive purely for its `Drop`, which
            // is why it's underscore-prefixed.
            let (response, _stream_permit, mut flow_guard) = {
                let mut last_err = None;
                let mut winner = None;
                for attempt in 0..=self.max_rate_limit_retries {
                    if attempt > 0 {
                        let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                        warn!(
                            attempt,
                            backoff_ms = backoff,
                            "rate-limited (429), retrying"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                    }

                    let permit = self.acquire_permit().await?;

                    // Every attempt here is a genuinely separate connection —
                    // a 429 retry as much as an idle-restart pass — so each
                    // gets its own fresh `FlowId`. Minted before `.send()` and
                    // attached via `with_extension` so the middleware defers
                    // finalizing this record until we call `complete` below,
                    // instead of (mis)completing it at header time.
                    let attempt_flow_id = rupu_netflow::FlowId::new();
                    let attempt_started = std::time::Instant::now();

                    let builder = self
                        .client
                        .post(&self.api_url)
                        .header("Content-Type", "application/json")
                        .json(&body);
                    let resp = self
                        .apply_auth_headers(
                            builder,
                            &request.model,
                            request.context_window,
                            request.anthropic_context_management.is_some(),
                        )
                        .with_extension(attempt_flow_id)
                        .send()
                        .await?;

                    // Constructed ONLY after `.send()` returned `Ok` — Fix 2
                    // of the 2026-08-03 review. If `.send()` itself fails,
                    // the `?` above returns before any guard exists, so
                    // nothing is armed to fabricate a completion for a body
                    // that was never observed; the middleware's own `Err`
                    // arm already wrote the honest record
                    // (`bytes_in: None, body_complete: true`). A
                    // cancellation DURING `.send()` cannot dangle either:
                    // the whole future — including the not-yet-created
                    // guard — is simply dropped with it, and no record
                    // exists to finalize. Wrapped in a `FlowCompletionGuard`
                    // from here on so a cancellation mid-attempt (e.g. a
                    // user pause) while draining the body still finalizes
                    // the record with whatever was observed, instead of
                    // leaving it dangling.
                    let mut flow_guard = FlowCompletionGuard::new(
                        attempt_flow_id,
                        attempt_started,
                        self.sink.clone(),
                    );

                    let status = resp.status();
                    if status.as_u16() == 429 {
                        let text = resp.text().await.unwrap_or_default();
                        flow_guard.add_bytes(text.len() as u64);
                        flow_guard.complete().await;
                        last_err = Some(ProviderError::Api {
                            status: 429,
                            message: text,
                        });
                        retries_used += 1;
                        continue; // `permit` drops here, before the next iteration's backoff sleep
                    }
                    if !status.is_success() {
                        let text = resp.text().await.unwrap_or_default();
                        let bytes_in = text.len() as u64;
                        let truncated = if text.len() > 4096 {
                            format!("{}... (truncated)", &text[..4096])
                        } else {
                            text
                        };
                        flow_guard.add_bytes(bytes_in);
                        flow_guard.complete().await;
                        return Err(ProviderError::Api {
                            status: status.as_u16(),
                            message: truncated,
                        });
                    }
                    winner = Some((resp, permit, flow_guard));
                    break;
                }
                match winner {
                    Some(w) => w,
                    None => {
                        return Err(last_err.unwrap_or_else(|| ProviderError::Api {
                            status: 429,
                            message: "rate-limited after max retries".into(),
                        }))
                    }
                }
            };

            let mut parser = SseParser::new();
            let mut accumulator = StreamAccumulator::new();
            let mut response = response;
            let mut emitted_content = false;
            // `_stream_permit` (if any) stays alive through the SSE-reading
            // loop below and is dropped when this outer-loop iteration
            // ends — either by returning, or by falling through to the
            // next idle-restart pass, which acquires its own fresh permit.

            // Read chunks with a per-chunk IDLE timeout. A timeout means the
            // server stopped emitting bytes — treat as a stall. Wrapped in an
            // inner async block so the explicit `flow_guard.complete()` below
            // fires exactly once on every NORMAL exit — stream end, idle
            // stall, or any `?` error. If `stream()` itself is cancelled
            // instead (dropped mid-`.await`, e.g. by a user pause racing this
            // call — see rupu-agent's runner), this block never finishes and
            // that explicit call never runs; `flow_guard`'s own `Drop` is
            // what finalizes the record in that case, with whatever byte
            // count had been accumulated via `add_bytes` so far.
            let stalled_result: Result<bool, ProviderError> = async {
                loop {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS),
                        response.chunk(),
                    )
                    .await
                    {
                        Ok(chunk_result) => match chunk_result? {
                            Some(chunk) => {
                                flow_guard.add_bytes(chunk.len() as u64);
                                for event in parser.feed(&chunk)? {
                                    self.process_sse_event(&event, &mut accumulator, &mut |ev| {
                                        if stream_event_counts_as_emitted_content(&ev) {
                                            emitted_content = true;
                                        }
                                        on_event(ev);
                                    })?;
                                }
                            }
                            None => return Ok(false), // stream completed normally
                        },
                        Err(_elapsed) => return Ok(true), // idle timeout → stalled
                    }
                }
            }
            .await;

            flow_guard.complete().await;
            let stalled = stalled_result?;

            if !stalled {
                return accumulator
                    .into_response()
                    .ok_or(ProviderError::UnexpectedEndOfStream);
            }

            // Stalled. Retry only on a *prefill* stall (nothing emitted yet)
            // with budget left; a mid-stream stall can't be re-sent without
            // duplicating already-streamed output, so it fails regardless of
            // budget.
            if !should_retry_idle_stall(emitted_content, retries_used, total_retry_budget) {
                let reason = if emitted_content {
                    "mid-response".to_string()
                } else {
                    format!("during prefill, after {idle_attempt} retries")
                };
                return Err(ProviderError::Transient(anyhow::anyhow!(
                    "stream idle for {STREAM_IDLE_TIMEOUT_SECS}s — model stalled ({reason})"
                )));
            }
            retries_used += 1;
            idle_attempt += 1;
        }
    }

    fn build_request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        // Walk the conversation history once, sanitizing every ToolUse
        // block's `name` to its wire-safe form. Internal state still uses
        // the canonical "scm.repos.list" form; only the wire payload sees
        // the escaped variant.
        //
        // Then restore internal `Reasoning` blocks to Anthropic's own wire
        // shape (and drop `Unknown` blocks) — same post-process pattern, on the
        // same serialized array.
        let messages_value: serde_json::Value =
            serde_json::to_value(&request.messages).unwrap_or_else(|_| serde_json::json!([]));
        let mut messages_value = sanitize_messages_tool_names(messages_value);
        // Must run after `sanitize_messages_tool_names`: restoring reasoning
        // blocks last guarantees `raw` is echoed byte-exact and is never
        // touched by the tool-name sanitizer's block-rewriting pass.
        restore_reasoning_blocks(&mut messages_value, PROVIDER_TAG);

        let max_tokens = request
            .max_tokens
            .unwrap_or(crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS);
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": max_tokens,
            "messages": messages_value,
            "stream": stream,
        });

        // System prompts go as an array of TextBlock-shaped objects rather
        // than a bare string. Both shapes are accepted by Anthropic, but the
        // block form is what `@anthropic-ai/sdk` emits and is the shape that
        // carries per-block `cache_control` (see `apply_cache_breakpoints`).
        //
        // For OAuth requests, prepend the canonical "You are Claude Code,
        // …" prefix block as system[0] when the per-client toggle is on
        // (default). This is the first-party-identity signal Anthropic's
        // OAuth quota router keys on; without it Pro/Max users still
        // bind to a stricter pool even with `account_uuid`. Per-agent
        // opt-out via `anthropicOauthPrefix: false` in frontmatter.
        let mut system_blocks: Vec<serde_json::Value> = Vec::new();
        if self.auth.is_oauth() && self.oauth_system_prefix_enabled {
            // The billing-attribution block + Claude-Agent-SDK self-
            // description are what get OAuth requests recognized as
            // Claude-Code-billable. See the constant docstrings above
            // for the full WAF/billing rationale and the TODO around
            // re-MITM'ing if upstream rotates these values. Per-agent
            // opt-out via `anthropicOauthPrefix: false` for the rare
            // case where the persona must not be augmented (and the
            // user accepts the risk of falling back into the WAF
            // reject pool).
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": ANTHROPIC_BILLING_HEADER_BLOCK,
            }));
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION,
            }));
        }
        if let Some(system) = &request.system {
            system_blocks.push(serde_json::json!({ "type": "text", "text": system }));
        }
        if !system_blocks.is_empty() {
            body["system"] = serde_json::Value::Array(system_blocks);
        }

        if !request.tools.is_empty() {
            // Sanitize each tool's name on the wire. Description and
            // input_schema are unaffected.
            let tools_array: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": sanitize_anthropic_tool_name(&t.name),
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools_array);
        }

        // OAuth-scope parity with the first-party Claude client: carry a
        // `metadata.user_id` blob so Anthropic can attribute the request to
        // the user's Pro/Max quota. The OAuth beta itself goes on the wire
        // exclusively as the `anthropic-beta` header (set by
        // `apply_auth_headers`); putting it in the body too is rejected by
        // /v1/messages with `"betas: Extra inputs are not permitted"` —
        // `betas` is an `@anthropic-ai/sdk` call-options field that the SDK
        // strips into the header before sending, not a wire-level body
        // field.
        //
        // `account_uuid` is captured from the OAuth token-exchange
        // response (see `oauth/callback.rs::TokenResponse`) and threaded
        // here via `with_oauth_account_uuid`. Without it Anthropic cannot
        // bind the call to the authenticated account's quota and falls
        // back to a default-pool rate limit — that was the second 429 we
        // saw post-#24.
        if self.auth.is_oauth() {
            let mut user_id = serde_json::json!({
                "device_id": "rupu",
                "session_id": request.cell_id.clone().unwrap_or_default(),
            });
            if let Some(uuid) = &self.oauth_account_uuid {
                user_id["account_uuid"] = serde_json::Value::String(uuid.clone());
            }
            body["metadata"] = serde_json::json!({ "user_id": user_id.to_string() });
        }

        // Thinking/extended thinking. Anthropic accepts two shapes:
        //   * `thinking.type: "adaptive"` — server picks the budget
        //     (Opus/Sonnet 4 OAuth path).
        //   * `thinking.type: "enabled"` + `budget_tokens: <n>` — fixed
        //     budget. Must be >= 1024 (API minimum); we silently skip
        //     thinking when the clamped budget falls below that.
        //   * `display: "summarized"` — opt in to readable thinking text.
        //     Only ever set alongside "adaptive": `display` accepts exactly
        //     "summarized" | "omitted" (there is no raw/full — the raw chain of
        //     thought is not exposed on any Claude model), and the
        //     budget_tokens path targets pre-4.6 models that predate it.
        //     Without this, display defaults to "omitted" on Opus 4.7/4.8 and
        //     Sonnet 5, whose thinking blocks then carry an empty text field.
        if let Some(level) = &request.thinking {
            use crate::model_tier::ThinkingLevel;
            match level {
                ThinkingLevel::Auto => {
                    body["thinking"] =
                        serde_json::json!({ "type": "adaptive", "display": "summarized" });
                }
                _ => {
                    let raw_budget = match level {
                        ThinkingLevel::Minimal => 0,
                        ThinkingLevel::Low => 2000,
                        ThinkingLevel::Medium => 5000,
                        ThinkingLevel::High => 10000,
                        ThinkingLevel::Max => max_tokens.saturating_sub(2000),
                        ThinkingLevel::Auto => unreachable!(),
                    };
                    if raw_budget > 0 {
                        let clamped = raw_budget.min(max_tokens);
                        if clamped >= 1024 {
                            body["thinking"] = serde_json::json!({
                                "type": "enabled",
                                "budget_tokens": clamped,
                            });
                        }
                    }
                }
            }
        } else if self.auth.is_oauth() && model_supports_adaptive_thinking(&request.model) {
            // claude-cli always emits `thinking: {"type":"adaptive"}` on
            // OAuth requests to Opus / Sonnet 4-tier models when no explicit
            // budget is requested (see services/api/claude.ts:1609-1613 in
            // the reference). Without this the request fingerprints differently
            // and may be down-classified by the OAuth quota router (a 429, not
            // a 400) — that fingerprinting warning is still true and load-bearing.
            //
            // The `display: "summarized"` addition below is an intentional,
            // verified deviation from that pinned claude-cli shape (its wire
            // shape is confirmed against the authoritative API reference, same
            // as the `ThinkingLevel::Auto` arm above). Do NOT "restore" this to
            // bare `{"type":"adaptive"}` to match claude-cli more closely: on
            // Opus 4.7/4.8 and Sonnet 5, `display` defaults to "omitted", and
            // without an explicit "summarized" here captured reasoning text
            // comes back empty on those models.
            body["thinking"] = serde_json::json!({ "type": "adaptive", "display": "summarized" });
        }

        // ── Optional output-shape / context / speed knobs ──────────────
        // Each block is emitted only when the corresponding agent-
        // frontmatter field was set, so we keep the wire payload
        // minimal for agents that don't opt in. Field shapes track
        // what the official `@anthropic-ai/sdk` emits when the
        // matching SDK option is enabled.

        // `output_config` carries `task_budget` and, when the agent
        // declares a schema, `format`. We deliberately do NOT map
        // `request.output_format` on its own: Anthropic's
        // structured-outputs API requires `output_config.format` to be
        // an object of the shape `{"type": "json_schema", "schema":
        // <full JSON Schema>}` — there is no schema-less JSON mode.
        // The `OutputFormat` enum (`Json`/`Text`) carries no schema, so
        // by itself there is nothing valid to send; emitting a bare
        // string 400s every request ("output_config.format: Input does
        // not match the expected shape"). `request.output_schema`
        // (from the agent's `outputSchema` frontmatter) is what
        // actually carries the schema, so `format` is only emitted
        // when it is `Some`. Agents with `outputFormat: json` and no
        // `outputSchema` keep today's prompt-driven-only behavior.
        let mut output_config = serde_json::Map::new();
        if let Some(budget) = request.anthropic_task_budget {
            output_config.insert(
                "task_budget".to_string(),
                serde_json::Value::Number(serde_json::Number::from(budget)),
            );
        }
        if let Some(schema) = &request.output_schema {
            // Anthropic's structured-outputs schema is a strict subset of
            // JSON Schema and rejects numeric bounds (`minimum`/`maximum`/…)
            // on `integer`/`number` nodes — a schema that is otherwise valid
            // (and accepted verbatim by OpenAI) 400s here. Down-convert to
            // the supported subset so the same `outputSchema` is portable
            // across providers rather than making authors hand-tune per
            // provider. See `sanitize_output_schema_for_anthropic`.
            let mut schema = schema.clone();
            let removed = sanitize_output_schema_for_anthropic(&mut schema);
            if removed > 0 {
                warn!(
                    removed,
                    "stripped {removed} Anthropic-unsupported numeric-constraint keyword(s) \
                     (minimum/maximum/exclusiveMinimum/exclusiveMaximum/multipleOf) from \
                     outputSchema before sending; Anthropic structured outputs does not accept \
                     numeric bounds on integer/number nodes"
                );
            }
            output_config.insert(
                "format".to_string(),
                serde_json::json!({ "type": "json_schema", "schema": schema }),
            );
        }
        if !output_config.is_empty() {
            body["output_config"] = serde_json::Value::Object(output_config);
        }

        // `context_management.type: "tool_clearing"` lets the server
        // transparently drop earlier `tool_use` / `tool_result`
        // blocks when the conversation would otherwise overflow.
        if let Some(strat) = request.anthropic_context_management {
            let strat_str = match strat {
                crate::types::ContextManagement::ToolClearing => "tool_clearing",
            };
            body["context_management"] = serde_json::json!({ "type": strat_str });
        }

        // Top-level `speed: "fast"` toggle — account-gated. Sending
        // from an account without the feature returns 400, so we
        // only emit when the agent explicitly opted in.
        if let Some(speed) = request.anthropic_speed {
            let speed_str = match speed {
                crate::types::Speed::Fast => "fast",
            };
            body["speed"] = serde_json::Value::String(speed_str.to_string());
        }

        // Must stay LAST: it marks the final shape of `system` / `tools` /
        // `messages` (post-sanitizing, post-reasoning-restoration, post-OAuth
        // prefix) and touches only the chosen blocks' `cache_control` key.
        // A per-request opt-out (`disable_prompt_cache`) wins over the
        // client-level flag.
        if self.prompt_cache_enabled && !request.disable_prompt_cache {
            apply_cache_breakpoints(&mut body);
        }

        body
    }

    fn process_sse_event(
        &self,
        event: &crate::sse::SseEvent,
        acc: &mut StreamAccumulator,
        on_event: &mut impl FnMut(StreamEvent),
    ) -> Result<(), ProviderError> {
        match event.event_type.as_str() {
            "message_start" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(msg) = data.get("message") {
                    acc.id = msg
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    acc.model = msg
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Some(usage) = msg.get("usage") {
                        acc.wire = serde_json::from_value(usage.clone()).unwrap_or_default();
                        // message_start's `output_tokens` is a placeholder
                        // (real streams send 1). Output is authoritative only
                        // from `message_delta`; a nonzero placeholder would
                        // freeze the live output estimate, which only
                        // updates while output == 0.
                        acc.wire.output_tokens = 0;
                        // Anthropic's output_tokens already includes reasoning
                        // tokens, so reasoning_tokens stays at 0 — see the
                        // contrast note on `Usage::reasoning_tokens`.
                        on_event(StreamEvent::UsageSnapshot(acc.wire.normalize()));
                    }
                }
            }
            "content_block_start" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(block) = data.get("content_block") {
                    let block_type = block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    match block_type {
                        "tool_use" => {
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            // Wire-form names may carry the __dot__ escape from
                            // build_request_body's sanitization. Reverse it before
                            // surfacing to the dispatcher / accumulator.
                            let name = desanitize_anthropic_tool_name(
                                block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default(),
                            );
                            acc.current_tool_id = Some(id.clone());
                            acc.current_tool_name = Some(name.clone());
                            acc.current_tool_input.clear();
                            on_event(StreamEvent::ToolUseStart { id, name });
                        }
                        "thinking" => {
                            // Seed with any text present on the start event; deltas append.
                            acc.current_reasoning_text = Some(
                                block
                                    .get("thinking")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                            acc.current_reasoning_signature = block
                                .get("signature")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                        "redacted_thinking" => {
                            // Opaque and self-contained: no deltas follow. Push
                            // immediately, preserving the block verbatim for the echo.
                            acc.content_blocks.push(ContentBlock::Reasoning {
                                text: None,
                                provider: PROVIDER_TAG.to_string(),
                                model: acc.model.clone(),
                                raw: block.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(delta) = data.get("delta") {
                    let delta_type = delta
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                acc.text.push_str(text);
                                on_event(StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                        "input_json_delta" => {
                            if let Some(json) = delta.get("partial_json").and_then(|v| v.as_str()) {
                                acc.current_tool_input.push_str(json);
                                on_event(StreamEvent::InputJsonDelta(json.to_string()));
                            }
                        }
                        "thinking_delta" => {
                            if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                                // Only append to a buffer that a `content_block_start`
                                // of type "thinking" actually opened. Using
                                // `get_or_insert_with` here would fabricate a buffer
                                // for any future block type that also emits
                                // `thinking_delta` but was dropped by the `_ => {}`
                                // arm above — `content_block_stop` would then push a
                                // `{"type":"thinking", ...}` raw block that was never
                                // really opened, and Anthropic rejects that block when
                                // it's echoed back verbatim.
                                if let Some(buf) = acc.current_reasoning_text.as_mut() {
                                    buf.push_str(t);
                                    on_event(StreamEvent::ReasoningDelta(t.to_string()));
                                }
                            }
                        }
                        "signature_delta" => {
                            if let Some(sig) = delta.get("signature").and_then(|v| v.as_str()) {
                                // Signatures arrive whole, but append defensively rather
                                // than overwrite: a truncated signature is rejected by
                                // the API.
                                acc.current_reasoning_signature
                                    .get_or_insert_with(String::new)
                                    .push_str(sig);
                            }
                        }
                        _ => {
                            debug!(delta_type, "unknown delta type");
                        }
                    }
                }
            }
            "content_block_stop" => {
                // Finalize a pending thinking block. Reconstruct raw in Anthropic's
                // own wire shape so it echoes back byte-identical to what arrived.
                // Note the block is emitted even when the text is empty
                // (display: "omitted") — it must still round-trip.
                if let Some(text) = acc.current_reasoning_text.take() {
                    let mut raw = serde_json::json!({ "type": "thinking", "thinking": text });
                    if let Some(sig) = acc.current_reasoning_signature.take() {
                        raw["signature"] = serde_json::Value::String(sig);
                    }
                    acc.content_blocks.push(ContentBlock::Reasoning {
                        text: if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        },
                        provider: PROVIDER_TAG.to_string(),
                        model: acc.model.clone(),
                        raw,
                    });
                }

                // If we were accumulating a tool use, finalize it
                if let (Some(id), Some(name)) =
                    (acc.current_tool_id.take(), acc.current_tool_name.take())
                {
                    // Architect fix: propagate JSON error instead of silent default
                    let input: serde_json::Value = if acc.current_tool_input.is_empty() {
                        serde_json::Value::Object(serde_json::Map::new())
                    } else {
                        serde_json::from_str(&acc.current_tool_input).map_err(|e| {
                            ProviderError::Json(format!(
                                "malformed tool input JSON for tool '{}': {}",
                                name, e
                            ))
                        })?
                    };
                    acc.content_blocks
                        .push(ContentBlock::ToolUse { id, name, input });
                    acc.current_tool_input.clear();
                }
            }
            "message_delta" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                        acc.stop_reason =
                            serde_json::from_value(serde_json::Value::String(reason.to_string()))
                                .ok();
                    }
                }
                if let Some(usage) = data.get("usage") {
                    let field = |k: &str| usage.get(k).and_then(|v| v.as_u64());
                    acc.wire.output_tokens = field("output_tokens").unwrap_or(0) as u32;
                    // Newer API versions repeat the input/cache fields in the
                    // delta with their final values — overwrite when present.
                    if let Some(n) = field("input_tokens") {
                        acc.wire.input_tokens = n as u32;
                    }
                    if let Some(n) = field("cache_read_input_tokens") {
                        acc.wire.cache_read_input_tokens = n as u32;
                    }
                    if let Some(n) = field("cache_creation_input_tokens") {
                        acc.wire.cache_creation_input_tokens = n as u32;
                    }
                    // Anthropic's output_tokens already includes reasoning
                    // tokens, so reasoning_tokens stays at 0 — see the
                    // contrast note on `Usage::reasoning_tokens`.
                    on_event(StreamEvent::UsageSnapshot(acc.wire.normalize()));
                }
            }
            "message_stop" | "ping" => {}
            other => {
                debug!(event_type = other, "unhandled SSE event type");
            }
        }
        Ok(())
    }

    /// Every page of `GET /v1/models`, with limits (spec 2026-09-30 §3).
    async fn fetch_all_models(&self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        #[derive(serde::Deserialize)]
        struct Page {
            data: Vec<Entry>,
            #[serde(default)]
            has_more: bool,
            #[serde(default)]
            last_id: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            id: String,
            #[serde(default)]
            max_input_tokens: Option<u32>,
            #[serde(default)]
            max_tokens: Option<u32>,
        }
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        // Every cursor already requested: a server that cycles (A -> B -> A)
        // would otherwise re-collect the same models until the page cap.
        let mut seen_cursors: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Bounded as well: a server that never repeats a cursor must not loop
        // forever either.
        for page_num in 0..50 {
            let resp = self
                .models_request(after.as_deref(), MODELS_PAGE_LIMIT)
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let message: String = resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(500)
                    .collect();
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }
            let body = resp
                .text()
                .await
                .map_err(|e| ProviderError::Http(e.to_string()))?;
            let page: Page = crate::error::parse_listing_json("anthropic", &body)?;
            out.extend(page.data.into_iter().map(|e| crate::model_pool::ModelInfo {
                id: e.id,
                provider: ProviderId::Anthropic,
                context_window: e.max_input_tokens.unwrap_or(0),
                max_output_tokens: e.max_tokens.unwrap_or(0),
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            }));
            match (page.has_more, page.last_id) {
                (false, _) => break, // Normal end: has_more is false
                (true, None) => {
                    // Edge case (b): has_more but no last_id
                    warn!(
                        "anthropic models pagination stopped: has_more=true but no last_id; \
                         returning {} models collected so far",
                        out.len()
                    );
                    break;
                }
                (true, Some(ref new_last)) => {
                    // Check for a repeated cursor (edge case a): the one just
                    // requested, or any earlier one (a cycle).
                    if !seen_cursors.insert(new_last.clone()) {
                        warn!(
                            provider = "anthropic",
                            cursor = %new_last,
                            collected = out.len(),
                            "models pagination stopped: cursor already seen; \
                             returning the models collected so far"
                        );
                        break;
                    }
                    // Check if we're about to hit the cap (edge case c)
                    if page_num == 49 {
                        warn!(
                            "anthropic models pagination stopped: reached 50-page limit with has_more=true; \
                             returning {} models collected so far",
                            out.len()
                        );
                        break;
                    }
                    after = Some(new_last.clone());
                }
            }
        }
        Ok(out)
    }
}

/// Anthropic's wire usage. Its `input_tokens` EXCLUDES cache reads and cache
/// writes; `normalize` folds both back in so every provider's
/// `Usage.input_tokens` means "the whole prompt".
#[derive(Debug, Clone, Default, Deserialize)]
struct AnthropicWireUsage {
    #[serde(default, deserialize_with = "null_as_zero")]
    input_tokens: u32,
    #[serde(default, deserialize_with = "null_as_zero")]
    output_tokens: u32,
    #[serde(default, deserialize_with = "null_as_zero")]
    cache_read_input_tokens: u32,
    #[serde(default, deserialize_with = "null_as_zero")]
    cache_creation_input_tokens: u32,
}

/// `#[serde(default)]` covers a missing key, not an explicit `null` — and the
/// Anthropic SDK types declare the cache fields optional. Map `null` to 0 so
/// one null counter cannot zero the others (streaming) or fail the whole
/// response parse (non-streaming).
fn null_as_zero<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<u32>::deserialize(d)?.unwrap_or(0))
}

impl AnthropicWireUsage {
    fn normalize(&self) -> Usage {
        Usage {
            input_tokens: self
                .input_tokens
                .saturating_add(self.cache_read_input_tokens)
                .saturating_add(self.cache_creation_input_tokens),
            output_tokens: self.output_tokens,
            cached_tokens: self.cache_read_input_tokens,
            cache_write_tokens: self.cache_creation_input_tokens,
            reasoning_tokens: 0,
        }
    }
}

/// Accumulates SSE events into a complete LlmResponse.
///
/// **Ordering limitation**: Text deltas are accumulated into a single text block
/// placed after any leading reasoning blocks; tool use blocks follow. If the model
/// interleaves text and tool_use blocks, the original ordering is not preserved.
/// That remains acceptable because the agent loop reads `response.text()` /
/// `response.tool_calls()`, which don't depend on order — but reasoning-before-text
/// *is* load-bearing: Anthropic requires thinking blocks first in an assistant turn
/// and rejects a rebuilt turn that echoes them out of order.
#[derive(Debug, Default)]
struct StreamAccumulator {
    id: String,
    model: String,
    text: String,
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    wire: AnthropicWireUsage,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
    current_reasoning_text: Option<String>,
    current_reasoning_signature: Option<String>,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self::default()
    }

    fn into_response(mut self) -> Option<LlmResponse> {
        if self.id.is_empty() {
            return None;
        }

        if !self.text.is_empty() {
            // Anthropic requires reasoning blocks first in an assistant turn,
            // so text goes after any leading reasoning — not at index 0. (The
            // old `insert(0, ..)` predates reasoning capture, when nothing
            // depended on block order.)
            let idx = self
                .content_blocks
                .iter()
                .position(|b| !matches!(b, ContentBlock::Reasoning { .. }))
                .unwrap_or(self.content_blocks.len());
            self.content_blocks
                .insert(idx, ContentBlock::Text { text: self.text });
        }

        Some(LlmResponse {
            id: self.id,
            model: self.model,
            content: self.content_blocks,
            stop_reason: self.stop_reason,
            usage: self.wire.normalize(),
        })
    }
}

/// Parse Anthropic's wire content blocks into rupu's internal representation.
///
/// Explicit rather than derived: `ContentBlock`'s serde is rupu's internal
/// format, not Anthropic's wire format, and the two must not be coupled.
fn parse_content_blocks(raw: Vec<serde_json::Value>, model: &str) -> Vec<ContentBlock> {
    raw.into_iter()
        .filter_map(|block| {
            let block_type = block
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match block_type {
                "text" => Some(ContentBlock::Text {
                    text: block
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                }),
                "tool_use" => Some(ContentBlock::ToolUse {
                    id: block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    // NOTE: deliberately NOT desanitized — the derive path this
                    // replaces never desanitized either, and changing that is a
                    // separate behavior change (see spec, latent defect #4).
                    name: block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    input: block
                        .get("input")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }),
                "thinking" | "redacted_thinking" => {
                    let text = block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .filter(|t| !t.is_empty())
                        .map(|t| t.to_string());
                    Some(ContentBlock::Reasoning {
                        text,
                        provider: PROVIDER_TAG.to_string(),
                        model: model.to_string(),
                        raw: block,
                    })
                }
                other => {
                    debug!(block_type = other, "dropping unrecognized content block");
                    None
                }
            }
        })
        .collect()
}

/// Anthropic API response (non-streaming).
///
/// `content` stays as raw JSON: `ContentBlock`'s derive is rupu's internal
/// format and cannot represent Anthropic's `thinking` blocks, so the blocks
/// are translated explicitly by `parse_content_blocks`.
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<serde_json::Value>,
    stop_reason: Option<StopReason>,
    usage: AnthropicWireUsage,
}

impl AnthropicResponse {
    fn into_llm_response(self) -> LlmResponse {
        let content = parse_content_blocks(self.content, &self.model);
        LlmResponse {
            id: self.id,
            model: self.model,
            content,
            stop_reason: self.stop_reason,
            usage: self.usage.normalize(),
        }
    }
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for AnthropicClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        AnthropicClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        // &mut dyn FnMut implements FnMut, so this delegates to the generic inherent method.
        AnthropicClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-6"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    /// List available models via the public `/v1/models` endpoint.
    /// Works on both api-key (`x-api-key`) and OAuth (`Authorization:
    /// Bearer`) auth — the discovery endpoint accepts either.
    /// Returns an empty vec on transport / auth / parse failure
    /// rather than propagating, so the CLI's "show what we got"
    /// fallback still renders the baked-in list.
    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        match self.fetch_all_models().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "anthropic list_models failed");
                Vec::new()
            }
        }
    }

    /// Probe via the same authenticated `GET /v1/models` call `list_models`
    /// makes — but here the status IS the answer, so nothing is swallowed. A
    /// 2xx means the credential works, even if the account lists no models.
    async fn probe(&self) -> Result<(), ProviderError> {
        let resp = self.models_request(None, 1).await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        // Bound the body: this string ends up in a cache the dashboard reads,
        // not in a log the operator greps.
        let message = resp
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect::<String>();
        Err(ProviderError::Api {
            status: status.as_u16(),
            message,
        })
    }

    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        self.ensure_valid_token().await?;
        self.fetch_all_models().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_anthropic_tool_name_escapes_dots() {
        assert_eq!(
            sanitize_anthropic_tool_name("scm.repos.list"),
            "scm__dot__repos__dot__list"
        );
        assert_eq!(
            sanitize_anthropic_tool_name("issues.create"),
            "issues__dot__create"
        );
    }

    #[test]
    fn sanitize_anthropic_tool_name_passes_through_safe_names() {
        assert_eq!(sanitize_anthropic_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_anthropic_tool_name("bash"), "bash");
        assert_eq!(sanitize_anthropic_tool_name("write-file"), "write-file");
    }

    #[test]
    fn anthropic_tool_name_round_trip() {
        for name in [
            "scm.repos.list",
            "scm.branches.list",
            "issues.create",
            "github.workflows_dispatch",
            "read_file",
            "bash",
        ] {
            let escaped = sanitize_anthropic_tool_name(name);
            let unescaped = desanitize_anthropic_tool_name(&escaped);
            assert_eq!(unescaped, name, "round-trip failed for {name}");
        }
    }

    #[test]
    fn sanitize_messages_tool_names_rewrites_tool_use_blocks() {
        let input = serde_json::json!([
            {
                "role": "user",
                "content": [{"type": "text", "text": "hi"}]
            },
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "let me check"},
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "scm.repos.list",
                        "input": {"platform": "github"}
                    }
                ]
            },
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}
                ]
            }
        ]);
        let out = sanitize_messages_tool_names(input);
        // Assistant tool_use name was rewritten.
        assert_eq!(
            out[1]["content"][1]["name"].as_str().unwrap(),
            "scm__dot__repos__dot__list"
        );
        // Other fields untouched.
        assert_eq!(out[0]["content"][0]["text"], "hi");
        assert_eq!(out[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn build_request_body_sanitizes_tool_names() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            system: None,
            tools: vec![ToolDefinition {
                name: "scm.repos.list".into(),
                description: "list repos".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            max_tokens: Some(1024),
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["tools"][0]["name"].as_str().unwrap(),
            "scm__dot__repos__dot__list"
        );
    }

    #[test]
    fn beta_csv_omits_context_1m_for_default_model() {
        // Regression: shipping `context-1m-2025-08-07` on a stock 200K
        // model triggers a 429 `"Extra usage is required for long
        // context requests"` on accounts without extra-usage billing.
        let csv = build_oauth_beta_csv("claude-sonnet-4-6", false);
        assert!(
            !csv.contains("context-1m-2025-08-07"),
            "context-1m must not be sent without [1m] suffix; got: {csv}"
        );
        assert!(csv.contains("oauth-2025-04-20"));
        assert!(csv.contains("claude-code-20250219"));
    }

    #[test]
    fn beta_csv_includes_context_1m_when_suffix_present() {
        let csv = build_oauth_beta_csv("claude-sonnet-4-6[1m]", false);
        assert!(
            csv.contains("context-1m-2025-08-07"),
            "context-1m must be sent when [1m] suffix is present; got: {csv}"
        );
    }

    #[test]
    fn beta_csv_drops_claude_code_for_haiku() {
        let csv = build_oauth_beta_csv("claude-haiku-4-5", false);
        assert!(
            !csv.contains("claude-code-20250219"),
            "claude-code beta is gated to non-Haiku tiers; got: {csv}"
        );
    }

    #[test]
    fn beta_csv_omits_4_tier_betas_for_pre_4_models() {
        let csv = build_oauth_beta_csv("claude-3-5-sonnet-20241022", false);
        assert!(
            !csv.contains("interleaved-thinking-2025-05-14"),
            "interleaved-thinking must be 4-tier+; got: {csv}"
        );
        assert!(
            !csv.contains("context-management-2025-06-27"),
            "context-management must be 4-tier+; got: {csv}"
        );
    }

    #[test]
    fn beta_csv_includes_context_management_when_explicitly_opted_in() {
        // A pre-4 model that wouldn't normally get context-management-2025-06-27
        // should include it when wants_context_management is true.
        let csv = build_oauth_beta_csv("claude-3-5-sonnet-20241022", true);
        assert!(
            csv.contains("context-management-2025-06-27"),
            "explicit opt-in must include context-management beta; got: {csv}"
        );
    }

    #[test]
    fn test_with_url_constructor() {
        let client = AnthropicClient::with_url(
            "test-key".into(),
            "http://localhost:8080/v1/messages".into(),
            Arc::new(rupu_netflow::NullSink),
        );
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["model"], "claude-sonnet-4-6");
    }

    #[test]
    fn test_build_request_body_minimal() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hello")],
            max_tokens: Some(1024),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], false);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn unset_max_tokens_sends_the_anthropic_fallback() {
        let client = AnthropicClient::with_url(
            "k".into(),
            "http://x/v1/messages".into(),
            Arc::new(rupu_netflow::NullSink),
        );
        let mut req = make_request(None);
        req.max_tokens = None;
        let body = client.build_request_body(&req, false);
        assert_eq!(
            body["max_tokens"],
            crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS
        );
        req.max_tokens = Some(64_000);
        let body = client.build_request_body(&req, false);
        assert_eq!(body["max_tokens"], 64_000);
    }

    #[test]
    fn test_build_request_body_with_system_and_tools() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: Some("You are helpful.".into()),
            messages: vec![Message::user("hello")],
            max_tokens: Some(4096),
            tools: vec![ToolDefinition {
                name: "test".into(),
                description: "A test tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            cell_id: Some("test-cell".into()),
            trace_id: Some("trace-123".into()),
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, true);
        // Prompt caching is on by default: the (last) system block carries
        // the prefix breakpoint.
        assert_eq!(
            body["system"],
            serde_json::json!([{
                "type": "text",
                "text": "You are helpful.",
                "cache_control": { "type": "ephemeral" },
            }])
        );
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
    }

    fn oauth_client() -> AnthropicClient {
        AnthropicClient::from_auth(
            AuthMethod::OAuth {
                access_token: "oauth-access".into(),
                refresh_token: "oauth-refresh".into(),
                expires_ms: 0,
            },
            Arc::new(rupu_netflow::NullSink),
        )
    }

    fn oauth_client_with_account_uuid(uuid: &str) -> AnthropicClient {
        oauth_client().with_oauth_account_uuid(Some(uuid.to_string()))
    }

    #[test]
    fn oauth_body_includes_metadata_user_id() {
        let client = oauth_client();
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: Some("cell-abc".into()),
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        // `betas` belongs in the `anthropic-beta` header only — including it
        // in the body trips Anthropic's "Extra inputs are not permitted".
        assert!(
            body.get("betas").is_none(),
            "`betas` must not appear in the body — it is a header-only field"
        );
        let user_id = body["metadata"]["user_id"]
            .as_str()
            .expect("metadata.user_id should be a JSON string");
        let parsed: serde_json::Value = serde_json::from_str(user_id).expect("user_id JSON");
        assert_eq!(parsed["device_id"], "rupu");
        assert_eq!(parsed["session_id"], "cell-abc");
    }

    #[test]
    fn api_key_body_omits_oauth_only_fields() {
        let client = AnthropicClient::new("sk-ant-test".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: Some("cell-abc".into()),
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("betas").is_none(),
            "betas leaked into api-key request"
        );
        assert!(
            body.get("metadata").is_none(),
            "metadata leaked into api-key request"
        );
    }

    #[test]
    fn oauth_metadata_user_id_includes_account_uuid_when_set() {
        let client = oauth_client_with_account_uuid("acct-12345");
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: Some("cell-xyz".into()),
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        let user_id = body["metadata"]["user_id"]
            .as_str()
            .expect("metadata.user_id should be a JSON string");
        let parsed: serde_json::Value = serde_json::from_str(user_id).expect("user_id JSON");
        assert_eq!(parsed["account_uuid"], "acct-12345");
        assert_eq!(parsed["device_id"], "rupu");
        assert_eq!(parsed["session_id"], "cell-xyz");
    }

    #[test]
    fn oauth_metadata_user_id_omits_account_uuid_when_unset() {
        let client = oauth_client();
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        let user_id = body["metadata"]["user_id"]
            .as_str()
            .expect("metadata.user_id should be a JSON string");
        let parsed: serde_json::Value = serde_json::from_str(user_id).expect("user_id JSON");
        assert!(
            parsed.get("account_uuid").is_none(),
            "account_uuid must not appear when not configured"
        );
    }

    fn make_request(system: Option<&str>) -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: system.map(str::to_string),
            messages: vec![Message::user("hi")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        }
    }

    #[test]
    fn oauth_request_prepends_billing_attribution_blocks_by_default() {
        let client = oauth_client();
        let body = client.build_request_body(&make_request(Some("agent persona")), false);
        let blocks = body["system"].as_array().expect("system is array");
        assert_eq!(
            blocks.len(),
            3,
            "expected billing block + sdk-self-description block + agent block"
        );
        assert_eq!(blocks[0]["text"], ANTHROPIC_BILLING_HEADER_BLOCK);
        assert_eq!(blocks[1]["text"], ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION);
        assert_eq!(blocks[2]["text"], "agent persona");
    }

    #[test]
    fn oauth_request_skips_billing_blocks_when_opted_out() {
        let client = oauth_client().with_oauth_system_prefix(false);
        let body = client.build_request_body(&make_request(Some("agent persona")), false);
        let blocks = body["system"].as_array().expect("system is array");
        assert_eq!(blocks.len(), 1, "expected agent block only");
        assert_eq!(blocks[0]["text"], "agent persona");
    }

    #[test]
    fn api_key_request_never_emits_billing_blocks() {
        let client = AnthropicClient::new("sk-ant-test".into(), Arc::new(rupu_netflow::NullSink));
        let body = client.build_request_body(&make_request(Some("agent persona")), false);
        let blocks = body["system"].as_array().expect("system is array");
        assert_eq!(
            blocks.len(),
            1,
            "api-key requests must not carry OAuth billing blocks"
        );
        assert_eq!(blocks[0]["text"], "agent persona");
    }

    #[test]
    fn oauth_request_billing_blocks_emit_even_with_no_agent_system() {
        let client = oauth_client();
        let body = client.build_request_body(&make_request(None), false);
        let blocks = body["system"].as_array().expect("system is array");
        assert_eq!(blocks.len(), 2, "expected billing + sdk blocks only");
        assert_eq!(blocks[0]["text"], ANTHROPIC_BILLING_HEADER_BLOCK);
        assert_eq!(blocks[1]["text"], ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION);
    }

    #[test]
    fn test_build_request_body_thinking_low() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("low effort")],
            max_tokens: Some(8000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Low),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 2000);
    }

    #[test]
    fn adaptive_thinking_requests_summarized_display() {
        // Auto → `thinking.type: "adaptive"` regardless of auth mode, plus
        // `display: "summarized"`. Without `display`, it defaults to "omitted"
        // on Opus 4.7/4.8 + Sonnet 5 and every captured thinking text is empty.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-opus-4-7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(8000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Auto),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["thinking"],
            serde_json::json!({ "type": "adaptive", "display": "summarized" })
        );
    }

    #[test]
    fn oauth_implicit_adaptive_thinking_requests_summarized_display() {
        // No explicit `thinking` level + OAuth + adaptive-capable model → the
        // implicit claude-cli adaptive shape, which also opts into summaries.
        let client = oauth_client();
        let request = LlmRequest {
            model: "claude-opus-4-7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(8000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["thinking"],
            serde_json::json!({ "type": "adaptive", "display": "summarized" })
        );
    }

    #[test]
    fn budget_tokens_thinking_does_not_set_display() {
        // The budget_tokens path targets pre-4.6 models, which predate
        // `display`; sending it there risks a 400 on every request.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: Some(32000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["thinking"],
            serde_json::json!({ "type": "enabled", "budget_tokens": 10000 })
        );
        assert!(
            body["thinking"].get("display").is_none(),
            "budget_tokens path must not carry `display`; got: {}",
            body["thinking"]
        );
    }

    /// Build a request whose history carries a single assistant message with
    /// `blocks`, so tests can assert the exact serialized `messages` payload.
    fn request_with_assistant_blocks(blocks: Vec<ContentBlock>) -> LlmRequest {
        LlmRequest {
            model: "claude-opus-4-7".into(),
            system: None,
            messages: vec![
                Message::user("hi"),
                Message {
                    role: Role::Assistant,
                    content: blocks,
                },
            ],
            max_tokens: Some(8000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        }
    }

    /// Anthropic's real `thinking` wire block, as Task 2 captures it into `raw`.
    fn anthropic_thinking_raw() -> serde_json::Value {
        serde_json::json!({
            "type": "thinking",
            "thinking": "step one, then step two",
            "signature": "sig-abc123",
        })
    }

    #[test]
    fn reasoning_block_is_restored_to_anthropic_wire_shape() {
        // Caching off: this pins the restoration shape exactly;
        // `restored_reasoning_raw_block_is_byte_identical` pins the cache-on shape.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink))
            .with_prompt_cache(false);
        let raw = anthropic_thinking_raw();
        let request = request_with_assistant_blocks(vec![
            ContentBlock::Reasoning {
                text: Some("step one, then step two".into()),
                provider: PROVIDER_TAG.into(),
                model: "claude-opus-4-7".into(),
                raw: raw.clone(),
            },
            ContentBlock::Text {
                text: "the answer".into(),
            },
        ]);
        let body = client.build_request_body(&request, false);
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        // Byte-exact echo of `raw` — no internal fields on the wire.
        assert_eq!(content[0], raw);
        assert_eq!(
            content[1],
            serde_json::json!({ "type": "text", "text": "the answer" })
        );
        let wire = serde_json::to_string(&body).unwrap();
        for leaked in ["provider", "\"reasoning\"", "\"model\":\"claude-opus-4-7\""] {
            assert!(
                !body["messages"].to_string().contains(leaked),
                "internal field {leaked} leaked onto the wire: {wire}"
            );
        }
    }

    #[test]
    fn foreign_provider_reasoning_block_is_dropped_from_request() {
        // A Gemini thoughtSignature is an alien wire format; it must never
        // reach Anthropic.
        // Caching off: this pins the restoration shape exactly;
        // `restored_reasoning_raw_block_is_byte_identical` pins the cache-on shape.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink))
            .with_prompt_cache(false);
        let request = request_with_assistant_blocks(vec![
            ContentBlock::Reasoning {
                text: Some("gemini thoughts".into()),
                provider: "google_gemini".into(),
                model: "gemini-3-pro".into(),
                raw: serde_json::json!({ "thoughtSignature": "opaque-token" }),
            },
            ContentBlock::Text {
                text: "the answer".into(),
            },
        ]);
        let body = client.build_request_body(&request, false);
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            content,
            &vec![serde_json::json!({ "type": "text", "text": "the answer" })]
        );
    }

    #[test]
    fn same_provider_different_model_reasoning_block_is_still_echoed() {
        // Regression guard: the echo gate is the provider tag ONLY. Thinking
        // blocks are not origin-locked — they replay across models fine, and
        // *stripping* them is what triggers ordering/signature 400s. Do not
        // reintroduce a model gate.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let raw = anthropic_thinking_raw();
        let request = request_with_assistant_blocks(vec![ContentBlock::Reasoning {
            text: Some("step one, then step two".into()),
            // Captured under a *different* Anthropic model than the request's.
            provider: PROVIDER_TAG.into(),
            model: "claude-sonnet-4-6".into(),
            raw: raw.clone(),
        }]);
        assert_ne!(request.model, "claude-sonnet-4-6");
        let body = client.build_request_body(&request, false);
        assert_eq!(body["messages"][1]["content"], serde_json::json!([raw]));
    }

    #[test]
    fn empty_text_reasoning_block_is_still_echoed() {
        // The `display: "omitted"` case: a thinking block with no readable
        // text still carries a signature and must be passed back as received.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let raw = serde_json::json!({
            "type": "thinking",
            "thinking": "",
            "signature": "sig-empty",
        });
        let request = request_with_assistant_blocks(vec![ContentBlock::Reasoning {
            text: None,
            provider: PROVIDER_TAG.into(),
            model: "claude-opus-4-7".into(),
            raw: raw.clone(),
        }]);
        let body = client.build_request_body(&request, false);
        assert_eq!(body["messages"][1]["content"], serde_json::json!([raw]));
    }

    #[test]
    fn unknown_block_is_dropped_from_request() {
        // `ContentBlock::Unknown` serializes as `{"type":"Unknown"}` (pinned by
        // a test in types.rs), which no provider accepts.
        // Caching off: this pins the restoration shape exactly;
        // `restored_reasoning_raw_block_is_byte_identical` pins the cache-on shape.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink))
            .with_prompt_cache(false);
        assert_eq!(
            serde_json::to_value(ContentBlock::Unknown).unwrap()["type"],
            "Unknown",
            "Unknown's wire tag changed — restore_reasoning_blocks must follow"
        );
        let request = request_with_assistant_blocks(vec![
            ContentBlock::Unknown,
            ContentBlock::Text {
                text: "the answer".into(),
            },
        ]);
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["messages"][1]["content"],
            serde_json::json!([{ "type": "text", "text": "the answer" }])
        );
    }

    #[test]
    fn malformed_raw_reasoning_block_is_dropped_from_request() {
        // `raw` has no `skip_serializing_if`, so `block.get("raw")` is always
        // `Some` — a `null`, scalar, or `{}` `raw` must still be rejected
        // before being echoed, or it goes on the wire as a malformed content
        // block and Anthropic 400s the request.
        // Caching off: this pins the restoration shape exactly;
        // `restored_reasoning_raw_block_is_byte_identical` pins the cache-on shape.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink))
            .with_prompt_cache(false);
        let request = request_with_assistant_blocks(vec![
            ContentBlock::Reasoning {
                text: Some("null raw".into()),
                provider: PROVIDER_TAG.into(),
                model: "claude-opus-4-7".into(),
                raw: serde_json::Value::Null,
            },
            ContentBlock::Reasoning {
                text: Some("scalar raw".into()),
                provider: PROVIDER_TAG.into(),
                model: "claude-opus-4-7".into(),
                raw: serde_json::json!("not-a-block"),
            },
            ContentBlock::Reasoning {
                text: Some("empty object raw".into()),
                provider: PROVIDER_TAG.into(),
                model: "claude-opus-4-7".into(),
                raw: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "the answer".into(),
            },
        ]);
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["messages"][1]["content"],
            serde_json::json!([{ "type": "text", "text": "the answer" }]),
            "malformed `raw` reasoning blocks must be dropped, not echoed onto the wire"
        );
    }

    #[test]
    fn test_build_request_body_thinking_medium() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("medium effort")],
            max_tokens: Some(8000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Medium),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 5000);
    }

    #[test]
    fn test_build_request_body_with_thinking_high() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("think hard")],
            max_tokens: Some(16000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
    }

    #[test]
    fn test_build_request_body_thinking_none() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("quick")],
            max_tokens: Some(100),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn test_build_request_body_thinking_minimal_skipped() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("classify")],
            max_tokens: Some(100),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Minimal),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert!(body.get("thinking").is_none()); // Minimal = skip
    }

    #[test]
    fn test_build_request_body_thinking_max() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-opus-4-6".into(),
            system: None,
            messages: vec![Message::user("deep analysis")],
            max_tokens: Some(32000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Max),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 30000); // 32000 - 2000
    }

    #[test]
    fn test_build_request_body_thinking_high_clamped_to_max_tokens() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("think")],
            max_tokens: Some(4096),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert!(
            budget <= 4096,
            "budget {budget} should be <= max_tokens 4096"
        );
        assert!(budget >= 1024, "budget {budget} should be >= minimum 1024");
    }

    /// With no cap pinned, the thinking budget is computed against the
    /// effective cap — the 8192 fallback that actually goes on the wire — not
    /// against a missing value.
    #[test]
    fn thinking_budget_uses_the_fallback_cap_when_max_tokens_is_unset() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let fallback = crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS;
        let mut request = make_request(None);
        request.max_tokens = None;

        request.thinking = Some(crate::model_tier::ThinkingLevel::Max);
        let body = client.build_request_body(&request, false);
        assert_eq!(body["max_tokens"], fallback);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], fallback - 2000);
        assert_eq!(body["thinking"]["budget_tokens"], 8192 - 2000);

        // High asks for 10_000, more than the effective cap: clamped to it.
        request.thinking = Some(crate::model_tier::ThinkingLevel::High);
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["budget_tokens"], fallback);
    }

    #[test]
    fn test_build_request_body_thinking_skipped_when_max_tokens_too_small() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("tiny")],
            max_tokens: Some(500),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Low),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("thinking").is_none(),
            "budget too small for 1024 minimum"
        );
    }

    #[test]
    fn test_stream_accumulator_text_only() {
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_123".into();
        acc.model = "claude-sonnet-4-6".into();
        acc.text = "Hello world".into();
        acc.stop_reason = Some(StopReason::EndTurn);
        acc.wire.input_tokens = 10;
        acc.wire.output_tokens = 5;

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "msg_123");
        assert_eq!(response.text(), Some("Hello world"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 10);
    }

    #[test]
    fn test_stream_accumulator_with_tool_use() {
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_456".into();
        acc.model = "claude-sonnet-4-6".into();
        acc.text = "Let me check.".into();
        acc.content_blocks.push(ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/test"}),
        });
        acc.stop_reason = Some(StopReason::ToolUse);

        let response = acc.into_response().unwrap();
        assert_eq!(response.content.len(), 2);
        assert_eq!(response.tool_calls().len(), 1);
    }

    #[test]
    fn test_stream_accumulator_empty_returns_none() {
        let acc = StreamAccumulator::new();
        assert!(acc.into_response().is_none());
    }

    #[test]
    fn test_new_creates_api_key_auth() {
        let client =
            AnthropicClient::new("sk-ant-api-test".into(), Arc::new(rupu_netflow::NullSink));
        assert!(!client.auth.is_oauth());
    }

    #[test]
    fn test_from_auth_oauth() {
        let auth = crate::auth::AuthMethod::OAuth {
            access_token: "sk-ant-oat01-test".into(),
            refresh_token: "refresh".into(),
            expires_ms: 9999999999999,
        };
        let client = AnthropicClient::from_auth(auth, Arc::new(rupu_netflow::NullSink));
        assert!(client.auth.is_oauth());
    }

    #[test]
    fn test_anthropic_response_deserialization() {
        let json = r#"{
            "id": "msg_test",
            "model": "claude-sonnet-4-6",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        }"#;
        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        let llm = response.into_llm_response();
        assert_eq!(llm.text(), Some("Hello!"));
        assert_eq!(llm.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn test_process_sse_events_full_text_stream() {
        // Architect requested: test process_sse_event through a full event sequence
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let mut events_received: Vec<String> = Vec::new();

        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":25}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_start".into(),
                data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_stop".into(),
                data: r#"{"type":"content_block_stop","index":0}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_stop".into(),
                data: r#"{"type":"message_stop"}"#.into(),
            },
        ];

        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    events_received.push(format!("{:?}", se));
                })
                .unwrap();
        }

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "msg_1");
        assert_eq!(response.model, "claude-sonnet-4-6");
        assert_eq!(response.text(), Some("Hello world"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 3);
        // Verify callback was called with text deltas
        assert!(events_received.iter().any(|e| e.contains("Hello")));
        assert!(events_received.iter().any(|e| e.contains(" world")));
    }

    #[test]
    fn test_process_sse_events_tool_use_stream() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_2".into();
        acc.model = "claude-sonnet-4-6".into();
        let mut callback_events = Vec::new();

        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "content_block_start".into(),
                data: r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"read_file"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/tmp/test\"}"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_stop".into(),
                data: r#"{"type":"content_block_stop","index":1}"#.into(),
            },
        ];

        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    callback_events.push(format!("{:?}", se));
                })
                .unwrap();
        }

        assert_eq!(acc.content_blocks.len(), 1);
        match &acc.content_blocks[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_abc");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test");
            }
            _ => panic!("expected ToolUse block"),
        }
        assert!(callback_events.iter().any(|e| e.contains("ToolUseStart")));
    }

    #[test]
    fn test_process_sse_event_malformed_json_returns_error() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let bad_event = crate::sse::SseEvent {
            event_type: "message_start".into(),
            data: "{ this is not json".into(),
        };
        let result = client.process_sse_event(&bad_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn test_process_sse_event_malformed_tool_input_json() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_1".into();
        acc.current_tool_id = Some("toolu_bad".into());
        acc.current_tool_name = Some("some_tool".into());
        acc.current_tool_input = "{ broken json".into();
        let stop_event = crate::sse::SseEvent {
            event_type: "content_block_stop".into(),
            data: r#"{"type":"content_block_stop","index":0}"#.into(),
        };
        let result = client.process_sse_event(&stop_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    // ── Anthropic-specific auth tests (moved from auth/mod.rs) ───────

    #[test]
    fn test_load_auth_json_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{
                "anthropic": {
                    "type": "oauth",
                    "access": "sk-ant-oat01-test",
                    "refresh": "sk-ant-ort01-test",
                    "expires": 9999999999999
                }
            }"#,
        )
        .unwrap();
        let result = load_auth_json(&path).unwrap();
        assert!(result.is_some());
        match result.unwrap() {
            AuthMethod::OAuth { access_token, .. } => {
                assert!(access_token.contains("oat01"));
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[test]
    fn test_load_auth_json_no_anthropic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai": {"type": "oauth", "access": "x", "refresh": "y", "expires": 0}}"#,
        )
        .unwrap();
        let result = load_auth_json(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_auth_json_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_auth_json(&path).is_err());
    }

    #[test]
    fn test_save_auth_json_preserves_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai": {"type": "oauth", "access": "x", "refresh": "y", "expires": 0}}"#,
        )
        .unwrap();

        let auth = AuthMethod::OAuth {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_ms: 12345,
        };
        save_auth_json(&path, &auth).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("openai").is_some());
        assert_eq!(content["anthropic"]["access"], "new-access");
    }

    #[test]
    fn test_load_auth_json_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#).unwrap();
        let method = load_auth_json(&path).unwrap().unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-test"));
    }

    #[test]
    fn test_save_auth_json_api_key_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        save_auth_json(&path, &AuthMethod::ApiKey("sk-roundtrip".into())).unwrap();
        let method = load_auth_json(&path).unwrap().unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-roundtrip"));
    }

    #[test]
    fn test_resolve_with_cortex_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"anthropic":{"type":"api_key","key":"sk-cortex"}}"#,
        )
        .unwrap();
        let method = resolve_anthropic_auth(None, Some(dir.path())).unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-cortex"));
    }

    #[cfg(unix)]
    #[test]
    fn test_save_auth_json_sets_0o600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        save_auth_json(&path, &AuthMethod::ApiKey("sk-test".into())).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_load_claude_code_keychain_returns_some_or_none() {
        // This test verifies the function doesn't panic regardless of keychain state.
        // On CI or machines without Claude Code it returns None; on dev machines with
        // Claude Code installed it returns Some with valid OAuth tokens.
        let result = load_claude_code_keychain();
        if let Some(AuthMethod::OAuth { access_token, .. }) = &result {
            assert!(
                access_token.starts_with("sk-ant-"),
                "keychain token should have sk-ant- prefix"
            );
        }
        // None is also acceptable — no Claude Code installed
    }

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
                "cache_read_input_tokens": 200
            }
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(body).unwrap();
        let resp: LlmResponse = parsed.into_llm_response();
        assert_eq!(resp.usage.cached_tokens, 200);
        // Anthropic's wire `input_tokens` (10) EXCLUDES cache reads; the
        // provider boundary normalizes it to the whole prompt: 10 + 200.
        assert_eq!(resp.usage.input_tokens, 210);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn decode_response_normalizes_cache_reads_and_writes_into_input() {
        let body = r#"{
            "id": "msg_x", "model": "claude-opus-5-5",
            "content": [{"type":"text","text":"hi"}], "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_read_input_tokens": 200, "cache_creation_input_tokens": 30}
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(body).unwrap();
        let resp = parsed.into_llm_response();
        assert_eq!(resp.usage.input_tokens, 240);
        assert_eq!(resp.usage.cached_tokens, 200);
        assert_eq!(resp.usage.cache_write_tokens, 30);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn stream_usage_normalizes_and_message_delta_cache_fields_win_when_present() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let mut snapshots: Vec<Usage> = Vec::new();

        // message_start carries input 10 / read 200 / write 30; message_delta
        // carries output 7 and (newer API) repeats the cache fields.
        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_c","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":200,"cache_creation_input_tokens":30}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7,"cache_read_input_tokens":200,"cache_creation_input_tokens":30}}"#.into(),
            },
        ];
        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    if let StreamEvent::UsageSnapshot(u) = se {
                        snapshots.push(u);
                    }
                })
                .unwrap();
        }

        // message_start snapshot is already normalized.
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].input_tokens, 240);
        assert_eq!(snapshots[0].cached_tokens, 200);
        assert_eq!(snapshots[0].cache_write_tokens, 30);
        // Final usage.
        let response = acc.into_response().unwrap();
        assert_eq!(response.usage.input_tokens, 240);
        assert_eq!(response.usage.cached_tokens, 200);
        assert_eq!(response.usage.cache_write_tokens, 30);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(snapshots[1].input_tokens, 240);
        assert_eq!(snapshots[1].output_tokens, 7);
    }

    #[test]
    fn stream_message_delta_cache_fields_overwrite_message_start_values() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_d","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4,"input_tokens":12,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}"#.into(),
            },
        ];
        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }
        let response = acc.into_response().unwrap();
        // 12 + 100 + 50
        assert_eq!(response.usage.input_tokens, 162);
        assert_eq!(response.usage.cached_tokens, 100);
        assert_eq!(response.usage.cache_write_tokens, 50);
        assert_eq!(response.usage.output_tokens, 4);
    }

    #[test]
    fn stream_message_delta_without_cache_fields_keeps_message_start_values() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_e","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":200,"cache_creation_input_tokens":30}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#.into(),
            },
        ];
        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }
        let response = acc.into_response().unwrap();
        assert_eq!(response.usage.input_tokens, 240);
        assert_eq!(response.usage.cached_tokens, 200);
        assert_eq!(response.usage.cache_write_tokens, 30);
        assert_eq!(response.usage.output_tokens, 9);
    }

    #[test]
    fn stream_message_start_output_tokens_is_not_authoritative() {
        // Real streams send `"output_tokens": 1` in message_start. The live
        // output estimate only updates while output == 0, so the first
        // snapshot must carry 0; the final value comes from message_delta.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let mut snapshots: Vec<Usage> = Vec::new();
        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_o","model":"claude-opus-5-5","usage":{"input_tokens":25,"output_tokens":1}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#.into(),
            },
        ];
        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    if let StreamEvent::UsageSnapshot(u) = se {
                        snapshots.push(u);
                    }
                })
                .unwrap();
        }
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].input_tokens, 25);
        assert_eq!(snapshots[0].output_tokens, 0);
        assert_eq!(snapshots[1].output_tokens, 42);
        let response = acc.into_response().unwrap();
        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 42);
    }

    #[test]
    fn stream_message_start_null_cache_field_keeps_other_counters() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = StreamAccumulator::new();
        let event = crate::sse::SseEvent {
            event_type: "message_start".into(),
            data: r#"{"type":"message_start","message":{"id":"msg_n","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":200,"cache_creation_input_tokens":null}}}"#.into(),
        };
        client
            .process_sse_event(&event, &mut acc, &mut |_| {})
            .unwrap();
        let response = acc.into_response().unwrap();
        // 10 + 200 + (null -> 0): input and read survive the null write field.
        assert_eq!(response.usage.input_tokens, 210);
        assert_eq!(response.usage.cached_tokens, 200);
        assert_eq!(response.usage.cache_write_tokens, 0);
    }

    #[test]
    fn decode_response_tolerates_null_cache_fields() {
        let body = r#"{
            "id": "msg_x", "model": "claude-opus-5-5",
            "content": [{"type":"text","text":"hi"}], "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_read_input_tokens": null, "cache_creation_input_tokens": 30}
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(body).unwrap();
        let resp = parsed.into_llm_response();
        assert_eq!(resp.usage.input_tokens, 40);
        assert_eq!(resp.usage.cached_tokens, 0);
        assert_eq!(resp.usage.cache_write_tokens, 30);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn build_body_does_not_emit_output_config_format_for_schemaless_json() {
        // Anthropic's structured-outputs API requires `output_config.format`
        // to be a `{type: "json_schema", schema: ...}` object; our
        // `OutputFormat` enum carries no schema, so we must not emit the
        // field at all (a bare string 400s every request). An agent with
        // only `output_format: Json` and no task_budget should therefore
        // produce a request body with no `output_config` key whatsoever.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_format: Some(crate::types::OutputFormat::Json),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("output_config").is_none(),
            "expected no output_config, got: {:?}",
            body.get("output_config")
        );
    }

    #[test]
    fn build_body_emits_output_config_format_json_schema_when_schema_present() {
        // The real fix: an agent that declares `outputSchema` gets a
        // correctly-shaped `output_config.format = {type: "json_schema",
        // schema: <the schema>}` — the only shape Anthropic accepts.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "findings": { "type": "array" } },
            "required": ["findings"]
        });
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_format: Some(crate::types::OutputFormat::Json),
            output_schema: Some(schema.clone()),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["output_config"]["format"],
            serde_json::json!({ "type": "json_schema", "schema": schema })
        );
    }

    #[test]
    fn build_body_strips_anthropic_unsupported_numeric_bounds_from_output_schema() {
        // Anthropic structured outputs reject numeric bounds like
        // `minimum` on an `integer`/`number` node — the API 400s with
        // "output_config.format.schema: For 'integer' type, property
        // 'minimum' is not supported". A schema that is valid JSON Schema
        // (and accepted by OpenAI's structured outputs, which DOES support
        // `minimum`) must still go through, so the provider strips the
        // unsupported keywords from numeric nodes before sending.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["findings"],
            "properties": {
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "line": { "type": "integer", "minimum": 1 },
                            "confidence": {
                                "type": "number",
                                "minimum": 0,
                                "maximum": 100,
                                "exclusiveMinimum": 0
                            }
                        }
                    }
                }
            }
        });
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_schema: Some(schema),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        let sent = &body["output_config"]["format"]["schema"];
        let props = &sent["properties"]["findings"]["items"]["properties"];
        // The unsupported numeric bounds are gone …
        assert!(
            props["line"].get("minimum").is_none(),
            "line.minimum survived"
        );
        assert!(props["confidence"].get("minimum").is_none());
        assert!(props["confidence"].get("maximum").is_none());
        assert!(props["confidence"].get("exclusiveMinimum").is_none());
        // … but the rest of the schema is intact.
        assert_eq!(props["line"]["type"], "integer");
        assert_eq!(props["confidence"]["type"], "number");
        assert_eq!(sent["required"], serde_json::json!(["findings"]));
        assert_eq!(sent["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn sanitize_schema_preserves_property_literally_named_minimum() {
        // A property KEY named "minimum" must survive; only the `minimum`
        // *keyword* on a numeric node is stripped.
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": { "minimum": { "type": "integer", "minimum": 3 } }
        });
        let removed = sanitize_output_schema_for_anthropic(&mut schema);
        assert_eq!(removed, 1);
        assert!(
            schema["properties"].get("minimum").is_some(),
            "property named `minimum` was dropped"
        );
        assert!(schema["properties"]["minimum"].get("minimum").is_none());
        assert_eq!(schema["properties"]["minimum"]["type"], "integer");
    }

    #[test]
    fn sanitize_schema_leaves_bounds_on_non_numeric_nodes() {
        // Only integer/number nodes are touched — the exact surface
        // Anthropic rejects. A (nonsensical) bound on a non-numeric node
        // is left alone rather than guessed at.
        let mut schema = serde_json::json!({ "type": "string", "minimum": 1 });
        let removed = sanitize_output_schema_for_anthropic(&mut schema);
        assert_eq!(removed, 0);
        assert_eq!(schema["minimum"], 1);
    }

    #[test]
    fn sanitize_schema_recurses_into_combinators_and_defs() {
        let mut schema = serde_json::json!({
            "$defs": { "n": { "type": "integer", "minimum": 0 } },
            "anyOf": [
                { "type": "number", "maximum": 5, "multipleOf": 2 },
                { "type": "string" }
            ]
        });
        let removed = sanitize_output_schema_for_anthropic(&mut schema);
        assert_eq!(removed, 3);
        assert!(schema["$defs"]["n"].get("minimum").is_none());
        assert!(schema["anyOf"][0].get("maximum").is_none());
        assert!(schema["anyOf"][0].get("multipleOf").is_none());
    }

    #[test]
    fn build_body_omits_output_config_format_when_schema_none_even_with_output_format_json() {
        // Floor (#469): `outputFormat: json` alone is prompt-driven only.
        // Without a schema there is nothing valid to send, so `format`
        // must never appear even though `output_format` is `Json`.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_format: Some(crate::types::OutputFormat::Json),
            output_schema: None,
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("output_config").is_none(),
            "expected no output_config at all, got: {:?}",
            body.get("output_config")
        );
    }

    #[test]
    fn build_body_output_config_carries_both_schema_format_and_task_budget() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let schema = serde_json::json!({"type": "object"});
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_schema: Some(schema.clone()),
            anthropic_task_budget: Some(1500),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(
            body["output_config"]["format"],
            serde_json::json!({ "type": "json_schema", "schema": schema })
        );
        assert_eq!(body["output_config"]["task_budget"], 1500);
    }

    #[test]
    fn build_body_emits_output_config_task_budget() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            anthropic_task_budget: Some(2048),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["output_config"]["task_budget"], 2048);
    }

    #[test]
    fn build_body_emits_context_management_tool_clearing() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            anthropic_context_management: Some(crate::types::ContextManagement::ToolClearing),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["context_management"]["type"], "tool_clearing");
    }

    #[test]
    fn build_body_emits_speed_fast() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            anthropic_speed: Some(crate::types::Speed::Fast),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["speed"], "fast");
    }

    #[test]
    fn build_body_omits_optional_fields_when_none() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("output_config").is_none(),
            "no output_format/task_budget set → no output_config"
        );
        assert!(body.get("context_management").is_none());
        assert!(body.get("speed").is_none());
    }

    #[test]
    fn build_body_output_config_carries_task_budget_only_even_with_output_format_set() {
        // `output_format` is prompt-driven only (see comment above
        // `output_config` construction) — it must never surface in the
        // wire body, even when `anthropic_task_budget` is also set and
        // legitimately populates `output_config`.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            messages: vec![Message::user("hi")],
            max_tokens: Some(100),
            output_format: Some(crate::types::OutputFormat::Json),
            anthropic_task_budget: Some(1500),
            ..Default::default()
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["output_config"]["task_budget"], 1500);
        assert!(body["output_config"].get("format").is_none());
    }

    // ── Prompt caching (spec 2026-09-29 §9.3) ─────────────────────────

    fn body_for(client: &AnthropicClient, req: &LlmRequest) -> serde_json::Value {
        client.build_request_body(req, true)
    }

    /// Every `cache_control` key anywhere in the body.
    fn cache_markers(v: &serde_json::Value) -> usize {
        let mut n = 0;
        fn walk(v: &serde_json::Value, n: &mut usize) {
            match v {
                serde_json::Value::Object(m) => {
                    if m.contains_key("cache_control") {
                        *n += 1;
                    }
                    m.values().for_each(|x| walk(x, n));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, n)),
                _ => {}
            }
        }
        walk(v, &mut n);
        n
    }

    /// The body with every `cache_control` key removed — equal to the
    /// caching-off body iff `apply_cache_breakpoints` touched nothing else.
    fn strip_cache_control(mut v: serde_json::Value) -> serde_json::Value {
        fn walk(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(m) => {
                    m.remove("cache_control");
                    m.values_mut().for_each(walk);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
                _ => {}
            }
        }
        walk(&mut v);
        v
    }

    fn ephemeral() -> serde_json::Value {
        serde_json::json!({ "type": "ephemeral" })
    }

    fn cache_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("the {name} tool"),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }

    fn uncached_client() -> AnthropicClient {
        AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink)).with_prompt_cache(false)
    }

    #[test]
    fn caching_marks_last_system_block_and_last_message_block() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = LlmRequest {
            model: "claude-opus-5-5".into(),
            system: Some("You are a reviewer.".into()),
            messages: vec![
                Message::user("first"),
                Message::assistant("an answer"),
                Message::user("second"),
            ],
            tools: vec![cache_tool("read_file")],
            max_tokens: Some(1024),
            ..Default::default()
        };
        let b = body_for(&client, &req);
        let sys = b["system"].as_array().unwrap();
        // Exactly `{"type":"ephemeral"}` — the 5-minute TTL, never a `ttl`.
        assert_eq!(sys.last().unwrap()["cache_control"], ephemeral());
        // Tools render before system, so the system marker already caches
        // them; the tool itself carries no second marker.
        assert!(b["tools"][0].get("cache_control").is_none());
        let msgs = b["messages"].as_array().unwrap();
        let last_msg = msgs.last().unwrap();
        let blocks = last_msg["content"]
            .as_array()
            .expect("Message::user serializes content as a block array");
        assert_eq!(blocks.last().unwrap()["cache_control"], ephemeral());
        // One rolling conversation breakpoint: no earlier message is marked.
        for m in &msgs[..msgs.len() - 1] {
            assert_eq!(cache_markers(m), 0, "history message marked: {m}");
        }
        // No top-level (automatic) caching field.
        assert!(b.get("cache_control").is_none());
        assert_eq!(cache_markers(&b), 2);
        // Only `cache_control` keys were added — nothing else changed.
        assert_eq!(strip_cache_control(b), body_for(&uncached_client(), &req));
    }

    #[test]
    fn caching_disabled_emits_no_markers() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink))
            .with_prompt_cache(false);
        let req = LlmRequest {
            model: "m".into(),
            system: Some("s".into()),
            messages: vec![Message::user("hi")],
            tools: vec![cache_tool("t")],
            ..Default::default()
        };
        assert_eq!(cache_markers(&body_for(&client, &req)), 0);
    }

    #[test]
    fn per_request_opt_out_emits_no_markers_even_with_caching_enabled() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        assert!(client.prompt_cache_enabled);
        let mut req = LlmRequest {
            model: "m".into(),
            system: Some("summarize the prior turns".into()),
            messages: vec![
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
            ],
            tools: vec![cache_tool("t")],
            ..Default::default()
        };
        // Sanity: the same request is marked without the opt-out.
        assert_eq!(cache_markers(&body_for(&client, &req)), 2);
        req.disable_prompt_cache = true;
        let b = body_for(&client, &req);
        assert_eq!(cache_markers(&b), 0);
        assert_eq!(b, body_for(&uncached_client(), &req));
        // OAuth (system-prefix blocks prepended) honours it too.
        assert_eq!(cache_markers(&body_for(&oauth_client(), &req)), 0);
    }

    #[test]
    fn caching_is_on_by_default_for_every_constructor() {
        let sink = || -> Arc<dyn rupu_netflow::FlowSink> { Arc::new(rupu_netflow::NullSink) };
        let api_key = || AuthMethod::ApiKey("k".into());
        let dir = tempfile::tempdir().unwrap();
        let store = crate::credential_store::CredentialStore::load(
            dir.path().join("auth.json"),
            dir.path().join("auth_status.json"),
        )
        .expect("empty credential store");
        let clients = [
            ("from_auth", AnthropicClient::from_auth(api_key(), sink())),
            ("new", AnthropicClient::new("k".into(), sink())),
            (
                "with_url",
                AnthropicClient::with_url("k".into(), "http://x.test".into(), sink()),
            ),
            (
                "from_auth_with_url",
                AnthropicClient::from_auth_with_url(api_key(), "http://x.test".into(), sink()),
            ),
            (
                "from_auth_with_path",
                AnthropicClient::from_auth_with_path(api_key(), dir.path().join("a.json"), sink()),
            ),
            (
                "from_auth_with_store",
                AnthropicClient::from_auth_with_store(api_key(), Arc::new(store), sink()),
            ),
        ];
        for (name, client) in clients {
            assert!(
                client.prompt_cache_enabled,
                "{name}: caching must default ON"
            );
            assert!(!client.with_prompt_cache(false).prompt_cache_enabled);
        }
    }

    #[test]
    fn no_system_marks_last_tool_instead() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = LlmRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            tools: vec![cache_tool("a"), cache_tool("b")],
            ..Default::default()
        };
        let b = body_for(&client, &req);
        assert!(b.get("system").is_none());
        assert!(b["tools"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][1]["cache_control"], ephemeral());
        assert_eq!(b["messages"][0]["content"][0]["cache_control"], ephemeral());
        assert_eq!(cache_markers(&b), 2);
        assert_eq!(strip_cache_control(b), body_for(&uncached_client(), &req));
    }

    #[test]
    fn no_system_and_no_tools_marks_only_the_final_message() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![Message::user("hi")],
            ..Default::default()
        };
        let b = body_for(&client, &req);
        assert_eq!(b["messages"][0]["content"][0]["cache_control"], ephemeral());
        assert_eq!(cache_markers(&b), 1);
    }

    #[test]
    fn empty_system_text_block_is_skipped_for_the_prefix_marker() {
        // An empty text block cannot carry `cache_control`; with no other
        // system block the prefix marker falls back to the last tool.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = LlmRequest {
            model: "m".into(),
            system: Some(String::new()),
            messages: vec![Message::user("hi")],
            tools: vec![cache_tool("a")],
            ..Default::default()
        };
        let b = body_for(&client, &req);
        assert!(b["system"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][0]["cache_control"], ephemeral());
        assert_eq!(cache_markers(&b), 2);
    }

    #[test]
    fn never_marks_thinking_or_empty_text_blocks() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let thinking = || ContentBlock::Reasoning {
            text: Some("step one, then step two".into()),
            provider: PROVIDER_TAG.into(),
            model: "claude-opus-4-7".into(),
            raw: anthropic_thinking_raw(),
        };
        let redacted = || ContentBlock::Reasoning {
            text: None,
            provider: PROVIDER_TAG.into(),
            model: "claude-opus-4-7".into(),
            raw: serde_json::json!({ "type": "redacted_thinking", "data": "enc_abc" }),
        };
        let text = |t: &str| ContentBlock::Text { text: t.into() };

        // [text "x", thinking] → the marker lands on "x", the last
        // NON-thinking block; the thinking block is untouched.
        let b = body_for(
            &client,
            &request_with_assistant_blocks(vec![text("x"), thinking()]),
        );
        let content = b["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["cache_control"], ephemeral());
        assert_eq!(content[1], anthropic_thinking_raw());
        assert_eq!(cache_markers(&b), 1);

        // [tool_use, empty text, whitespace text] → the tool_use is marked.
        let b = body_for(
            &client,
            &request_with_assistant_blocks(vec![
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "a" }),
                },
                text(""),
                text(" \n"),
            ]),
        );
        let content = b["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["cache_control"], ephemeral());
        assert!(content[1].get("cache_control").is_none());
        assert!(content[2].get("cache_control").is_none());
        assert_eq!(cache_markers(&b), 1);

        // Nothing cacheable in the final message → the final message stays
        // unmarked and the rolling marker walks back to the nearest earlier
        // message with a cacheable block (here the opening user "hi").
        // Without that, the request would carry no message-level breakpoint
        // and read no history from the cache at all.
        for blocks in [
            vec![text("")],
            vec![thinking()],
            vec![redacted(), thinking()],
        ] {
            let mut req = request_with_assistant_blocks(blocks);
            req.system = Some("s".into());
            let b = body_for(&client, &req);
            assert_eq!(b["system"][0]["cache_control"], ephemeral());
            assert_eq!(
                cache_markers(&b["messages"][1]),
                0,
                "final message must stay unmarked: {}",
                b["messages"]
            );
            assert_eq!(b["messages"][0]["content"][0]["cache_control"], ephemeral());
            assert_eq!(cache_markers(&b), 2);
            // Thinking bytes untouched: only `cache_control` was added.
            assert_eq!(strip_cache_control(b), body_for(&uncached_client(), &req));
        }
    }

    /// A tool-use turn: user prompt, assistant `[text, tool_use]`, then the
    /// tool result as the final message.
    fn tool_turn_request(result: &str) -> LlmRequest {
        LlmRequest {
            model: "m".into(),
            system: Some("s".into()),
            messages: vec![
                Message::user("rebuild the widget crate"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text {
                            text: "Rebuilding now.".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "toolu_q7".into(),
                            name: "bash".into(),
                            input: serde_json::json!({ "command": "make -s widget" }),
                        },
                    ],
                },
                Message::tool_result("toolu_q7", result, false),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn empty_tool_result_tail_walks_back_to_the_preceding_tool_use() {
        // A silent bash command yields `tool_result` with `content: ""` as the
        // final message's only block. It must not carry the marker; the
        // preceding assistant's `tool_use` does instead.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        for silent in ["", "  \n\t"] {
            let req = tool_turn_request(silent);
            let b = body_for(&client, &req);
            let msgs = b["messages"].as_array().unwrap();
            assert_eq!(cache_markers(&msgs[2]), 0, "empty tool_result marked");
            assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
            assert_eq!(msgs[1]["content"][1]["cache_control"], ephemeral());
            assert!(msgs[1]["content"][0].get("cache_control").is_none());
            assert_eq!(cache_markers(&msgs[0]), 0);
            assert_eq!(b["system"][0]["cache_control"], ephemeral());
            assert_eq!(cache_markers(&b), 2);
            assert_eq!(strip_cache_control(b), body_for(&uncached_client(), &req));
        }
    }

    #[test]
    fn real_tool_result_tail_keeps_the_marker() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = tool_turn_request("compiled 3 units\nok");
        let b = body_for(&client, &req);
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["cache_control"], ephemeral());
        assert_eq!(
            cache_markers(&msgs[1]),
            0,
            "no walk-back when the tail is cacheable"
        );
        assert_eq!(cache_markers(&b), 2);
        assert_eq!(strip_cache_control(b), body_for(&uncached_client(), &req));
    }

    #[test]
    fn walk_back_skips_thinking_before_the_tool_use() {
        // [thinking, tool_use] + [tool_result ""] → the tool_use is marked
        // and the restored thinking block stays byte-identical.
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let raw = anthropic_thinking_raw();
        let req = LlmRequest {
            model: "claude-opus-4-7".into(),
            messages: vec![
                Message::user("list the repos"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Reasoning {
                            text: Some("step one, then step two".into()),
                            provider: PROVIDER_TAG.into(),
                            model: "claude-opus-4-7".into(),
                            raw: raw.clone(),
                        },
                        ContentBlock::ToolUse {
                            id: "toolu_r2".into(),
                            name: "scm.repos.list".into(),
                            input: serde_json::json!({}),
                        },
                    ],
                },
                Message::tool_result("toolu_r2", "", false),
            ],
            max_tokens: Some(8000),
            ..Default::default()
        };
        let on = body_for(&client, &req);
        let off = body_for(&uncached_client(), &req);
        assert_eq!(
            serde_json::to_string(&on["messages"][1]["content"][0]).unwrap(),
            serde_json::to_string(&off["messages"][1]["content"][0]).unwrap(),
        );
        assert_eq!(on["messages"][1]["content"][0], raw);
        assert_eq!(
            on["messages"][1]["content"][1]["cache_control"],
            ephemeral()
        );
        assert_eq!(cache_markers(&on["messages"][2]), 0);
        assert_eq!(cache_markers(&on), 1);
        assert_eq!(strip_cache_control(on), off);
    }

    #[test]
    fn empty_tool_result_is_not_cacheable_in_any_content_shape() {
        let tr = |content: Option<serde_json::Value>| {
            let mut b = serde_json::json!({ "type": "tool_result", "tool_use_id": "toolu_1" });
            if let Some(c) = content {
                b["content"] = c;
            }
            b
        };
        for empty in [
            None,
            Some(serde_json::json!("")),
            Some(serde_json::json!(" \n")),
            Some(serde_json::json!([])),
            Some(serde_json::json!([{ "type": "text", "text": "" }])),
            Some(serde_json::json!([
                { "type": "text", "text": "  " },
                { "type": "text", "text": "" },
            ])),
        ] {
            let block = tr(empty.clone());
            assert!(!is_cacheable_block(&block), "cacheable: {block}");
        }
        for real in [
            serde_json::json!("exit 0"),
            serde_json::json!([{ "type": "text", "text": "" }, { "type": "text", "text": "ok" }]),
            serde_json::json!([{
                "type": "image",
                "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0K" },
            }]),
        ] {
            let block = tr(Some(real));
            assert!(is_cacheable_block(&block), "not cacheable: {block}");
        }

        // End to end on a raw body: `[user: tool_result []]` walks back to the
        // preceding assistant's tool_use.
        for empty in [
            serde_json::json!([]),
            serde_json::json!([{ "type": "text", "text": "" }]),
        ] {
            let mut body = serde_json::json!({
                "messages": [
                    { "role": "user", "content": [{ "type": "text", "text": "go" }] },
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_1", "name": "bash", "input": {} },
                    ] },
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": empty },
                    ] },
                ],
            });
            apply_cache_breakpoints(&mut body);
            assert_eq!(cache_markers(&body["messages"][2]), 0);
            assert_eq!(
                body["messages"][1]["content"][0]["cache_control"],
                ephemeral()
            );
            assert_eq!(cache_markers(&body), 1);
        }
    }

    #[test]
    fn walk_back_is_bounded_to_three_messages_before_the_last() {
        let msg = |role: &str, content: serde_json::Value| serde_json::json!({ "role": role, "content": content });
        let anchor = || {
            msg(
                "user",
                serde_json::json!([{ "type": "text", "text": "anchor" }]),
            )
        };
        let thinking = || {
            msg(
                "assistant",
                serde_json::json!([{ "type": "thinking", "thinking": "hm", "signature": "sig" }]),
            )
        };
        let redacted = || {
            msg(
                "assistant",
                serde_json::json!([{ "type": "redacted_thinking", "data": "enc" }]),
            )
        };
        let empty_result = |content: serde_json::Value| {
            msg(
                "user",
                serde_json::json!([{ "type": "tool_result", "tool_use_id": "t", "content": content }]),
            )
        };

        // The anchor is exactly 3 messages before the last → still reached.
        let mut body = serde_json::json!({ "messages": [
            anchor(),
            empty_result(serde_json::json!("")),
            redacted(),
            empty_result(serde_json::json!([])),
        ] });
        apply_cache_breakpoints(&mut body);
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            ephemeral()
        );
        assert_eq!(cache_markers(&body), 1);

        // One more uncacheable message pushes it 4 back → out of range, so
        // no message-level marker at all (rather than one deep in history).
        let mut body = serde_json::json!({ "messages": [
            anchor(),
            thinking(),
            empty_result(serde_json::json!("")),
            redacted(),
            empty_result(serde_json::json!([])),
        ] });
        let before = body.clone();
        apply_cache_breakpoints(&mut body);
        assert_eq!(cache_markers(&body), 0);
        assert_eq!(body, before);
    }

    #[test]
    fn oauth_billing_blocks_stay_first_and_unmarked_except_last_system_block() {
        let client = oauth_client();
        let b = body_for(&client, &make_request(Some("agent persona")));
        let sys = b["system"].as_array().expect("system is array");
        assert_eq!(sys.len(), 3);
        // Byte-identical billing + self-description blocks, in order, unmarked.
        assert_eq!(
            sys[0],
            serde_json::json!({ "type": "text", "text": ANTHROPIC_BILLING_HEADER_BLOCK })
        );
        assert_eq!(
            sys[1],
            serde_json::json!({ "type": "text", "text": ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION })
        );
        assert_eq!(
            sys[2],
            serde_json::json!({
                "type": "text",
                "text": "agent persona",
                "cache_control": { "type": "ephemeral" },
            })
        );
        assert_eq!(cache_markers(&b), 2);

        // No agent system prompt: the self-description is now the last
        // system block and carries the marker; the billing block never does.
        let b = body_for(&client, &make_request(None));
        let sys = b["system"].as_array().expect("system is array");
        assert_eq!(sys.len(), 2);
        assert_eq!(
            sys[0],
            serde_json::json!({ "type": "text", "text": ANTHROPIC_BILLING_HEADER_BLOCK })
        );
        assert_eq!(sys[1]["text"], ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION);
        assert_eq!(sys[1]["cache_control"], ephemeral());
        assert_eq!(cache_markers(&b), 2);
    }

    #[test]
    fn restored_reasoning_raw_block_is_byte_identical() {
        let cached = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let raw = anthropic_thinking_raw();
        let reasoning = ContentBlock::Reasoning {
            text: Some("step one, then step two".into()),
            provider: PROVIDER_TAG.into(),
            model: "claude-opus-4-7".into(),
            raw: raw.clone(),
        };

        // Restored block in history; the final message is the tool result.
        let req = LlmRequest {
            model: "claude-opus-4-7".into(),
            messages: vec![
                Message::user("hi"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        reasoning.clone(),
                        ContentBlock::ToolUse {
                            id: "toolu_1".into(),
                            name: "scm.repos.list".into(),
                            input: serde_json::json!({}),
                        },
                    ],
                },
                Message::tool_result("toolu_1", "ok", false),
            ],
            max_tokens: Some(8000),
            ..Default::default()
        };
        let on = body_for(&cached, &req);
        let off = body_for(&uncached_client(), &req);
        assert_eq!(
            serde_json::to_string(&on["messages"][1]["content"][0]).unwrap(),
            serde_json::to_string(&off["messages"][1]["content"][0]).unwrap(),
        );
        assert_eq!(on["messages"][1]["content"][0], raw);
        assert_eq!(
            on["messages"][2]["content"][0]["cache_control"],
            ephemeral()
        );
        assert_eq!(strip_cache_control(on), off);

        // Restored block as the LAST block of the final message: skipped,
        // not marked, and still byte-identical.
        let req = request_with_assistant_blocks(vec![
            ContentBlock::Text {
                text: "the answer".into(),
            },
            reasoning,
        ]);
        let on = body_for(&cached, &req);
        let off = body_for(&uncached_client(), &req);
        assert_eq!(
            serde_json::to_string(&on["messages"][1]["content"][1]).unwrap(),
            serde_json::to_string(&off["messages"][1]["content"][1]).unwrap(),
        );
        assert_eq!(on["messages"][1]["content"][1], raw);
        assert_eq!(
            on["messages"][1]["content"][0]["cache_control"],
            ephemeral()
        );
        assert_eq!(strip_cache_control(on), off);
    }

    #[test]
    fn apply_cache_breakpoints_converts_string_content_only_when_marking() {
        let mut body = serde_json::json!({
            "messages": [
                { "role": "user", "content": "earlier" },
                { "role": "user", "content": "hello" },
            ],
        });
        apply_cache_breakpoints(&mut body);
        // History string content is left as-is.
        assert_eq!(body["messages"][0]["content"], "earlier");
        assert_eq!(
            body["messages"][1]["content"],
            serde_json::json!([{
                "type": "text",
                "text": "hello",
                "cache_control": { "type": "ephemeral" },
            }])
        );

        // Empty string content cannot carry a marker → left untouched.
        let mut body = serde_json::json!({
            "messages": [{ "role": "user", "content": "" }],
        });
        apply_cache_breakpoints(&mut body);
        assert_eq!(body["messages"][0]["content"], "");
        assert_eq!(cache_markers(&body), 0);

        // Empty final string content → walk back: the earlier string content
        // is converted (because it is now the block being marked); the empty
        // final message is left as-is.
        let mut body = serde_json::json!({
            "messages": [
                { "role": "user", "content": "earlier" },
                { "role": "user", "content": " " },
            ],
        });
        apply_cache_breakpoints(&mut body);
        assert_eq!(
            body["messages"][0]["content"],
            serde_json::json!([{
                "type": "text",
                "text": "earlier",
                "cache_control": { "type": "ephemeral" },
            }])
        );
        assert_eq!(body["messages"][1]["content"], " ");
        assert_eq!(cache_markers(&body), 1);
    }

    #[tokio::test]
    async fn list_models_api_key_path_parses_v1_models() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .header("x-api-key", "sk-ant-test")
                .header("anthropic-version", ANTHROPIC_VERSION);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "data": [
                        { "id": "claude-opus-4-1-20250805", "type": "model" },
                        { "id": "claude-sonnet-4-6", "type": "model" }
                    ],
                    "first_id": "claude-opus-4-1-20250805",
                    "last_id": "claude-sonnet-4-6",
                    "has_more": false
                }));
        });
        // Override the api_url to point at the mock. Real production
        // URL is `/v1/messages?beta=true`; the discovery impl strips
        // both the path and the query string.
        let client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::list_models(&client).await;
        assert!(models.iter().any(|m| m.id == "claude-opus-4-1-20250805"));
        assert!(models.iter().any(|m| m.id == "claude-sonnet-4-6"));
        assert!(models.iter().all(|m| m.provider == ProviderId::Anthropic));
    }

    // ── [providers.anthropic] tuning (ISSUES.md I-9 / I-10) ──────────

    #[test]
    fn tuning_sets_the_retry_budget_the_client_will_spend() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        // Historical default, unchanged for non-factory constructors.
        assert_eq!(client.max_rate_limit_retries(), 1);
        let tuned = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink)).with_tuning(
            &crate::tuning::ProviderTuning {
                max_retries: 4,
                ..crate::tuning::ProviderTuning::for_provider("anthropic")
            },
        );
        assert_eq!(tuned.max_rate_limit_retries(), 4);
    }

    /// The budget is observable AT THE CONSUMER: with `max_retries = 0` a
    /// rate-limited request is issued exactly once, where the historical
    /// hardcoded budget of 1 would have issued it twice.
    #[tokio::test]
    async fn max_retries_zero_issues_exactly_one_request() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"rate_limit_error"}}"#);
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        )
        .with_tuning(&crate::tuning::ProviderTuning {
            max_retries: 0,
            ..crate::tuning::ProviderTuning::for_provider("anthropic")
        });
        let _ = client.send(&make_request(None)).await;
        m.assert_hits(1);
    }

    /// …and with `max_retries = 1` the same 429 is retried once (two hits).
    /// This is the assertion that the config value, not a constant, drives
    /// the loop. It sleeps out one 2s backoff.
    #[tokio::test]
    async fn max_retries_one_issues_two_requests() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"rate_limit_error"}}"#);
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        )
        .with_tuning(&crate::tuning::ProviderTuning {
            max_retries: 1,
            ..crate::tuning::ProviderTuning::for_provider("anthropic")
        });
        let _ = client.send(&make_request(None)).await;
        m.assert_hits(2);
    }

    // ── I-84: stream()'s idle-restart / 429-retry loops share one budget ──

    /// The fixed ceiling is a SUM of the two individual limits, not their
    /// PRODUCT — and critically, a `retries_used` count past the old
    /// idle-only ceiling (`MAX_STREAM_IDLE_RETRIES`) must still be retried
    /// when the configured `max_rate_limit_retries` extends the shared
    /// budget. The old nested-loop code had no such combined budget at
    /// all: the outer loop's own bound (`idle_attempt ==
    /// MAX_STREAM_IDLE_RETRIES`) never referenced `max_rate_limit_retries`,
    /// so the worst case was their product, e.g. (10 + 1) * (3 + 1) = 44
    /// requests for one `stream()` call — vs. 10 + 3 = 13 now.
    #[test]
    fn should_retry_idle_stall_draws_from_a_shared_sum_budget_not_the_old_product() {
        let max_rate_limit_retries = 3;
        let total_retry_budget = MAX_STREAM_IDLE_RETRIES + max_rate_limit_retries;
        assert_eq!(
            total_retry_budget, 13,
            "budget must be additive, not the product (44)"
        );

        // Past the old idle-only ceiling (10) but within the shared budget
        // (13): must still retry, because max_rate_limit_retries extends it.
        assert!(should_retry_idle_stall(false, 11, total_retry_budget));
        assert!(should_retry_idle_stall(false, 12, total_retry_budget));
        // Budget exactly exhausted: give up.
        assert!(!should_retry_idle_stall(false, 13, total_retry_budget));
    }

    #[test]
    fn should_retry_idle_stall_never_retries_after_emitted_content() {
        // A mid-response stall can't be re-sent without duplicating
        // already-streamed output — regardless of remaining budget.
        assert!(!should_retry_idle_stall(true, 0, 100));
    }

    /// A stream that's persistently 429'd (never a stall) fails inside the
    /// inner 429 loop's own `max_rate_limit_retries` cap and never touches
    /// the outer idle-restart logic at all — so its request count must stay
    /// exactly `max_rate_limit_retries + 1`, not get multiplied by
    /// `MAX_STREAM_IDLE_RETRIES + 1` the way the old nested loops could for
    /// an interleaved stall+429 stream. Regression coverage for the
    /// unchanged half of the I-84 fix (sleeps out one real 2s backoff).
    #[tokio::test]
    async fn stream_persistent_429_fails_without_touching_idle_budget() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"rate_limit_error"}}"#);
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        )
        .with_tuning(&crate::tuning::ProviderTuning {
            max_retries: 1,
            ..crate::tuning::ProviderTuning::for_provider("anthropic")
        });
        let result = client.stream(&make_request(None), |_ev| {}).await;
        assert!(result.is_err());
        m.assert_hits(2);
    }

    // ── I-75: the native 429 backoff releases its concurrency permit ────

    /// `send()`'s own 429 backoff sleep must not hold the concurrency
    /// permit — otherwise every other concurrent Anthropic call is starved
    /// for the whole backoff ladder, the exact failure mode the
    /// `RetryingProvider(ThrottledProvider(..))` ordering avoids for every
    /// other provider. Sleeps out one real ~2s backoff, sampling permit
    /// availability mid-sleep. Uses a private, test-only semaphore (set
    /// directly on the private field, not via `with_tuning`) rather than
    /// the name-keyed process-wide registry `with_tuning` normally wires
    /// up — that registry is cached per provider name across the whole
    /// test binary, so a `max_concurrency: 1` requested here could
    /// silently be ignored if some other "anthropic"-named test already
    /// initialized it at a different size.
    #[tokio::test]
    async fn native_429_backoff_releases_the_concurrency_permit() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"rate_limit_error"}}"#);
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        )
        .with_tuning(&crate::tuning::ProviderTuning {
            max_retries: 1,
            ..crate::tuning::ProviderTuning::for_provider("anthropic")
        });
        let sem = Arc::new(Semaphore::new(1));
        client.semaphore = Some(sem.clone());

        let request = make_request(None);
        let handle = tokio::spawn(async move {
            let _ = client.send(&request).await;
        });

        // The first attempt (429) should have completed and be mid-way
        // through its ~2s backoff sleep by now.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(
            sem.available_permits(),
            1,
            "the permit was still held during the native 429 backoff sleep"
        );

        handle.await.unwrap();
        m.assert_hits(2);
        assert_eq!(sem.available_permits(), 1, "permit leaked after the call");
    }

    /// Same property for `stream()`'s inner 429-retry loop.
    #[tokio::test]
    async fn native_stream_429_backoff_releases_the_concurrency_permit() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"rate_limit_error"}}"#);
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        )
        .with_tuning(&crate::tuning::ProviderTuning {
            max_retries: 1,
            ..crate::tuning::ProviderTuning::for_provider("anthropic")
        });
        let sem = Arc::new(Semaphore::new(1));
        client.semaphore = Some(sem.clone());

        let request = make_request(None);
        let handle = tokio::spawn(async move {
            let _ = client.stream(&request, |_ev| {}).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(
            sem.available_permits(),
            1,
            "the permit was still held during stream()'s native 429 backoff sleep"
        );

        handle.await.unwrap();
        m.assert_hits(2);
        assert_eq!(sem.available_permits(), 1, "permit leaked after the call");
    }

    #[tokio::test]
    async fn list_models_returns_empty_on_non_2xx() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"authentication_error"}}"#);
        });
        let client = AnthropicClient::with_url(
            "bad-key".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::list_models(&client).await;
        assert!(models.is_empty());
    }

    /// The same 401 `list_models` swallows into an empty vec must surface as a
    /// real error from `probe` — that difference is the whole reason `probe`
    /// exists rather than being derived from `list_models`.
    #[tokio::test]
    async fn probe_surfaces_401_instead_of_swallowing_it() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"authentication_error"}}"#);
        });
        let client = AnthropicClient::with_url(
            "bad-key".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );

        let err = <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect_err("a 401 must never be reported as healthy");

        match err {
            ProviderError::Api {
                status: 401,
                message,
            } => assert!(
                message.contains("authentication_error"),
                "the response body must land in the message, got {message:?}"
            ),
            other => panic!("expected Api{{status:401}}, got {other:?}"),
        }
    }

    /// `probe` only needs the status, so it must not download the whole
    /// catalog: it asks for a single model.
    #[tokio::test]
    async fn probe_requests_a_single_model() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("limit", "1")
                .matches(no_after_id);
            then.status(200)
                .json_body(serde_json::json!({ "data": [], "has_more": false }));
        });
        let client = AnthropicClient::with_url(
            "good-key".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );

        <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect("probe must succeed against a limit=1 listing");
        m.assert_hits(1);
    }

    /// A 2xx means the credential works — even when the account lists no
    /// models at all, which is exactly the case `list_models` cannot
    /// distinguish from a failure.
    #[tokio::test]
    async fn probe_succeeds_on_2xx_with_no_models() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":[]}"#);
        });
        let client = AnthropicClient::with_url(
            "good-key".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );

        <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect("a 2xx must probe clean even with zero models");
    }

    /// A transport failure (nothing listening) is Unreachable-shaped, NOT
    /// auth-shaped — the adapter's `classify` depends on that split.
    #[tokio::test]
    async fn probe_maps_transport_failure_to_http() {
        // Port 1 is reserved and never listening.
        let client = AnthropicClient::with_url(
            "k".into(),
            "http://127.0.0.1:1/v1/messages".to_string(),
            Arc::new(rupu_netflow::NullSink),
        );

        let err = <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect_err("nothing is listening");

        assert!(
            matches!(err, ProviderError::Http(_)),
            "a transport failure must be Http, not an auth error; got {err:?}"
        );
    }

    // ── fetch_models (Task 3: limits, paging, token refresh) ──────────

    fn no_after_id(req: &httpmock::prelude::HttpMockRequest) -> bool {
        !req.query_params
            .as_ref()
            .is_some_and(|q| q.iter().any(|(k, _)| k == "after_id"))
    }

    #[tokio::test]
    async fn fetch_models_reads_limits_and_follows_pages() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let page1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("limit", "1000")
                .matches(no_after_id);
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "claude-alpha-9", "type": "model", "max_input_tokens": 1000000, "max_tokens": 128000 }],
                "has_more": true, "first_id": "claude-alpha-9", "last_id": "claude-alpha-9"
            }));
        });
        let page2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("after_id", "claude-alpha-9");
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "claude-beta-2-20260101", "type": "model", "max_input_tokens": null, "max_tokens": null }],
                "has_more": false, "first_id": "claude-beta-2-20260101", "last_id": "claude-beta-2-20260101"
            }));
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        page1.assert();
        page2.assert();
        assert_eq!(models.len(), 2);
        assert_eq!(
            (models[0].context_window, models[0].max_output_tokens),
            (1_000_000, 128_000)
        );
        assert_eq!(
            (models[1].context_window, models[1].max_output_tokens),
            (0, 0)
        );
    }

    #[tokio::test]
    async fn fetch_models_oauth_sends_bearer_and_beta() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .header("authorization", "Bearer tok-live")
                .header("anthropic-beta", "oauth-2025-04-20");
            then.status(200)
                .json_body(serde_json::json!({ "data": [], "has_more": false }));
        });
        let mut client = AnthropicClient::from_auth_with_url(
            AuthMethod::OAuth {
                access_token: "tok-live".into(),
                refresh_token: "r".into(),
                expires_ms: u64::MAX,
            },
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        m.assert();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn fetch_models_surfaces_non_2xx_as_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401).body("{\"error\":\"nope\"}");
        });
        let mut client = AnthropicClient::with_url(
            "k".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let err = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Api { status: 401, .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_models_stops_on_repeated_cursor_edge_case_a() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let page1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("limit", "1000")
                .matches(no_after_id);
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "model-1", "type": "model", "max_input_tokens": 100, "max_tokens": 10 }],
                "has_more": true, "first_id": "model-1", "last_id": "model-1"
            }));
        });
        let page2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("after_id", "model-1");
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "model-2", "type": "model", "max_input_tokens": 200, "max_tokens": 20 }],
                "has_more": true, "first_id": "model-2", "last_id": "model-1"
            }));
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        page1.assert();
        page2.assert_hits(1); // Cursor repeated: after_id=model-1 page fetched exactly once
        assert_eq!(models.len(), 2); // Two models: one from page1, one from page2
        assert_eq!(models[0].id, "model-1");
        assert_eq!(models[1].id, "model-2");
    }

    /// An A -> B -> A cursor cycle (not just an immediate repeat) must stop
    /// before re-requesting a page — otherwise the same models are collected
    /// again until the 50-page cap.
    #[tokio::test]
    async fn fetch_models_stops_on_an_a_b_a_cursor_cycle() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let page1 = server.mock(|when, then| {
            when.method(GET).path("/v1/models").matches(no_after_id);
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "model-1", "max_input_tokens": 100, "max_tokens": 10 }],
                "has_more": true, "last_id": "cursor-a"
            }));
        });
        let page2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("after_id", "cursor-a");
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "model-2", "max_input_tokens": 200, "max_tokens": 20 }],
                "has_more": true, "last_id": "cursor-b"
            }));
        });
        let page3 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/models")
                .query_param("after_id", "cursor-b");
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "model-3", "max_input_tokens": 300, "max_tokens": 30 }],
                "has_more": true, "last_id": "cursor-a"
            }));
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        page1.assert_hits(1);
        page2.assert_hits(1);
        page3.assert_hits(1);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["model-1", "model-2", "model-3"], "no duplicates");
    }

    /// A 200 whose body is not JSON is a decode failure, not a transport
    /// failure: it must surface as `Json`, not `Http`.
    #[tokio::test]
    async fn fetch_models_non_json_body_is_a_json_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200).body("not json");
        });
        let mut client = AnthropicClient::with_url(
            "k".into(),
            format!("{}/v1/messages", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let err = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Json(_)), "{err:?}");
    }

    // ── Reasoning capture (Task 2) ───────────────────────────────────

    fn sse(event_type: &str, data: &str) -> crate::sse::SseEvent {
        crate::sse::SseEvent {
            event_type: event_type.into(),
            data: data.into(),
        }
    }

    fn reasoning_acc() -> StreamAccumulator {
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_think".into();
        acc.model = "claude-opus-4-8".into();
        acc
    }

    #[test]
    fn stream_captures_thinking_block_with_signature() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_xyz"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }

        assert_eq!(acc.content_blocks.len(), 1);
        match &acc.content_blocks[0] {
            ContentBlock::Reasoning {
                text,
                provider,
                model,
                raw,
            } => {
                assert_eq!(text.as_deref(), Some("let me think"));
                assert_eq!(provider, "anthropic");
                assert_eq!(model, "claude-opus-4-8");
                assert_eq!(
                    *raw,
                    serde_json::json!({
                        "type": "thinking",
                        "thinking": "let me think",
                        "signature": "sig_xyz"
                    })
                );
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn stream_captures_thinking_block_with_empty_text() {
        // display:"omitted" -> a thinking block arrives with no thinking_delta,
        // only a signature_delta. It MUST still produce a Reasoning block so it
        // can be echoed back unchanged.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_only"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }

        assert_eq!(
            acc.content_blocks.len(),
            1,
            "empty-text block must not be skipped"
        );
        match &acc.content_blocks[0] {
            ContentBlock::Reasoning { text, raw, .. } => {
                assert_eq!(*text, None);
                assert_eq!(
                    *raw,
                    serde_json::json!({
                        "type": "thinking",
                        "thinking": "",
                        "signature": "sig_only"
                    })
                );
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn stream_captures_redacted_thinking_block() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"enc_abc123"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }

        assert_eq!(acc.content_blocks.len(), 1);
        match &acc.content_blocks[0] {
            ContentBlock::Reasoning {
                text,
                provider,
                raw,
                ..
            } => {
                assert_eq!(*text, None);
                assert_eq!(provider, "anthropic");
                assert_eq!(
                    *raw,
                    serde_json::json!({"type": "redacted_thinking", "data": "enc_abc123"})
                );
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn stream_emits_reasoning_delta_events() {
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let mut events_received = Vec::new();
        let events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    events_received.push(format!("{se:?}"));
                })
                .unwrap();
        }

        assert!(
            events_received
                .iter()
                .any(|e| e.contains("ReasoningDelta") && e.contains("let me think")),
            "expected a ReasoningDelta event, got {events_received:?}"
        );
        assert!(
            !events_received.iter().any(|e| e.contains("TextDelta")),
            "thinking content must not be emitted as TextDelta"
        );
    }

    #[test]
    fn thinking_delta_without_open_block_is_not_fabricated() {
        // No `content_block_start` of type "thinking" precedes this delta
        // (e.g. it followed a start type this parser doesn't recognize and
        // dropped via the `_ => {}` arm). The accumulator must not fabricate
        // a reasoning buffer for it — doing so would let `content_block_stop`
        // push a `{"type":"thinking", ...}` raw block that was never really
        // opened, which Anthropic rejects when echoed back.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let mut events_received = Vec::new();
        let events = vec![
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"orphan"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    events_received.push(format!("{se:?}"));
                })
                .unwrap();
        }

        assert!(
            acc.content_blocks.is_empty(),
            "no Reasoning block should be fabricated for an unopened thinking buffer: {:?}",
            acc.content_blocks
        );
        assert!(
            !events_received.iter().any(|e| e.contains("ReasoningDelta")),
            "no ReasoningDelta event should fire for an unopened block: {events_received:?}"
        );
    }

    #[test]
    fn reasoning_delta_counts_as_emitted_content_for_idle_guard() {
        // Calls the actual production predicate used by `stream()`'s
        // idle-stall guard (see `stream_event_counts_as_emitted_content`,
        // wired in at the `for event in parser.feed(...)` loop) — not a
        // hand-copied match arm. If `ReasoningDelta` were ever dropped from
        // the production guard this test would fail: a stream that only
        // produced thinking deltas before an idle gap has emitted content
        // and must not be treated as a prefill stall and silently retried
        // (which would duplicate the response).
        assert!(stream_event_counts_as_emitted_content(
            &StreamEvent::ReasoningDelta("thinking".into())
        ));
        // Sanity check the predicate isn't vacuously true for everything.
        assert!(stream_event_counts_as_emitted_content(
            &StreamEvent::TextDelta("hi".into())
        ));
        assert!(stream_event_counts_as_emitted_content(
            &StreamEvent::ToolUseStart {
                id: "toolu_1".into(),
                name: "read_file".into(),
            }
        ));
        assert!(stream_event_counts_as_emitted_content(
            &StreamEvent::InputJsonDelta("{}".into())
        ));
        assert!(
            !stream_event_counts_as_emitted_content(&StreamEvent::UsageSnapshot(Usage::default())),
            "a bare usage snapshot carries no generated content"
        );
    }

    #[test]
    fn into_response_places_reasoning_before_text() {
        // Anthropic requires thinking blocks first in an assistant turn.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();
        let events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"planning"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#,
            ),
        ];
        for event in &events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }

        let resp = acc.into_response().unwrap();
        assert_eq!(resp.content.len(), 2);
        assert!(
            matches!(resp.content[0], ContentBlock::Reasoning { .. }),
            "reasoning must come first, got {:?}",
            resp.content
        );
        assert!(matches!(resp.content[1], ContentBlock::Text { .. }));
        assert_eq!(resp.text(), Some("answer"));
    }

    #[test]
    fn streaming_thinking_text_tool_use_interleaving_is_isolated() {
        // Full streaming sequence: thinking, then text, then tool_use.
        // `content_block_stop` finalizes reasoning and tool_use via two
        // independent `if let` checks against accumulator state — this
        // guards against a regression where the thinking block's stop event
        // also resolves (or otherwise disturbs) a tool_use that hasn't even
        // started yet, and against tool input getting corrupted by the
        // unrelated reasoning block that preceded it.
        let client = AnthropicClient::new("test-key".into(), Arc::new(rupu_netflow::NullSink));
        let mut acc = reasoning_acc();

        let thinking_events = vec![
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"reasoning steps"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ];
        for event in &thinking_events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }
        // The thinking block's own stop must not fabricate or resolve
        // anything beyond the reasoning block itself — no tool_use exists
        // yet at this point in the stream.
        assert_eq!(
            acc.content_blocks.len(),
            1,
            "thinking's content_block_stop must only emit the reasoning block: {:?}",
            acc.content_blocks
        );
        assert!(matches!(
            acc.content_blocks[0],
            ContentBlock::Reasoning { .. }
        ));

        let rest_events = vec![
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"here's the answer"}}"#,
            ),
            sse(
                "content_block_start",
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#,
            ),
            sse(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/tmp/x\"}"}}"#,
            ),
            sse(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":2}"#,
            ),
        ];
        for event in &rest_events {
            client
                .process_sse_event(event, &mut acc, &mut |_| {})
                .unwrap();
        }

        let resp = acc.into_response().unwrap();
        assert_eq!(
            resp.content.len(),
            3,
            "expected exactly [Reasoning, Text, ToolUse], got {:?}",
            resp.content
        );
        assert!(
            matches!(resp.content[0], ContentBlock::Reasoning { .. }),
            "reasoning must come first, got {:?}",
            resp.content
        );
        assert!(matches!(resp.content[1], ContentBlock::Text { .. }));
        match &resp.content[2] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "read_file");
                assert_eq!(input, &serde_json::json!({"path": "/tmp/x"}));
            }
            other => panic!("expected exactly one ToolUse block, got {other:?}"),
        }
        assert_eq!(
            resp.content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                .count(),
            1,
            "tool_use must be resolved exactly once, not duplicated by the thinking stop"
        );
    }

    #[test]
    fn non_streaming_response_with_thinking_block_deserializes() {
        // Regression guard: this previously failed with "unknown variant".
        let json = r#"{
            "id": "msg_ns",
            "model": "claude-opus-4-8",
            "content": [
                {"type": "thinking", "thinking": "deliberating", "signature": "sig_ns"},
                {"type": "text", "text": "the answer"},
                {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "/tmp/x"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        }"#;
        let response: AnthropicResponse =
            serde_json::from_str(json).expect("thinking block must deserialize");
        let llm = response.into_llm_response();

        assert_eq!(llm.content.len(), 3);
        match &llm.content[0] {
            ContentBlock::Reasoning {
                text,
                provider,
                model,
                raw,
            } => {
                assert_eq!(text.as_deref(), Some("deliberating"));
                assert_eq!(provider, "anthropic");
                assert_eq!(model, "claude-opus-4-8");
                assert_eq!(raw["signature"], "sig_ns");
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
        assert_eq!(llm.text(), Some("the answer"));
        assert_eq!(llm.tool_calls().len(), 1);
        assert_eq!(llm.reasoning_text().as_deref(), Some("deliberating"));
    }

    #[test]
    fn non_streaming_redacted_thinking_block_deserializes() {
        let json = r#"{
            "id": "msg_red",
            "model": "claude-opus-4-8",
            "content": [{"type": "redacted_thinking", "data": "enc_zzz"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let llm = serde_json::from_str::<AnthropicResponse>(json)
            .unwrap()
            .into_llm_response();
        match &llm.content[0] {
            ContentBlock::Reasoning { text, raw, .. } => {
                assert_eq!(*text, None);
                assert_eq!(
                    *raw,
                    serde_json::json!({"type": "redacted_thinking", "data": "enc_zzz"})
                );
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn non_streaming_thinking_block_with_empty_text_is_captured() {
        // display:"omitted" — text is None but raw keeps the empty string so
        // the block still echoes back unchanged.
        let json = r#"{
            "id": "msg_empty",
            "model": "claude-opus-4-8",
            "content": [{"type": "thinking", "thinking": "", "signature": "sig_e"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let llm = serde_json::from_str::<AnthropicResponse>(json)
            .unwrap()
            .into_llm_response();
        assert_eq!(llm.content.len(), 1, "empty-text block must not be dropped");
        match &llm.content[0] {
            ContentBlock::Reasoning { text, raw, .. } => {
                assert_eq!(*text, None);
                assert_eq!(raw["thinking"], "");
                assert_eq!(raw["signature"], "sig_e");
            }
            other => panic!("expected Reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn non_streaming_unknown_block_type_is_dropped_not_fatal() {
        let json = r#"{
            "id": "msg_unk",
            "model": "claude-opus-4-8",
            "content": [
                {"type": "some_future_block", "payload": 1},
                {"type": "text", "text": "still here"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }"#;
        let llm = serde_json::from_str::<AnthropicResponse>(json)
            .expect("unknown block must not fail the turn")
            .into_llm_response();
        assert_eq!(llm.content.len(), 1);
        assert_eq!(llm.text(), Some("still here"));
    }
}
