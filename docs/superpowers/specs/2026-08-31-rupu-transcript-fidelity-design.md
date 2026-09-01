# Transcript Fidelity — faithful, replayable transcripts, displayed everywhere

**Date:** 2026-08-31
**Status:** Approved direction (matt: "What happens (bit by bit) should be recorded and it should be displayed, as simple as that.")
**Plans:** three, in dependency order —
1. `docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-1-recording-and-replay.md` (schema v2 + runner emission + replay reconstruction + CLI renderers)
2. `docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-2-cp-web-display.md` (CP API honesty + web transcript full-parity rendering)
3. `docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-3-macos-display.md` (macOS TranscriptModels + TranscriptFeed full-parity rendering)

## 1. Principle

A run's transcript JSONL is the complete, ordered record of what actually
happened: every thinking block (in its true position, including redacted
ones), every assistant text block, every tool call and result, the user
prompt, the seed context, and every runtime intervention (compaction,
context-trim, retries). From the transcript alone (plus nothing else) the
exact `Vec<Message>` conversation the provider saw must be reconstructible —
byte-exact, including provider `raw` reasoning payloads (signatures). And
every recorded event kind is rendered by every transcript surface (CLI
pretty view, CP web, macOS app) — nothing is silently dropped.

## 2. Verified current gaps (audit of 2026-08-31, this session)

Recording (`crates/rupu-agent/src/runner.rs`, `crates/rupu-transcript/src/event.rs`):
- No first-class thinking event. Reasoning is a nullable `thinking` string on
  `assistant_message` (runner.rs:1162-1200): interleaved positions collapse
  onto the first Text block; tool-only turns record thinking AFTER the
  tool_call events; redacted/textless blocks vanish (`reasoning_text()`
  filters them, types.rs:263-277).
- Provider `raw` payloads (Anthropic `signature`, Gemini `thoughtSignature`,
  OpenAI `encrypted_content`) are never written — the transcript cannot
  rebuild a replayable conversation (signature continuity lives only in
  `paused_seed.json` / session records).
- `StreamEvent::ReasoningDelta` is a no-op (runner.rs:1002) — no live
  thinking in the JSONL while text gets per-chunk `assistant_delta`.
- The initial user prompt and the resume/session seed are not recorded at
  all. Neither is the effective system prompt, the provider response id, nor
  the stop reason.
- Compaction and context-trim/retry notices are jammed into free-text
  `assistant_delta` lines (runner.rs:465-469, 1040, 1067) — invisible to any
  renderer that (correctly) drops deltas, and compaction leaves the JSONL
  unable to describe the post-compaction conversation.
- Faithfulness bug: on an unknown/narrowed-out tool the transcript records
  `error: "unknown tool: {name}"` (runner.rs:1262) but the model is fed
  `"error: unknown_tool\n"` (runner.rs:1266) — record and reality diverge.

Display:
- CP web (`transcriptView.ts:460-465`) silently drops `gate_requested`,
  `turn_start`/`turn_end` (per-turn tokens), unpaired `tool_result`, and
  unpaired `file_edit`/`command_run`.
- CP API (`rupu-cp/src/api/transcript.rs:167`) drops unparseable lines via
  `.filter_map(Result::ok)` with no signal.
- CLI pretty view caps thinking at 96 chars in EVERY view mode including
  `--view full` (transcript.rs:797; mirrored in session.rs:4389,
  workflow_printer.rs:1298, streaming transcript.rs:1258); `net_flow` has no
  row at all (transcript.rs:1078).
- macOS (`TranscriptFeed.swift:120`) skips `tool_audit` / `action_emitted`
  entirely (they decode payload-less, TranscriptModels.swift:191-196) — no
  `actions:` audit surface; `file_edit.diff` is decoded but never shown;
  turn framing/usage never rendered.

Verified non-issue: orchestrator resume mints a fresh `run_{Ulid}.jsonl` per
attempt (rupu-orchestrator/src/runner.rs:5942-5943), so `JsonlWriter::create`
truncation never destroys pre-pause events. But a resumed attempt's
transcript starts mid-conversation with no recorded context — the Seed event
(below) fixes that.

## 3. Schema v2 (additive; adjacently tagged as today)

New `Event` variants in `crates/rupu-transcript/src/event.rs`:

| variant | wire `type` | payload | emitted when |
|---|---|---|---|
| `Thinking` | `thinking` | `text: Option<String>`, `provider: String`, `model: String`, `raw: Value` | per `ContentBlock::Reasoning`, in block order. `text: None` = redacted/omitted. `raw` is the byte-exact provider block — persisted, never displayed. |
| `ThinkingDelta` | `thinking_delta` | `content: String` | per `StreamEvent::ReasoningDelta` chunk (live mirror of `assistant_delta`; dropped by after-the-fact renderers) |
| `UserMessage` | `user_message` | `content: String` | when `opts.user_message` is appended (runner.rs:917) |
| `Seed` | `seed` | `message_count: u32`, `sha256: String`, `source_transcript: Option<String>`, `messages: Option<Value>` (exactly one of the last two is set) | when `opts.initial_messages` is non-empty (resume seed / session history) |
| `Compaction` | `compaction` | `seq: u32`, `summarized_messages: u32`, `backup_path: String`, `messages: Value` (full post-compaction `Vec<Message>`) | in `compact_context` on success (replaces the free-text delta note) |
| `Notice` | `notice` | `kind: String` (`"context_trim"` \| `"provider_retry"`), `message: String` | replaces the bracketed `assistant_delta` hacks at runner.rs:1040/1067 |
| `Unknown` | (any unrecognized) | unit, `#[serde(other)]` | never written; forward-compat read fallback (mirrors `ContentBlock::Unknown` precedent, types.rs:59-60) |

Additive fields on existing variants (all `Option` + `#[serde(default)]`,
skip-if-none, so legacy lines parse and old readers ignore them):
- `RunStart`: `schema: Option<u32>` (write `Some(2)`), `system_prompt:
  Option<String>` (the effective post-coverage-append system prompt).
- `TurnEnd`: `stop_reason: Option<String>`, `response_id: Option<String>`.
- `AssistantMessage.thinking` stays for reading legacy transcripts but is no
  longer written (always `None` from the v2 writer; serde skips it).

**Seed dedup rule (store once, reconstruct at read time).** A session's
turn-N context is exactly "replay turns 1..N-1" — so re-embedding it every
turn would store the same conversation O(n²) times. Instead the Seed event
stores a *reference*: `source_transcript` names a transcript whose
replay-reconstruction equals `initial_messages` byte-exact, plus a `sha256`
over the canonical seed JSON so replay can verify the chain instead of
trusting it. The caller supplies the reference via a new
`AgentRunOpts::seed_source`; the session worker passes the previous turn's
transcript path (chains resolve recursively — each link only embeds what's
new). When no caller vouches for a source (arbitrary `initial_messages`,
orchestrator pause/resume — rare and bounded, one seed per resume), the
seed falls back to a full inline `messages` embed, so every transcript
remains replayable. Failure honesty: a pruned/missing chain link or a hash
mismatch is a hard, named replay error — never silent partial
reconstruction. Total storage: O(conversation), full fidelity retained.

Emission-order contract (the "bit by bit" rule): within a turn, events are
written in the provider's block order — `Thinking` / `AssistantMessage` /
`ToolCall` exactly as `resp.content` orders them — then `ToolResult`s in
dispatch order, then `TurnEnd`. No more collapse-onto-first-text, no more
post-loop thinking flush.

Compat: old binaries reading v2 files drop unknown-type lines (pre-existing
serde behavior; acceptable on the beta cadence). New binaries reading old
files render legacy `assistant_message.thinking` as a thinking block. The CP
API stops silently dropping unparseable lines: the response carries an
`unparsed` count that both clients surface.

## 4. Replayability contract

New module `crates/rupu-agent/src/replay.rs`:
`reconstruct_messages(&[Event]) -> Result<Vec<Message>, ReplayError>` folds a
transcript back into the exact conversation: Seed → seed messages (inline, or
by recursively replaying the referenced transcript chain and verifying the
recorded sha256 — `reconstruct_transcript(path)` is the resolving
entrypoint); UserMessage
→ user turn; per turn, Thinking/AssistantMessage/ToolCall events (in order) →
one assistant `Message` (Reasoning blocks rebuilt with `raw` intact);
ToolResult events → the following user message of `ToolResult` blocks, using
the same error-formatting + clamp the runner feeds the model; Compaction →
replace accumulated state with the recorded post-compaction messages. A
round-trip test (MockProvider scripted with reasoning + tool turns) asserts
JSON-equality with the runner's real in-memory `messages`. The "unknown
tool" record/feed divergence is fixed by feeding the model the same string
the transcript records.

`paused_seed.json` and session records stay as the operational resume
mechanism this arc (converging resume onto `reconstruct_messages` is a
follow-up once the round-trip has soaked).

## 5. Display parity matrix (target: no silent drops anywhere)

| event | CLI pretty | CP web | macOS |
|---|---|---|---|
| `thinking` | dim `thinking` row; full body in Compact/Full (96-char cap removed there; Focused keeps one line); `[redacted reasoning]` when text is None; `raw` never displayed | collapsible thinking block in true position, full text, redacted chip | dim DisclosureGroup row in true position, redacted label |
| `thinking_delta` / `assistant_delta` | dropped in after-the-fact views (existing delta convention); live views may stream | dropped (existing convention) | dropped (existing convention) |
| `user_message` | `prompt` row | prompt block at turn top | UserPromptRow |
| `seed` | one-liner `seeded with N messages · from <source>` | marker row with a link to the source transcript (display-time reconstruction — the session view already stacks the prior turns' transcripts, so nothing is shown twice) | SeedRow one-liner naming the source |
| `notice` | notice row | notice block | NoticeRow |
| `compaction` | row with seq/summarized/backup | notice-style block | CompactionRow |
| `gate_requested` | exists | **new** gate block (port of macOS treatment) | exists |
| `turn_end` | exists | **new** per-turn token badge | **new** turn separator row |
| `tool_audit` | exists | exists (FIFO pairing) | **new** payload decode + AuditBadge pairing + standalone row |
| `action_emitted` | exists | exists (merged into audit) | **new** payload decode, merged into audit row |
| unpaired `tool_result` | exists | **new** standalone card | exists |
| unpaired `file_edit`/`command_run` | exists | **new** standalone rows | exists |
| `file_edit.diff` body | exists (Compact/Full) | exists | **new** disclosure body |
| `net_flow` | **new** one-liner (was dropped) | Network tab (already displayed — stays out of transcript feed) | Netflow tab (same) |
| unknown type / unparsed line | **new** `unrecognized event` row / count | **new** `unparsed: N` badge + unknown row | **new** UnknownRow + badge |

## 6. Out of scope (tracked elsewhere, unchanged by this arc)

- Remote-host transcript resolution (SSH/tunnel/bucket connectors reading
  the CP's local disk; SSE `stream_transcript` ignoring `?host=`) — the
  deferred remote-transcript design (see memory/TODO); this arc changes what
  a transcript *contains* and how a *reachable* one renders.
- `/api/transcript` pagination (whole-file read stays; noted as a follow-up).
- Retiring `paused_seed.json` / session `message_history` in favor of
  `reconstruct_messages` — follow-up after soak.
- Live tool/thinking deltas over the *session* SSE (TODO.md:314) — the
  transcript SSE tail picks `thinking_delta` up for free; the session
  channel's own protocol is separate.
