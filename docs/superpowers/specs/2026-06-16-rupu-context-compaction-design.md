# rupu Context Compaction — Design

**Date:** 2026-06-16
**Status:** Approved (design); implementation pending
**Author:** matt + Claude

## Problem

Long-running agent sessions accumulate conversation history until the prompt
approaches the model's context window. At that point every turn re-sends ~1M
tokens, turns become slow and expensive, and eventually the request 400s with
`prompt is too long: <N> tokens > <window> maximum`.

Two mechanisms exist or were considered:

- **Reactive trim+retry ("B", shipped in 0.9.3, #297).** On a context-overflow
  error the runner drops the oldest assistant↔user exchange and retries until
  the request fits. This prevents crashes but is *lossy* (old turns are dropped,
  not summarized) and *keeps the session pinned at the ceiling* — it trims only
  the minimum to fit, so every subsequent turn re-overflows and re-trims. Observed
  in practice: a 330-turn session ran 10 turns all at 988k–999k input, ~10M input
  tokens for one run.

- **LLM compaction (this design).** When the prompt approaches the window,
  *summarize* the older conversation into a compact synthetic message instead of
  dropping it. This preserves the important information AND creates real headroom,
  so the session drops well below the ceiling and has a long runway before the
  next compaction.

Compaction is the proactive primary; B remains the reactive fallback.

## Goal

When a turn's input approaches a configured fraction of the model's context
window, summarize older conversation into one compact message, preserving the
original task and recent turns verbatim, so the session continues with headroom
instead of thrashing at the limit. Opt-in per agent; safe and off by default.

## Configuration (agent frontmatter)

Two new optional fields on the agent `.md` frontmatter:

```yaml
contextWindowTokens: 1000000   # model's context window, in tokens
compactAtPercent: 75           # compact when a turn's input exceeds this % of the window
```

- `contextWindowTokens: Option<u32>` (serde rename `contextWindowTokens`).
  The token denominator the percentage applies to. Chosen as an explicit token
  count (rather than reusing the coarse `contextWindow` tier enum) so it works
  for non-standard / preview models like `claude-mythos-preview` that rupu
  cannot otherwise size. **Absent ⇒ compaction is disabled** (B-style reactive
  trim remains the only safety net).
- `compactAtPercent: Option<u8>` (serde rename `compactAtPercent`), default `80`
  when `contextWindowTokens` is set and this is omitted. Clamped to `[10, 95]`.

Both flow: `Frontmatter` → `AgentSpec` → `AgentRunOpts`.

## Trigger (proactive)

In the `run_agent` turn loop, after a turn completes the runner already reads
`resp.usage.input_tokens` (runner.rs:539). Add: if compaction is enabled
(`contextWindowTokens.is_some()`) and

```
resp.usage.input_tokens as u64 > (compact_at_percent as u64 * context_window_tokens as u64) / 100
```

then run compaction on the working `messages` *before* the next turn's request
is built. The check uses the **actual** prompt size reported by the provider —
no client-side tokenizer is needed for triggering.

## Compaction algorithm

Input: the runner's working `Vec<Message>` (the conversation excluding the
system prompt, which lives separately and is never compacted — so the coverage
catalog etc. always remain).

1. **Partition** into `task | middle | recent`:
   - `task` = the first message (the original user objective). Always kept.
   - `recent` = the most-recent messages whose raw char sum is ≤
     `recent_budget_chars` (derived via calibration — see "Token estimation and
     recent-sizing calibration" below), walking from the end. Never split a
     tool_use/tool_result pair — if the boundary lands between an assistant
     tool_use and its following tool_result user message, include both. Always
     keep at least the last complete exchange.
   - `middle` = everything between `task` and `recent`.
   - If `middle` is empty (nothing old enough to summarize), skip compaction and
     let B handle any overflow.

2. **Dump** the full pre-compaction `messages` as JSON to
   `<transcripts_dir>/compaction/<run_id>-<seq>.json` (audit + recovery). Best
   effort — a write failure logs a warning but does not abort compaction.

3. **Summarize** `task + middle` via a one-shot `opts.provider.send()` with a
   non-streaming request: the messages to summarize plus a structured system/user
   instruction:

   > Summarize the conversation so far for continuation. Preserve, as concise
   > structured notes: the objective/task; key decisions and conclusions;
   > findings and their locations; files/areas already examined; the current
   > state; and any open threads or planned next steps. Omit chit-chat and
   > redundant tool output. This summary replaces the omitted turns, so it must
   > be self-contained.

   Output budget for the summary call: `min(8192, output ceiling)`. The call uses
   the same provider+model as the agent (configurable model is out of scope v1).

4. **Rebuild** `messages`:
   ```
   [ user( "<original task text>\n\n## Summary of prior work\n<summary>" ) ]
   ++ recent
   ```
   The synthetic first message is a `user` message so the list still starts with
   `user`; `recent` begins with an assistant message (its first kept exchange),
   preserving alternation and pairing.

5. **Emit** a transcript event so it is visible, not silent — an
   `Event::AssistantDelta` (or a dedicated event if cleaner):
   `[context compacted: summarized N turns; ~Xk→Yk tokens; backup <path>]`.
   Also `tracing::info!`.

6. The loop continues. `run_agent` returns `final_messages` = the working
   `messages`; the session assigns `session.message_history = result.final_messages`
   (session.rs:6176) and persists it. **The compacted state therefore sticks
   across turns with no session.rs changes.**

## Token estimation and recent-sizing calibration

**Triggering** uses the provider's real `resp.usage.input_tokens` — no
client-side tokenizer is needed here.

**Sizing the recent window** uses a calibrated char budget rather than a fixed
chars/4 estimate. The old approach (chars/4 against `context_window_tokens / 2`)
undercounted real tokens ~2x for code/JSON-heavy conversations: a history
costing ~990k real tokens estimated at ~520k, which fell within the ~500k
budget, leaving nothing to summarize and silently skipping compaction.

The calibrated approach (`compact_context`, threading `last_input_tokens` from
the triggering turn):

1. Sum raw chars across all message content blocks: `total_chars`.
2. Derive tokens-per-char from the provider's real charge:
   `tokens_per_char = max(0.25, last_input_tokens / total_chars)`.
   The 0.25 floor matches the minimum that chars/4 would give; guarding avoids
   division-by-zero and extreme values.
3. Target landing the kept-recent portion at **half the compaction threshold**
   (e.g. 750k threshold → land recent at ~375k tokens), giving real headroom
   before the next compaction triggers.
4. Convert to a char budget:
   `recent_budget_chars = (threshold / 2) / tokens_per_char`.

`partition_for_compaction` now accepts `recent_budget_chars: usize` and
accumulates `message_chars(msg)` (raw char count of all content blocks) walking
from the end, instead of the old `estimate_tokens` (chars/4) approximation.

A legacy `estimate_tokens` helper is kept for reference and unit tests but is
no longer called by production sizing paths.

## Layering with B (existing reactive trim+retry)

- **Compaction** is proactive: it runs between turns when usage crosses the
  threshold, preventing the wall in the first place.
- **B** stays as the reactive fallback inside the provider-call retry loop, for:
  compaction disabled (no `contextWindowTokens`); a single turn that is
  individually too large even after compaction; or a summarizer call that failed.
- Order of defense: proactive compaction → request → on `prompt is too long`,
  B trims+retries → if B cannot trim, `RunError::ContextOverflow`.

## Error handling

- **Summarizer call fails** (provider error): log `tracing::warn!`, skip
  compaction this cycle, keep the original `messages`. The next overflow is
  caught by B. Compaction failure never aborts the run.
- **Backup dump fails**: log, proceed with compaction (the backup is a
  convenience, not a correctness requirement).
- **Degenerate partitions** (empty middle, recent already ≥ window): skip
  compaction; rely on B.

## Touch points

- `crates/rupu-agent/src/spec.rs`: add `contextWindowTokens`, `compactAtPercent`
  to `Frontmatter` and `AgentSpec`; default/clamp logic.
- `crates/rupu-agent/src/runner.rs`: `AgentRunOpts` gains the two fields; the
  turn loop gains the trigger check and calls a new `compact_context(...)`;
  helpers `estimate_tokens`, `partition_for_compaction`, the summary prompt
  builder, and the temp-file dump. Reuses the existing `Message`/`Role`/
  `ContentBlock` types and `provider.send`.
- **No `session.rs` changes** for persistence (`message_history` already
  round-trips `final_messages`). Sessions inherit compaction automatically.
- Agent config: set `contextWindowTokens: 1000000` + `compactAtPercent: 75` on
  `~/.rupu/agents/oracle-assessor.md` (matt's, edited directly) and on the
  shipped `examples/agents/security-assessor.md`.

## Testing

Pure, unit-testable helpers + a mock-provider integration test:

- `estimate_tokens` — monotonic, ~chars/4.
- `partition_for_compaction` — keeps task + last exchange; respects
  `recent_budget`; never splits a tool_use/tool_result pair; empty-middle ⇒
  signals skip.
- Rebuilt-message assembly — starts with `user`, alternation preserved, summary
  text present, recent turns intact.
- Trigger arithmetic — fires strictly above `compactAtPercent% × window`, not at
  or below; disabled when `contextWindowTokens` absent.
- Summarizer-error fallback — `MockProvider` returning an error for the summary
  call ⇒ `messages` unchanged, run proceeds.
- Default/clamp for `compactAtPercent`.

## Out of scope (v1 — YAGNI)

- Configurable summarizer model (`compactModel`) — use the agent's model.
- Configurable land-target percent (`compactKeepRecentPercent`) — derived as
  half the window.
- Semantic dedup / multi-level summary hierarchies.

These are noted for a follow-up if real usage warrants them.
