# rupu — backlog

Items deferred from completed slices. Each entry should name **why deferred**, **prereqs**, and **what unblocks it**.

## Sub-agent dispatch — agent-as-tool

Has its own design spec: [`docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`](docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md). That doc is the source of truth for the new tool family (`dispatch_agent` + `dispatch_agents_parallel`), the per-parent allowlist (`dispatchable_agents:` frontmatter), recursion / depth limits, the parent↔child run linkage in `RunRecord`, and how the line-stream printer renders dispatched children at indent+1.

**Status:** Design draft.
**Plans:** to be written after the design lands. Three suggested phases (single dispatch → parallel fan-out → live per-child streaming), each landable as an independent PR.

## Depth-aware tree-flow rendering for fan-out / parallel / panel ✅ shipped

Shipped in PR #112 (`feat(printer): depth-aware tree rendering for for_each / parallel / panel`). Every fan-out shape now renders as a parent frame holding indent+1 child frames with the `╭─` / `╰─` callout glyphs and per-kind headlines (`iter[N] · <input>` / `<sub_id>` / panelist name).

Live per-item streaming during a fan-out's execution is intentionally deferred — children currently render synchronously when the parent record lands. Same caveat as Plan 3 of the sub-agent design spec; both can share the same "live tail per child" infrastructure when it's built.

## Workflow triggers — manual / cron / event-driven

Has its own design spec: [`docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`](docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md). That doc is the source of truth for the architecture (three tiers: cron-tick polled, webhook-serve, rupu.cloud relay), the schema, the event vocabulary, and the Slice E hand-off contract.

**Status:**
- ✅ Trigger schema (`trigger.on: manual|cron|event`) parses + validates.
- ✅ `rupu cron tick` fires `trigger.on: cron` workflows from system cron / launchd. Idempotent.
- ✅ `rupu webhook serve` (long-running HTTP receiver, HMAC-validated) fires `trigger.on: event` workflows.
- ✅ **Plan 1 — polled events on cron tick** ([plan](docs/superpowers/plans/2026-05-07-rupu-workflow-triggers-plan-1-polled-events.md)). `rupu cron tick` now also polls `[triggers].poll_sources` for SCM events, fires matching workflows with `{{event.*}}` populated, idempotent via deterministic `evt-<wf>-<vendor>-<delivery>` run-ids. New: `rupu cron events` for inspection; `--skip-events` / `--only-events` flags for splitting tick frequencies. User docs at [`docs/triggers.md`](docs/triggers.md).
- ⏳ Plan 2 (future) — glob matching on `trigger.event:` (`github.issue.*`), extended event vocab (issue-tracker queue events).
- ⏳ Plan 3 (future, Slice E) — rupu.cloud webhook relay; cloud-as-connector or cloud-as-stream consumption.

## Workflow Tier 2 — fan-out, panel steps, approval gates

These add up to "platform-grade" orchestration but each is independently scoped.

**Fan-out (`parallel:` / `for_each:`).** A single step dispatches the same agent across multiple inputs in parallel and aggregates results:

```yaml
- id: review_each
  agent: code-reviewer
  for_each: "{{inputs.changed_files}}"
  prompt: "Review {{item}} ..."
  # per-item results bound as steps.review_each.results[*] (list)

- id: triage
  parallel:
    - { agent: security-reviewer,   prompt: "..." }
    - { agent: perf-reviewer,       prompt: "..." }
    - { agent: maintainability-reviewer, prompt: "..." }
  # parallel results bound as steps.triage.results.<sub_id>
```

Concurrency cap (`max_parallel: N`) + result aggregation part of the schema. The single highest-leverage Tier 2 item — most non-trivial workflows want parallel agent dispatch.

Both shapes shipped:
- `for_each:` (data fan-out) — `Step.for_each` + `max_parallel:`, results bound as `steps.<id>.results[*]` (list of strings).
- `parallel:` (agent fan-out) — `Step.parallel` (list of `SubStep`s, each with `id`/`agent`/`prompt`), results bound as `steps.<id>.sub_results.<sub_id>.{output,success}` (named map) and `steps.<id>.results[*]` (positional list).
Per-unit failures honor `continue_on_error:`. Approval gates shipped via the persistent run-state foundation (see below).

**Panel steps with gated review loop** (`kind: panel` — rupu's name; Okesu calls these "meeting steps"). A list of agents reviews/discusses something, emits structured findings, and the workflow loops with a fixer agent until the panel is satisfied:

```yaml
- id: code_review_panel
  kind: panel
  panelists:
    - security-reviewer
    - perf-reviewer
    - maintainability-reviewer
  subject: "{{inputs.diff}}"
  gate:
    until_no_findings_at_severity_or_above: HIGH    # loop while HIGH or CRITICAL findings exist
    fix_with: developer                              # agent that addresses each round
    max_iterations: 5                                # safety cap
  # Output: steps.code_review_panel.findings (consolidated, deduped),
  # .iterations (count), .resolved (bool)
```

Loop semantics: each iteration fans out the panelists in parallel, collects findings, classifies by severity. If any HIGH/CRITICAL, dispatch `fix_with` with the panel's findings as input; rerun panel on the fixed result; repeat until no HIGH/CRITICAL or `max_iterations` exhausted. Workflow proceeds when the gate clears (or fails with `unresolved_findings` if it doesn't). Distinctive feature; fits rupu's agent-builder pitch.

✅ **Shipped** (commits in feat/orchestrator-panel-steps): the `panel:` schema + parallel panelist dispatch + structured-findings parsing + gate loop with `fix_with`. Note rupu uses `panel:` as a sub-block on a Step (not `kind: panel` — keeps the Step shape consistent with the other fan-out kinds).

**Approval gates (`approval: required`).** ✅ shipped (PR 1: persistent run state in `<global>/runs/<id>/`; PR 2: schema + runner pause/resume + `rupu workflow approve` / `reject`; PR 3: `timeout_seconds:` enforcement). Webhook + cron callers report paused runs (run-id in JSON response / log). Timeout enforcement is **lazy** — checked on next operator interaction (`rupu workflow runs` / `approve` / `reject`); past-deadline paused runs flip to `Failed` with an "approval expired" error. A native ticker daemon would enforce eagerly but isn't needed for v1.

**Why deferred:** all three are bigger than a single PR's scope and Tier 1 (when / continue_on_error / inputs / defaults) closes the most painful gaps first.

## Anthropic feature flags ✅ shipped

Four agent-frontmatter knobs flow through to the Anthropic / OpenAI request body:
- `outputFormat: text | json` — cross-provider. Anthropic emits `output_config.format`; OpenAI emits `text.format.type: json_object`. Other providers ignore.
- `anthropicTaskBudget: <u32>` — Anthropic-only soft cap on output tokens (model self-paces). Distinct from `maxTurns` (hard ceiling). Emitted as `output_config.task_budget`.
- `anthropicContextManagement: tool_clearing` — Anthropic-only auto context-pruning. Server transparently drops earlier `tool_use`/`tool_result` blocks when conversation would overflow. Emitted as `context_management: { type: "tool_clearing" }`.
- `anthropicSpeed: fast` — Anthropic-only fast-mode toggle. Account-gated. Emitted as top-level `speed: "fast"`.

Pipeline: AgentSpec → AgentRunOpts → LlmRequest → Anthropic/OpenAI client body builder. Optional fields are only emitted on the wire when explicitly set, keeping the payload identical to pre-feature for agents that don't opt in.

## Re-MITM and refresh the Anthropic OAuth wire-shape pins (when 429s return)

The Anthropic OAuth path in `crates/rupu-providers/src/anthropic.rs` carries several **byte-equal pins** of upstream Claude Code values that gate Cloudflare/WAF/billing recognition. Captured 2026-05-04 from `claude --print "say hi"` MITM:

- `RUPU_USER_AGENT` — `claude-cli/2.1.126 (external, sdk-cli)`
- `ANTHROPIC_BETA_CSV` — full 10-element CSV
- `STAINLESS_HEADERS` — SDK telemetry (Package-Version `0.81.0`, Runtime-Version `v24.3.0`, etc.)
- `ANTHROPIC_BILLING_HEADER_BLOCK` — `system[0]` text with `cc_version=2.1.126.125; cc_entrypoint=sdk-cli; cch=0ab17;`
- `ANTHROPIC_AGENT_SDK_SELF_DESCRIPTION` — fixed `system[1]` block

**Symptom that this needs refreshing:** `/v1/messages` calls return `429 rate_limit_error` with **empty body** (`{"error":{"message":"Error"}}`) and **no `anthropic-ratelimit-*` headers**, while `claude --print` from the same shell still works. That signature is the WAF reject pool — request is no longer recognized as Claude-Code-shaped and we've drifted from upstream.

**Recovery:**
1. `claude --version` to confirm the upstream rev.
2. Re-MITM a working `claude --print "say hi"` through `mitmdump --listen-port 8080` with `NODE_EXTRA_CA_CERTS=~/.mitmproxy/mitmproxy-ca-cert.pem HTTPS_PROXY=http://127.0.0.1:8080`.
3. Diff captured headers + `system[0]` text against the constants in `anthropic.rs` and update.

**Special case — `cch=0ab17`:** the short trailing token may be a checksum of other request fields (UA, beta CSV, account UUID, …). We send it statically. If a fresh re-MITM shows the value rotating per request, **reverse-engineer the hash function** producing it. Likely candidates: a truncated HMAC over the access-token-derived account UUID + cc_version + a build-time secret. `0ab17` is hex-ish, 5 chars — likely 20-bit prefix of a longer digest. Start by capturing two requests in quick succession and seeing if `cch` differs; if same, it's not request-bound.

**Why deferred (not fixed now):** today's pin works against the live API. Reverse-engineering `cch` is only worthwhile if the static value stops working — premature otherwise. Long-term the right cure would be rupu-specific OAuth client registrations, but the vendors don't currently approve third-party rupu-branded clients for paid-subscriber inference, so impersonation is the durable answer until that policy changes.

## Code-signing & keychain trust

Status:
- ✅ **Layer 1 (local dev)**: `Makefile` + `scripts/sign-dev.sh` sign every `cargo build` with the Developer ID Application cert. Click "Always Allow" once on the first keychain prompt; the signing identity is stable across rebuilds, so subsequent builds inherit the trust.
- ✅ **Layer 2 (released binaries)**: `scripts/notarize-release.sh` submits a signed binary to `xcrun notarytool` with the `rupu` keychain profile. `docs/RELEASING.md` updated with the one-time `notarytool store-credentials` setup and the per-release notarization step. Notarized binaries are trusted by Gatekeeper out of the box for end users.
- ✅ **Layer 3 (programmatic keychain ACL)** — shipped for the modern CLI credential path. macOS `KeychainResolver` now bypasses `keyring` and creates generic-password items directly with `SecItemAdd` + `kSecAttrAccess` pre-populated to trust the running `rupu` binary. It also reads/deletes through the same macOS-specific helper path, and the access instance explicitly covers the operations `rupu` uses (read/decrypt, modify, delete, change ACL). Ignored real-keychain round trips now pass for both the low-level helper crate and the resolver path.

  ⚠️ **Remaining legacy scope:** the older `KeyringBackend` wrapper and probe path still use the `keyring` crate on macOS. `try_add_self_to_acl` remains a documented no-op there because the old retrofit path would reintroduce extra prompts. This is low-priority because the CLI runtime uses `KeychainResolver`, not `KeyringBackend`.

  Estimated cost: 1-2 days. ~200 lines of macOS-specific write code in `rupu-keychain-acl` plus a feature-flag in rupu-auth's keyring backend to use it instead of the keyring crate's `set_password` on macOS targets.

## `rupu usage` aggregation subcommand (deferred from Slice B-1)

Slice B-1 captures `Event::Usage` per response in JSONL transcripts. A `rupu usage` subcommand would aggregate across transcripts (per-day, per-agent, per-provider) for cost visibility. Deferred to Slice D so the same aggregator can also power the SaaS dashboard rather than building two implementations.

## Autoflow Plan 2 — portable runtime and serve mode

Has its own design spec: [`docs/superpowers/specs/2026-05-09-rupu-autoflow-plan-2-portable-runtime-design.md`](docs/superpowers/specs/2026-05-09-rupu-autoflow-plan-2-portable-runtime-design.md) and implementation plan: [`docs/superpowers/plans/2026-05-09-rupu-autoflow-plan-2-portable-runtime-and-serve.md`](docs/superpowers/plans/2026-05-09-rupu-autoflow-plan-2-portable-runtime-and-serve.md).

**Status:** Shipped.

**What landed:**
- `rupu autoflow serve` built on the same reconciliation engine as `tick`
- normalized wake queue with replay protection / delivery dedupe
- versioned `RunEnvelope`, `WakeRecord`, `ArtifactManifest`, and `WorkerRecord`
- execution backend boundary with `local_worktree` as the first backend
- richer operator tooling (`wakes`, `explain`, `doctor`, `repair`, `requeue`)
- deployment-mode and runtime-contract docs for future Slice E consumers

**Post-Plan-2 backlog (future Slice E consumers):**
- cloud control plane / remote API implementation
- cloud worker backend
- registered local worker dispatch from `rupu.cloud`
- object-storage-backed artifact transport
- remote secret / credential brokering for cloud execution
- fleet-level worker scheduling, routing, and admission control
- documented or partially automated tunnel / edge deployment recipes for `webhook serve` (for example Tailscale Funnel, Cloudflare Tunnel, ngrok)

## Gemini API-key support via AI Studio ✅ shipped

Added `GeminiVariant::AiStudio` to the existing `GoogleGeminiClient` (rather than a separate `GoogleGeminiAiStudioClient`) so the OAuth and api-key paths share the body builder + response parser. Variant-specific branches handle the different URL pattern (`v1beta/models/{model}:generateContent`), `x-goog-api-key` header, request-body shape (no Cloud Code Assist `project` / `requestId` wrapping), and skipped token refresh. Wired through `provider_factory::build_gemini` so `rupu auth login --provider gemini --mode api-key --key AIzaSy...` followed by `rupu run --provider gemini ...` works end-to-end.

## Copilot `default_model` inconsistency ✅ resolved

All sites in `crates/rupu-providers/src/github_copilot.rs` agree on `"claude-sonnet-4-6"` as of the polish-pass review (2026-05-07). No action needed.

## ProviderError truncate-helper test gap (deferred from Plan 1 Task 11)

`crates/rupu-providers/src/classify.rs::truncate` uses `s.is_char_boundary` walk-back, but the regression test in `crates/rupu-providers/tests/classify.rs::classify_handles_multibyte_utf8_body_without_panic` uses a 4-byte-aligned input (`"🦀".repeat(N)` with `N % 4 == 0`), so the walk-back loop is never actually exercised. A truly adversarial test should use a 3-byte char (e.g. `"€".repeat(N)`) where `(max % 3 != 0)` to exercise the walk-back path. Plan 2/3 polish.
