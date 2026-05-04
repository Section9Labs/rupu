# rupu — backlog

Items deferred from completed slices. Each entry should name **why deferred**, **prereqs**, and **what unblocks it**.

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
