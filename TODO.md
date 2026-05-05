# rupu — backlog

Items deferred from completed slices. Each entry should name **why deferred**, **prereqs**, and **what unblocks it**.

## Workflow triggers — manual / cron / event-driven (multi-PR initiative)

rupu workflows currently only run via `rupu workflow run <name>` (CLI manual trigger). Okesu supports rich trigger declarations that fire on schedule or on external events; that capability is what turns "agentic CLI" into "agentic platform". Designing this needs three independent PRs:

**PR 1 — Trigger schema + manual baseline.** Add a `trigger:` block to workflow YAML:
```yaml
trigger:
  on: manual                    # manual | cron | event
  # cron-only:  cron: "0 4 * * *"
  # event-only: event: github.pr.opened
  #             filter: "{{event.repo.name == 'rupu'}}"
```
Manual is the existing default (no scheduler / receiver yet). All this PR does is parse + validate the block and document the surface; no runtime wiring beyond the existing manual path.

**PR 2 — Cron runtime.** Three options for scheduling: (a) long-running rupu daemon keeping cron schedules in process and firing `rupu workflow run --trigger cron <name>`; (b) emit per-workflow systemd-timer / launchd-plist scaffolding via `rupu workflow install <name>`; (c) `rupu cron tick` invoked from system cron. Option (c) is the cheapest first step — system cron does the scheduling, rupu just exposes the firing entry point. (a) is the durable answer.

**PR 3 — Event triggers** (the substantial one). Webhook receiver subcommand (`rupu webhook serve` or similar) listening on a configurable port, validating provider HMACs (GitHub `x-hub-signature-256`, GitLab `x-gitlab-token`), routing to workflows whose `trigger.on: event` + `event:` + optional `filter:` expression matches. Initial event vocabulary worth supporting:

- **SCM events** (built atop the Slice B-2 connectors): `github.repo.cloned` (proxy: webhook on push to default branch + first-fetch detection), `github.issue.created` / `github.issue.updated` / `github.issue.closed`, `github.pr.opened` / `github.pr.review_requested` / `github.pr.merged`, `github.push`, plus GitLab equivalents.
- **Issue-tracker queue events**: `issue.entered_queue:<queue>`, `issue.left_queue:<queue>`, `issue.state_changed:<from>-><to>` (e.g., `triage->ready`).

Each event populates `{{event.*}}` template bindings (repo, issue number, author, body, …) usable in `when:` filters and step prompts. Polling is a fallback for IdPs without webhooks.

**Why deferred (multi-PR):** the schema is bounded but the runtime is three different daemons (cron tick, webhook receiver, polling fallback) and each needs auth + replay-safety + idempotency design. Ship PR 1 alongside Tier 1 orchestration so users see the trigger surface; PR 2 + 3 follow.

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

**Approval gates (`approval: required`).** ✅ shipped (PR 1: persistent run state in `<global>/runs/<id>/`; PR 2: schema + runner pause/resume + `rupu workflow approve` / `reject`). Webhook + cron callers report paused runs (run-id in JSON response / log). `timeout_seconds:` is parsed but enforcement is deferred — a future ticker daemon could expire stale paused runs.

**Why deferred:** all three are bigger than a single PR's scope and Tier 1 (when / continue_on_error / inputs / defaults) closes the most painful gaps first.

## Workflow output_config / context_management / speed (Anthropic feature flags)

Body fields surfaced in claude-cli that rupu doesn't yet emit:
- `output_config: { effort?, task_budget?, format? }` — output shape constraints
- `context_management: { type: "tool_clearing", ... }` — automatic context pruning when conversation grows large
- `speed: "fast"` — fast-mode toggle (account-gated)

Anthropic-specific body fields the SDK emits when corresponding features are enabled. None affect rate-limit/quota; all are quality/perf tuning. Add as agent-frontmatter knobs (`outputFormat: json`, `contextManagement: tool_clearing`, `speed: fast`) once a real use case lands.

**Why deferred:** lower-leverage than Tier 1 orchestration / triggers / panel steps; nobody has asked for these specific levers yet.

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

**Why deferred (not fixed now):** today's pin works against the live API. Reverse-engineering `cch` is only worthwhile if the static value stops working — premature otherwise. The durable cure is the rupu-specific OAuth client registration (next item), which sidesteps the WAF fingerprinting entirely.

## Register rupu-specific OAuth clients with each vendor (impersonation cleanup)

Slice B-1's SSO flows currently impersonate first-party CLIs:
- **Anthropic**: Claude Code's OAuth client `9d1c250a-e61b-44d9-88ed-5944d1962f5e`. Consent screen reads "Claude Code wants access ...". Request shape (URL, scopes, state-as-verifier, JSON token body) is mirrored from Claude Code/pi-mono so claude.ai's server accepts it.
- **OpenAI**: Codex CLI's `app_EMoamEEZ73f0CkXaXp7hrann`. Required ports 1455/1457 are pinned by OpenAI's Hydra registration for that client.

Long-term these should be rupu-specific OAuth clients so users see "rupu wants access ..." on the consent screen. Steps:
1. Apply via the vendor's developer console (Anthropic: console.anthropic.com OAuth apps; OpenAI: platform.openai.com app registration; Google: GCP project OAuth credentials).
2. Replace per-provider `client_id`, redirect URI, allowed ports, scopes in `crates/rupu-auth/src/oauth/providers.rs` with rupu's registration.
3. Drop the comment block in that file's docstring acknowledging impersonation.
4. Re-test all four providers' SSO flows end-to-end before release.

**Why deferred:** vendors take days to weeks to approve OAuth client registrations, and matt's primary use case (paid Claude.ai subscribers running inference) works today via impersonation. Revisit once a clean release/branding window exists.

## Code-signing & keychain trust

Status:
- ✅ **Layer 1 (local dev)**: `Makefile` + `scripts/sign-dev.sh` sign every `cargo build` with the Developer ID Application cert. Click "Always Allow" once on the first keychain prompt; the signing identity is stable across rebuilds, so subsequent builds inherit the trust.
- ✅ **Layer 2 (released binaries)**: `scripts/notarize-release.sh` submits a signed binary to `xcrun notarytool` with the `rupu` keychain profile. `docs/RELEASING.md` updated with the one-time `notarytool store-credentials` setup and the per-release notarization step. Notarized binaries are trusted by Gatekeeper out of the box for end users.
- ⚠️ **Layer 3 (programmatic keychain ACL)** — deferred. After L1+L2, the dev re-prompt is gone (stable signing identity = persistent "Always Allow") and end-user releases are notarized, so most of the user-facing pain is solved. Layer 3 would remove the *first-time* "Always Allow" prompt by setting the keychain item's ACL to include the rupu signing identity directly via `Security.framework` / `SecAccessCreateWithFlags`. The `keyring` crate v3 doesn't expose this — needs a small native shim (or upstream contribution). Worth doing only if first-launch friction becomes a complaint.

## `rupu usage` aggregation subcommand (deferred from Slice B-1)

Slice B-1 captures `Event::Usage` per response in JSONL transcripts. A `rupu usage` subcommand would aggregate across transcripts (per-day, per-agent, per-provider) for cost visibility. Deferred to Slice D so the same aggregator can also power the SaaS dashboard rather than building two implementations.

## Gemini API-key support via AI Studio (deferred from Slice B-1 Plan 1 Task 13)

The lifted `GoogleGeminiClient::new` rejects `AuthCredentials::ApiKey` — only the OAuth/Vertex path is implemented. AI Studio's API-key endpoint (`https://generativelanguage.googleapis.com/v1beta/...`) is a separate code path that's not yet wired. **Decision point for Plan 2:** add a dedicated `GoogleGeminiAiStudioClient` (separate constructor against AI Studio endpoint) OR rely entirely on Vertex SSO and document that Gemini API-key is a non-goal. Spec §4 currently implies API-key should work for all four providers, so the AI Studio path is the cleaner answer if cost is acceptable.

## Copilot `default_model` inconsistency (low-priority polish)

`crates/rupu-providers/src/github_copilot.rs:410`'s trait `default_model()` returns `"claude-sonnet-4"`, but other places in the same file (lines 497, 597, 749, 1996) use `"claude-sonnet-4-6"`. Either align the trait default to `"claude-sonnet-4-6"` (likely correct) or decide both are intentional and document why. Minor — only surfaces when an agent file omits `model:` and the provider is Copilot.

## ProviderError truncate-helper test gap (deferred from Plan 1 Task 11)

`crates/rupu-providers/src/classify.rs::truncate` uses `s.is_char_boundary` walk-back, but the regression test in `crates/rupu-providers/tests/classify.rs::classify_handles_multibyte_utf8_body_without_panic` uses a 4-byte-aligned input (`"🦀".repeat(N)` with `N % 4 == 0`), so the walk-back loop is never actually exercised. A truly adversarial test should use a 3-byte char (e.g. `"€".repeat(N)`) where `(max % 3 != 0)` to exercise the walk-back path. Plan 2/3 polish.
