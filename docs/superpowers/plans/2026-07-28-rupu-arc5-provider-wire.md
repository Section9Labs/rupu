# Arc 5 — provider wire correctness

Fifth arc of the integrity-remediation program
(`docs/superpowers/specs/2026-07-24-rupu-integrity-remediation-charter.md`).
Covers **I-45 … I-49**.

All five were verified against the code before this plan was written. **The triage's
priority order is wrong**, and two titles state as fact things the repo's own source marks
unverified. Corrections first.

## Corrections to the triage (read before working anything)

| ID | Filed as | Actually |
|---|---|---|
| **I-45** (P0) | "**Gemini 3** gets a guaranteed 400" | Both keys are co-sent for **every** non-`Auto` level, and **there is no model gate at all** — so `thinkingLevel` (a Gemini-3-only key) also goes to `gemini-2.5-pro`, which is this provider's `ModelTier::Default`. Blast radius is *wider* than filed. But the "guaranteed 400" is **doc-derived, never observed in-repo** — the only source is our own design doc reading Google's docs. |
| **I-46** (P1) | "the API expects lowercase" | The uppercase wire strings are real (hardcoded literals, serde uninvolved). The *"API expects lowercase"* half is stated as fact, but our own spec says **"Unverified whether the API tolerates both."** |
| **I-47** (P1) | "`xhigh`/`minimal` are **rejected outright** by DeepSeek, Groq and xAI" | Forwarding-without-an-allowlist is real. "Rejected outright" is **unproven** — our own spec says 400-vs-silent-ignore is *"undocumented across vendors."* The per-vendor ladders in the title are also garbled, and **none of the three are shipped providers** (no preset, no `ProviderId`) — they're reachable only as user-declared openai-compatible entries. |
| **I-48** (P1) | a cost issue | **This is the most severe of the five and the only one reachable from a default config.** Every Gemini run that thinks (2.5-pro thinks by default) silently under-reports tokens *and* under-bills, with no `cost_partial` marker, so a user cannot tell their spend is understated. |
| **I-49** (P2) | "every 429 backs off a flat 60s" | **False at every constant involved.** The stub is real, but `classify_*` has **zero production callers**, so `ProviderError::RateLimited` is never constructed in production and the stub is inert. Real default backoff is `DEFAULT_MAX_RETRIES = 1` with `2000ms << attempt` — **one 2s retry**. The only 60s is a `ModelPool` availability window in code with no production constructor. |

**Priority for this arc, corrected: I-48 first** (only default-reachable, silent money
impact), then I-45/I-46 together (same code path), then I-47, then I-49 as a re-triage.

## Global constraints

- `#![deny(clippy::all)]`; `unsafe_code` forbidden; thiserror in libs; workspace deps only.
- **Never package-wide `cargo fmt`** — per-file only. **Never `git commit -a`/`-am`. Never `git stash`.**
- **Validation bar (charter §3.2):** observe at the consumer. For wire changes that means
  asserting the **built request body** / **parsed usage**, not that a helper returns a value.
- Known-red baseline, do NOT chase: 4 `linear_runner.rs` (I-4); `tools_list_matches_snapshot`
  in rupu-mcp (I-81); rupu-cli ANSI/colour failures; `init_with_samples`; the
  self-terminating `cmd::session` test; `coverage_audit_cli.rs` clap.
- **`node` is currently broken in this worktree** (Homebrew `llhttp` upgrade left the binary
  linked against a missing dylib). No web work in this arc, so it doesn't block — but do not
  claim a web test run.
- Branch `arc5/provider-wire`, **stacked on `arc4/gate-action-correctness`** (PR #550).

---

### Task 1: I-48 — Gemini reasoning tokens must be counted (do this first)

**Why first:** it is the only one of the five a stock user hits, and it costs them money
silently.

**Evidence.** Both Gemini usage sites read exactly two fields —
`google_gemini.rs:709-717` (non-streaming) and `:884-896` (streaming):
`promptTokenCount` and `candidatesTokenCount`. `thoughtsTokenCount` appears **nowhere in
the workspace** (grep for `thoughts` across `crates/`: zero hits). `totalTokenCount` is
also unread.

There is **no field to put it in**: `types.rs:121-126` is
`Usage { input_tokens, output_tokens, cached_tokens }`.

Consequence is **both** cost and accounting: `pricing_config.rs:74-83`'s
`cost_usd(input, output, cached)` bills `output * output_per_mtok`, and Gemini reports
thinking tokens *outside* `candidatesTokenCount`, so they are billed by Google and
multiplied by zero here. `rupu-agent/src/runner.rs:1107,1120,1335` sum the same
`output_tokens` into `total_out`/`tokens_out`, so run totals and the context-budget
arithmetic under-report too.

**Contrast worth preserving in the fix's comment:** no provider in rupu reads a
reasoning-token field, but Anthropic and the openai-compatible path don't need one — their
`output_tokens`/`completion_tokens` already *include* reasoning. **Gemini is the only
provider where the omission loses tokens.** So this is provider-specific, not an instance
of a general gap.

- [ ] **Step 1: Write the failing tests.** Both Gemini paths, non-streaming and streaming,
  from a realistic `usageMetadata` containing `thoughtsTokenCount`. Assert the parsed
  `Usage` accounts for those tokens. Then a **cost** test: the same usage produces a cost
  strictly greater than the same response with `thoughtsTokenCount: 0`. The cost assertion
  is the binding one — a usage-struct assertion alone doesn't prove the money is right.
- [ ] **Step 2: RED. Step 3: implement.**

  **Decide and record which shape you take**, because it is a real design call:
  - (a) add `reasoning_tokens: u32` to `Usage` and include it in `cost_usd`, or
  - (b) fold `thoughtsTokenCount` into `output_tokens` at the Gemini parse site.

  (a) is more honest and keeps the number visible in transcripts; (b) is smaller but
  conflates two things and makes the transcript silently disagree with Google's console.
  **Prefer (a)** unless it ripples further than expected — `Usage` is constructed in many
  places, so use `..Default::default()` friendliness and check every construction site.
  If you take (b), say why in the report.
- [ ] **Step 4: GREEN.** Confirm no other provider regressed — Anthropic and openai-wire
  must be untouched, since their output counts already include reasoning.
- [ ] **Step 5: Commit** — `fix(providers): count Gemini reasoning tokens in usage and cost (I-48)`

---

### Task 2: I-45 + I-46 — gate the Gemini thinking config on the model (one change)

Both live in the same `serde_json::json!` literal at `google_gemini.rs:346-374`, so they
are one edit.

```rust
serde_json::json!({
    "includeThoughts": true,
    "thinkingLevel": level_str,   // Gemini-3-only key
    "thinkingBudget": budget,     // Gemini-2.5 key
})
```

Set for **every** non-`Auto` level; `Auto` alone sends `{includeThoughts, thinkingBudget: -1}`.

- [ ] **Step 1:** Add a **model gate**. This is the part the filed issue missed and it is
  the substance of the fix: pick `thinkingLevel` for Gemini 3 models and `thinkingBudget`
  for 2.5, **never both**. Find how the model string is available at
  `build_request_body` and gate on it. Do not gate on the provider — both models run
  through the same provider.
- [ ] **Step 2:** Send the level **lowercase** (I-46). Our own spec says lowercase is what
  Google documents, while noting tolerance is unverified — lowercase matches the
  documented contract, so it is the right default even though we cannot prove the
  uppercase form fails.
- [ ] **Step 3: Revise the tests that pin the bug.** `google_gemini.rs:1092`
  (`test_build_request_body_with_thinking`) currently asserts `thinkingLevel == "MEDIUM"`
  **and** `thinkingBudget == 8192` on the same body — it actively pins both defects.
  Also `:1121` (`test_build_request_body_thinking_max_clamped`) and `:1871`
  (`test_thinking_levels`). Revise deliberately; add a test asserting the two keys are
  **never both present**, whichever model is used.
- [ ] **Step 4:** In the report, state plainly that the "400" was **not** reproduced — we
  are conforming to the documented contract, not fixing an observed failure. Do not write
  a commit message or comment claiming an observed 400.
- [ ] **Step 5: Commit** — `fix(providers): gate Gemini thinking config on the model and lowercase the level (I-45, I-46)`

---

### Task 3: I-47 — stop forwarding values a provider can't accept, without over-claiming

`openai_wire.rs:176-189` maps `Minimal → "minimal"` and `Max → "xhigh"` and forwards them
verbatim. `build_chat_request_body` serves **every** openai-compatible endpoint plus
GitHub Copilot. There is no per-provider allowlist, no model gate, no local rejection.

**But scope it honestly.** "Rejected outright" is unproven; the vendor ladders in the title
are garbled; and DeepSeek/Groq/xAI aren't shipped providers — they exist only if a user
declares an openai-compatible entry pointing at them.

- [ ] **Step 1: Decide the shape and argue for it in the report.** Options, from least to
  most invasive: (a) document the caveat in `docs/providers.md` and leave the wire alone;
  (b) add an **opt-in** per-provider `reasoning_effort_values` allowlist in provider config
  so an operator can constrain it; (c) clamp `Minimal`/`Max` to `low`/`high` for
  openai-compatible endpoints by default.

  **(c) is tempting and probably wrong** — it would silently downgrade a user's explicit
  `effort: max` on an endpoint that *does* support `xhigh` (OpenAI's own gpt-5.5 does), and
  we have no evidence any vendor rejects it. Prefer (a), or (b) if it's cheap. Whatever you
  choose, **the openai-wire mapping and the Codex mapping are byte-identical duplicated
  `match` blocks in two files** — if you add logic, factor it into one shared helper rather
  than duplicating a third time.
- [ ] **Step 2:** If you touch the mapping, note `openai_codex.rs:1441` asserts
  `reasoning.effort == "xhigh"` and would need revising.
- [ ] **Step 3: Commit** — message to match the shape chosen.

---

### Task 4: I-49 — re-triage; the filed statement is false

**Do not implement the filed issue.** Verify, then re-title and either fix the *real*
defect or close it as not-a-defect with the evidence recorded.

Established facts: `parse_retry_after` (`classify.rs:11-16`) is a stub returning `None`,
but `classify_anthropic|openai|gemini|copilot` have **zero production callers** — every
real client builds `ProviderError::Api` directly, so `ProviderError::RateLimited` is never
constructed in production. Real default backoff is `DEFAULT_MAX_RETRIES = 1` and
`retry_backoff = 2000ms << attempt` capped at 60s — **one 2s retry** by default. The lone
60s literal is a `ModelPool` availability window whose only caller passes `None` anyway,
in a type with no production constructor.

- [ ] **Step 1: Re-verify** the "zero production callers" claim yourself; it is the crux.
- [ ] **Step 2: Re-title I-49** to whichever is true, and record the false original:
  - *"`classify.rs` is entirely dead code with a passing test suite"* — the honest finding,
    same silent-dead-code class as I-27 in Arc 2; **or**
  - *"`RetryingProvider` ignores server-supplied `Retry-After` by design"* — the real
    behavioral gap, if you judge that worth fixing.
- [ ] **Step 3:** Either wire `classify_*` into the clients (so `Retry-After` is honoured),
  or delete it. **Deleting is the smaller, more honest change** and matches how I-27 was
  handled; wiring is a behavior change that deserves its own scope. Recommend deletion +
  a follow-up issue for honouring `Retry-After`, and say which you did.
- [ ] **Step 4: Commit** — message to match.

---

### Task 5: Arc close-out

- [ ] `cargo build --workspace`; `cargo test -p rupu-providers -p rupu-config`. Compare
  failures against the known-red baseline.
- [ ] I-45…I-49 each in `## Fixed` with a validation naming what was observed, or in triage
  with a recorded reason. Retitled rows must show the corrected title. No row deleted.
- [ ] File the two adjacent findings recon surfaced, so they aren't lost:
  **(a)** `anthropic.rs:1088` × `:1101` nested idle-retry × 429-retry loops genuinely
  multiply attempts; **(b)** `provider_factory.rs:308-313` documents Anthropic's native
  backoff sleeping *inside* its concurrency permit as a known, deferred starvation bug.
- [ ] Open the PR against base `arc4/gate-action-correctness`.

## Deferred out of this arc

- Honouring server-supplied `Retry-After` in `RetryingProvider` (if Task 4 deletes rather
  than wires).
- A shared per-provider reasoning-effort translation helper — four hand-written ladders
  exist today (Anthropic budget tokens, Gemini level+budget, openai_wire string, Codex
  string behind a model gate), two of them byte-identical duplicates.
