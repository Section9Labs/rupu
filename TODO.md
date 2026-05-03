# rupu — backlog

Items deferred from completed slices. Each entry should name **why deferred**, **prereqs**, and **what unblocks it**.

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

## Code-signing & keychain trust (deferred from Slice B-1)

macOS keychain re-prompts on every rebuild because each binary has a different code identity. Three layers, do them in order:

1. **Local dev (fast fix, ~30 min):** `make sign-dev` target that runs a self-signed `codesign -f -s "rupu-dev"` after `cargo build`. Click "Always Allow" once on the first keychain prompt; future builds inherit the trust because the signing identity is stable. No Apple Developer account needed for this.
2. **Released binaries (proper fix):** add `Developer ID Application` signing + `xcrun notarytool` notarization to `docs/RELEASING.md`. Released binaries are trusted out of the box for end users. **Prereqs to verify before starting:**
   - `security find-identity -v -p codesigning` returns a `Developer ID Application: <name> (<TEAM_ID>)` entry (NOT `Apple Development` — that won't notarize).
   - `xcrun notarytool store-credentials rupu --apple-id <you@…> --team-id <TEAMID>` succeeds.
   - `entitlements.plist` allows `keychain-access-groups` for the hardened runtime.
3. **Programmatic keychain ACL (optional polish):** when `rupu auth login` writes a credential, set the keychain item's access-control list to include the rupu code-signing identity directly via `Security.framework` / `SecAccessCreateWithFlags`. Removes the "Always Allow" click on first use. The `keyring` crate v3 doesn't expose this — needs a small native shim (or contribute upstream).

## `rupu usage` aggregation subcommand (deferred from Slice B-1)

Slice B-1 captures `Event::Usage` per response in JSONL transcripts. A `rupu usage` subcommand would aggregate across transcripts (per-day, per-agent, per-provider) for cost visibility. Deferred to Slice D so the same aggregator can also power the SaaS dashboard rather than building two implementations.

## Gemini API-key support via AI Studio (deferred from Slice B-1 Plan 1 Task 13)

The lifted `GoogleGeminiClient::new` rejects `AuthCredentials::ApiKey` — only the OAuth/Vertex path is implemented. AI Studio's API-key endpoint (`https://generativelanguage.googleapis.com/v1beta/...`) is a separate code path that's not yet wired. **Decision point for Plan 2:** add a dedicated `GoogleGeminiAiStudioClient` (separate constructor against AI Studio endpoint) OR rely entirely on Vertex SSO and document that Gemini API-key is a non-goal. Spec §4 currently implies API-key should work for all four providers, so the AI Studio path is the cleaner answer if cost is acceptable.

## Copilot `default_model` inconsistency (low-priority polish)

`crates/rupu-providers/src/github_copilot.rs:410`'s trait `default_model()` returns `"claude-sonnet-4"`, but other places in the same file (lines 497, 597, 749, 1996) use `"claude-sonnet-4-6"`. Either align the trait default to `"claude-sonnet-4-6"` (likely correct) or decide both are intentional and document why. Minor — only surfaces when an agent file omits `model:` and the provider is Copilot.

## ProviderError truncate-helper test gap (deferred from Plan 1 Task 11)

`crates/rupu-providers/src/classify.rs::truncate` uses `s.is_char_boundary` walk-back, but the regression test in `crates/rupu-providers/tests/classify.rs::classify_handles_multibyte_utf8_body_without_panic` uses a 4-byte-aligned input (`"🦀".repeat(N)` with `N % 4 == 0`), so the walk-back loop is never actually exercised. A truly adversarial test should use a 3-byte char (e.g. `"€".repeat(N)`) where `(max % 3 != 0)` to exercise the walk-back path. Plan 2/3 polish.
