# Transcript Schema Reference

> Part of the rupu reference docs: [spec.md](spec.md) · [agent-format.md](agent-format.md) ·
> [workflow-format.md](workflow-format.md) · **transcript-schema.md**

---

## Overview

Every agent run rupu executes (a standalone `rupu run`, each workflow step and fan-out unit,
each sub-agent started with `dispatch_agent`, each session turn) writes an append-only log in
JSONL format. Each line is one event. A workflow run's own records (`run.json`,
`step_results.jsonl`, `events.jsonl`) live in the run store and point at each step's transcript. `rupu transcript list` globs JSONL files and reads the `run_start` event from
each for metadata — only that first event, which is what lets it sort thousands of transcripts
cheaply and then read just the `--limit` most recent ones end to end (`run_complete` at the tail
carries the status and token totals).

The transcripts directory is a shared namespace: `run:` workflow step transcripts, action-step
transcripts and per-run netflow ledgers are written there too, under the same `run_<ulid>.jsonl`
naming but with their own schemas. A file with no `run_start` anywhere is one of those, not a
damaged transcript — an agent transcript's `run_start` is written before the agent loop starts,
so it survives every tolerated damage mode. Readers classify such a file as
`ReadError::NotAnAgentTranscript` and skip it.

---

## File location

```
<transcripts-dir>/<run_id>.jsonl
```

Where `<transcripts-dir>` is:

- `<project>/.rupu/transcripts/` if the directory exists.
- `~/.rupu/transcripts/` (or `$RUPU_HOME/transcripts/`) otherwise (global fallback).

`rupu transcript archive` moves a standalone run's file to `<transcripts-dir>/archive/`.

**The filename is the canonical run identifier.** The `<run_id>` portion has the form
`run_<26-char-ULID>` (e.g., `run_01HXX3Y7K8NQVZ2P0M4BCJD5F6`). Individual events do not
repeat the `run_id` in their payload (except `run_start` and `run_complete`). Slice C remote
streaming wraps each event in a transport envelope `{run_id, workspace_id, event}` at the
network layer rather than inflating every intermediate event.

---

## File format

JSON Lines (`application/x-ndjson`). One event per line. Each line is a self-contained JSON
object with this shape:

```json
{"type": "<variant>", "data": {...}}
```

All field names are `snake_case` (`rename_all = "snake_case"` applied to every event variant).
The `type` discriminator is the snake_case event name (e.g., `"run_start"`, `"tool_call"`).

Optional fields are **omitted**, not written as `null`, when they have no value, and boolean
flags that default to `false` are omitted when `false` (the one exception is
`run_start.customer`, where `null` is meaningful). Treat a missing key as its default. Several
writers append to one file (the agent loop, the `tool_audit` hook, the netflow sink), each line
whole.

---

## Schema versions

`run_start.schema` says which writer produced the file. Current rupu writes `2`; a transcript
with no `schema` key is version 1.

| Version | What it records |
|---------|-----------------|
| 1 (no key) | The conversation as text. Reasoning, if any, is the plain `thinking` string on `assistant_message`, with no provider payload |
| 2 | Enough to rebuild the exact conversation sent to the provider: the effective `system_prompt` on `run_start`, the `user_message`, each reasoning block as a `thinking` event with the provider's byte-exact `raw` block, the `seed` the run started from, every `compaction`, and `assistant_block` for reply blocks with no event of their own |

Fields and events have been added within version 2 since it shipped (`outcome`, `recovery`,
`assistant_block`, `customer`, `codename`, `cache_write_tokens`, `purpose`). A reader should
accept a missing optional key and an unknown event type (see
[Unknown event types](#unknown-event-types)).

---

## Reading conventions

- **No `run_complete` event** — the run was aborted (mid-run crash). Treat as
  `RunStatus::Aborted`. Do not skip the file; prior events are valid and useful.
- **Truncated last line** — silently skip. Partial writes at crash time are safe to ignore.
- **Empty lines** — silently skip.
- **Bad JSON mid-file** — yields `Err(ReadError::Parse)` for that line; iteration continues
  with the next line. A single bad line does not abort reading the file.

---

## Enum types

### `RunStatus`

| Value     | Meaning                                              |
|-----------|------------------------------------------------------|
| `ok`      | Run completed normally                               |
| `error`   | Run ended with a provider or internal error          |
| `aborted` | Run was killed or crashed before `run_complete`      |

### `RunMode`

| Value      | Meaning                                     |
|------------|---------------------------------------------|
| `ask`      | Prompt user before write/bash tool calls    |
| `bypass`   | Execute all tools without prompting         |
| `readonly` | Deny write/bash; allow read/grep/glob only  |

### `FileEditKind`

| Value    | Meaning                              |
|----------|--------------------------------------|
| `create` | File was created (did not exist)     |
| `modify` | Existing file content was changed    |
| `delete` | File was removed                     |

---

## Event reference

### `run_start`

Emitted once at the very beginning of every run (agent or workflow step).

| Field          | Type              | Description                                |
|----------------|-------------------|--------------------------------------------|
| `run_id`       | string            | Matches the JSONL filename (without `.jsonl`) |
| `workspace_id` | string            | ULID-prefixed workspace id (`ws_…`)        |
| `agent`        | string            | Agent name from frontmatter                |
| `provider`     | string            | Provider name (e.g., `anthropic`)          |
| `model`        | string            | Model identifier (e.g., `claude-sonnet-4-6`) |
| `started_at`   | DateTime\<Utc\>   | RFC3339 timestamp                          |
| `mode`         | RunMode           | Permission mode for this run               |
| `schema`       | u32, optional     | Transcript schema version: `2` today; absent on version 1 |
| `system_prompt` | string, optional | The system prompt actually sent, after rupu appended its own sections (coverage catalog, run target, finding guidance) |
| `codename`     | string, optional  | The agent instance's human codename (e.g. `amber-lantern/heron`, `cobalt-harbor/heron#412`); absent on transcripts that predate codenames |
| `customer`     | string \| null, optional | The customer the run ran under. A slug; `null` = recorded as no customer; no key = the transcript predates customers |

`provider` and `model` are what the run was configured to use; a fallback hop appears later
as a `notice` and a `recovery`, not here.

```json
{"type":"run_start","data":{"run_id":"run_01HXX3Y7K8NQ","workspace_id":"ws_01HXX…","agent":"fix-bug","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-05-01T17:00:00Z","mode":"ask","schema":2,"system_prompt":"You fix failing tests…","codename":"amber-lantern/heron","customer":null}}
```

---

### `turn_start`

Emitted at the beginning of each agent turn (before the LLM request is sent).

| Field      | Type | Description                     |
|------------|------|---------------------------------|
| `turn_idx` | u32  | Zero-based turn counter         |

```json
{"type":"turn_start","data":{"turn_idx":0}}
```

---

### `usage`

Emitted once per provider call, right after the call returns and before any
`assistant_delta`/`assistant_message` events for that response. `output_tokens` is already the
billable output figure — reasoning tokens (e.g. Gemini's `thoughtsTokenCount`) are folded in
upstream, so no separate reasoning-token field exists on this event. A context-compaction
summariser call writes its own `usage` line with `purpose: "compaction"`: real, billed spend
that is not an agent turn (turn counters skip it; token and cost totals include it).

| Field           | Type             | Description                                                  |
|-----------------|------------------|----------------------------------------------------------------|
| `provider`      | string           | Provider name (e.g., `anthropic`)                             |
| `model`         | string           | Requested model id (used for pricing attribution)              |
| `served_model`  | string, optional | Actual model the provider served, if it differs from `model`   |
| `input_tokens`  | u32              | Input tokens for this response                                 |
| `output_tokens` | u32              | Billable output tokens (includes any reasoning tokens)         |
| `cached_tokens` | u32              | Prompt-cache reads included in `input_tokens` (default `0`)    |
| `cache_write_tokens` | u32, optional | Prompt-cache writes included in `input_tokens`. Only Anthropic reports it; omitted when `0` |
| `purpose`       | string, optional | Why the call happened when it is not a normal turn: `"compaction"` today. Absent for a normal turn |

```json
{"type":"usage","data":{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1024,"output_tokens":312,"cached_tokens":0}}
```

---

### `assistant_delta`

Emitted for each incremental text chunk while a streaming provider response is in flight —
zero or more of these precede the `assistant_message` event for the same message. Consumers
that only want the final text can ignore `assistant_delta` and read `assistant_message`
instead; consumers rendering a live typing effect read both.

| Field     | Type   | Description                        |
|-----------|--------|-------------------------------------|
| `content` | string | One incremental text chunk          |

```json
{"type":"assistant_delta","data":{"content":"I'll start by"}}
```

---

### `assistant_message`

Emitted when the LLM produces a text response (may be preceded by zero or more `assistant_delta`
events if the provider streams partial results, but rupu emits one `assistant_message` per
complete message block).

| Field      | Type             | Description                                     |
|------------|------------------|-------------------------------------------------|
| `content`  | string           | Full assistant text                             |
| `thinking` | string, optional | Version 1 reasoning text. Version 2 writers record reasoning as separate `thinking` events instead |

```json
{"type":"assistant_message","data":{"content":"I'll start by reading the test file to understand the failure."}}
```

---

### `tool_call`

Emitted when the agent requests a tool invocation.

| Field     | Type   | Description                                  |
|-----------|--------|----------------------------------------------|
| `call_id` | string | Provider-assigned call identifier            |
| `tool`    | string | Tool name (e.g., `bash`, `read_file`)        |
| `input`   | object | Tool input as a JSON object                  |

```json
{"type":"tool_call","data":{"call_id":"toolu_01ABC","tool":"bash","input":{"command":"cargo test -- --nocapture 2>&1 | head -40"}}}
```

---

### `tool_result`

Emitted after a tool call completes (or fails).

| Field         | Type             | Description                                   |
|---------------|------------------|-----------------------------------------------|
| `call_id`     | string           | Matches the `tool_call` `call_id`             |
| `output`      | string           | Tool output text                              |
| `error`       | string, optional | Error description if the tool failed          |
| `duration_ms` | u64              | Wall-clock time the tool took, in ms          |
| `structured`  | object, optional | A machine-readable payload some tools add next to the text `output`, e.g. `ast_grep`'s matches with ranges and metavariable bindings. Absent on older transcripts and for tools that add none |

```json
{"type":"tool_result","data":{"call_id":"toolu_01ABC","output":"error[E0308]: mismatched types\n  --> src/parser.rs:142","duration_ms":843}}
```

---

### `file_edit`

Derived event emitted alongside `tool_result` when the tool kind is `write_file` or `edit_file`.
Consumers can index on `file_edit` events without parsing `tool_call` inputs.

| Field  | Type         | Description                              |
|--------|--------------|------------------------------------------|
| `path` | string       | Absolute path of the file that changed   |
| `kind` | FileEditKind | `create`, `modify`, or `delete`          |
| `diff` | string       | Unified diff of the change               |

```json
{"type":"file_edit","data":{"path":"/Users/matt/Code/myproject/src/parser.rs","kind":"modify","diff":"@@ -140,7 +140,7 @@\n-    let x: i32 = val;\n+    let x: usize = val;\n"}}
```

---

### `command_run`

Derived event emitted alongside `tool_result` when the tool kind is `bash`. Consumers can
index on `command_run` events without parsing `tool_call` inputs.

| Field          | Type         | Description                              |
|----------------|--------------|------------------------------------------|
| `argv`         | array\<string\> | Command tokens                        |
| `cwd`          | string       | Working directory the command ran in     |
| `exit_code`    | i32          | Process exit code                        |
| `stdout_bytes` | u64          | Bytes written to stdout                  |
| `stderr_bytes` | u64          | Bytes written to stderr                  |

```json
{"type":"command_run","data":{"argv":["cargo","test","--","--nocapture"],"cwd":"/Users/matt/Code/myproject","exit_code":1,"stdout_bytes":0,"stderr_bytes":512}}
```

---

### `action_emitted`

**Two different things have shared this event name across rupu's history — do not confuse them:**

**1. Live action-node shape (current, real effects).** Written by
`execute_action_step` (`rupu-orchestrator`) whenever a standalone `action:`
workflow step calls its catalog tool (the same tool body an agent or `rupu
mcp serve` calls). `kind` is the tool name as the step wrote it (e.g. `issues.create`, not a bespoke verb);
`payload` is the rendered `with:` args sent to the connector; `applied` is
`true` whenever the dispatcher call actually reached the connector
(regardless of whether the connector call itself succeeded) and `false` only
when the call was denied before reaching it. This event is always followed
immediately by a `tool_audit` event covering the same call — `tool_audit`,
not `action_emitted`, is what the CP transcript panel renders a badge from.

| Field     | Type             | Description                                                          |
|-----------|------------------|-----------------------------------------------------------------------|
| `kind`    | string           | MCP catalog tool name (e.g. `issues.create`, `scm.prs.comment`)      |
| `payload` | object           | Rendered `with:` args sent to the connector                          |
| `allowed` | bool             | `false` only for `McpError::PermissionDenied` (the call never reached the connector) |
| `applied` | bool             | Whether the dispatcher call reached the connector (`true`) or was denied before reaching it (`false`) |
| `reason`  | string, optional | Explanation when `allowed` or `applied` is `false`                    |

```json
{"type":"action_emitted","data":{"kind":"issues.create","payload":{"title":"null pointer in parser.rs:142","body":"..."},"allowed":true,"applied":true}}
```

**2. Legacy finding/verb shape (dead, no longer producible).** Before the
`actions:`/`action:` catalog validation landed, `kind` could be an
Okesu-heritage free-form verb such as `log_finding` or `propose_edit` that
did not correspond to any real MCP tool, and `applied` was always `false`
(no effect was ever executed). `validate_step_actions` (`rupu-orchestrator`,
`workflow.rs`) now rejects any `actions:`/`action:` entry that isn't a real
MCP catalog tool name at workflow-parse time, so this shape can no longer be
produced by any current code path. It is documented here only so a reader of
an old transcript file understands what they're looking at.

---

### `tool_audit`

The audit line for one tool call. Every call ends in exactly one, whatever the tool (core,
coverage/findings, connector, agentiflow) and whatever happened to it: allowed, denied by the
run's permission policy, refused by the tool's own gate, or not in the run's grant at all.
Emitted from two choke points: the agent loop, right after the call's `tool_result` (before a
`run_complete` when the operator stops the run at the call's prompt), and `execute_action_step`
for `action:`-node calls, immediately after that call's `action_emitted` line. Never confuse
`tool_audit` with `action_emitted`: `tool_audit` is the general audit line, and it is what the CP
transcript panel renders a badge from (blocked / not granted).

| Field        | Type    | Description                                                                                     |
|--------------|---------|---------------------------------------------------------------------------------------------------|
| `tool`       | string  | The tool name as the model called it (canonical or a legacy alias, matching its `tool_call`); for an `action:` node, the step's tool |
| `declared`   | bool    | Whether the step's `actions:` names `tool`. `false` both when `actions:` is empty/absent (unrestricted — not a violation) and when it doesn't name this tool — use `restricted` to tell those apart |
| `granted`    | bool    | Whether the run's grant covered `tool` before any `actions:` narrowing. Always `true` for an `action:` node |
| `blocked`    | bool    | Whether the call was denied (`decision` is not `allowed`) |
| `restricted` | bool    | Whether the step declared a non-empty `actions:` allowlist at all (disambiguates `declared: false`) |
| `reason`     | string? | Why the tool was in the grant, as comma-separated reasons (the `tool_grant` entry's), or `action` for an `action:` node. Absent when the grant didn't cover the call, and on older lines |
| `decision`   | string? | `allowed`, `denied:readonly`, `denied:operator` (answered no at the prompt), `denied:operator_stop`, `denied:tool` (the tool's own gate refused), or `not_granted` (the model named a tool the run doesn't offer). Absent on lines written before every call was audited |

```json
{"type":"tool_audit","data":{"tool":"write_file","declared":false,"granted":true,"blocked":true,"restricted":false,"reason":"declared","decision":"denied:readonly"}}
```

---

### `tool_grant`

The tools the run offered its model, and why. Written once, right after `run_start` and its
`model_limits` notice. `entries` is exactly the model's tool list.

| Field         | Type   | Description |
|---------------|--------|-------------|
| `entries`     | array  | `{tool, reasons}` per offered tool: its canonical name and every reason it is there — `declared` (named in `tools:`), `declared:<wildcard>` (matched `*`, `core.*`, `scm.*`, …), `default` (the agent has no `tools:`), `ambient:concerns`, `ambient:engagement`, `origin:injected` (a tool the launch site handed the run, e.g. an agentiflow's board tools) |
| `narrowed`    | array? | Connector tools the step's `actions:` removed. Omitted when empty |
| `unavailable` | array? | `{tool, missing}`: tools named exactly in `tools:` that this run can't serve, with the services it lacks. Each also gets a `tool_unavailable` notice. Omitted when empty |
| `skipped`     | array? | `{tool, missing}`: tools a wildcard or the default grant matched that this run can't serve. Not offered, no notice. Omitted when empty |

Services: `scm` (configured connectors), `agent_dispatcher` (sub-agent dispatch), `launcher`
(agentiflow units), `findings`, `engagement`, `coverage` (a `concerns:` block), `message_bus`,
`run_status`, `catalog`, `workflow_generator`, `netflow`.

```json
{"type":"tool_grant","data":{"entries":[{"tool":"read_file","reasons":["declared"]},{"tool":"findings.report","reasons":["ambient:concerns"]}],"unavailable":[{"tool":"board.post","missing":["message_bus"]}]}}
```

---

### `gate_requested`

Defined, but **not written by current rupu**: workflow approval gates are recorded on the
workflow run (`run.json`, `events.jsonl`), not in agent transcripts. Readers still parse and
render the event so older or hand-made files display.

| Field        | Type             | Description                                       |
|--------------|------------------|---------------------------------------------------|
| `gate_id`    | string           | Unique gate identifier within the workflow run    |
| `prompt`     | string           | Human-readable description of what to approve     |
| `decision`   | string, optional | `approved` or `rejected` (set when gate resolves) |
| `decided_by` | string, optional | Identity of the approver                          |

```json
{"type":"gate_requested","data":{"gate_id":"gate_01HXX","prompt":"Apply the proposed edit to parser.rs?"}}
```

---

### `user_message`

The user turn this run added to the conversation. Absent on a run that only continues a
`seed`. A note rupu sends back to the model mid-run (for example after an interruption, or a
recovery note) is also a `user_message`.

| Field     | Type   | Description |
|-----------|--------|-------------|
| `content` | string | The prompt text |

```json
{"type":"user_message","data":{"content":"Fix the failing parser test."}}
```

---

### `seed`

The conversation this run started from (a `rupu run --continue`, a session's earlier turns, a
workflow resume), stored once rather than re-embedded per turn. Exactly one of
`source_transcript` and `messages` is present.

| Field               | Type             | Description |
|---------------------|------------------|-------------|
| `message_count`     | u32              | Number of messages in the seed |
| `sha256`            | string           | Hash of the canonical seed JSON, so a reader can verify a reference chain |
| `source_transcript` | string, optional | A transcript whose replay rebuilds exactly this seed (chains can be several deep) |
| `messages`          | array, optional  | The seed messages inline (reasoning `raw` intact), when no source transcript vouches for it |

```json
{"type":"seed","data":{"message_count":6,"sha256":"9f2c…","source_transcript":"/repo/.rupu/transcripts/run_01HXX2A.jsonl"}}
```

---

### `thinking`

One model reasoning block, in its real position among the turn's other blocks.

| Field      | Type             | Description |
|------------|------------------|-------------|
| `text`     | string, optional | Human-readable reasoning. Absent when the reasoning was redacted or its display omitted |
| `provider` | string           | Provider that produced the block |
| `model`    | string           | Model that produced the block |
| `raw`      | object           | The provider's byte-exact block, signatures included. Kept for replay; never display it |

```json
{"type":"thinking","data":{"text":"The test expects usize…","provider":"anthropic","model":"claude-sonnet-4-6","raw":{"type":"thinking","thinking":"The test expects usize…","signature":"EqQB…"}}}
```

---

### `thinking_delta`

A streamed chunk of reasoning text, the reasoning counterpart of `assistant_delta`. The
following `thinking` event carries the whole block; after-the-fact readers can ignore deltas.

| Field     | Type   | Description |
|-----------|--------|-------------|
| `content` | string | One incremental reasoning chunk |

---

### `assistant_block`

A reply content block with no event of its own, written in its position among the turn's
`assistant_message`, `thinking` and `tool_call` events. Replay folds it back into the
assistant message at that position.

| Field       | Type           | Description |
|-------------|----------------|-------------|
| `block`     | object         | The provider-neutral content block: `{"type":"fallback","from_model":…,"to_model":…}` (an Anthropic server-side fallback boundary) or `{"type":"unknown","provider":…,"raw":…}` (a block rupu does not model, with the provider's raw payload) |
| `abandoned` | bool, optional | `true` for a block the API discarded at a mid-reply server-side fallback. It was never sent back to the model; replay skips it. Omitted when `false` |

```json
{"type":"assistant_block","data":{"block":{"type":"fallback","from_model":"claude-opus-5","to_model":"claude-sonnet-4-6"}}}
```

---

### `compaction`

Context compaction rewrote the conversation. The turns that follow were built from
`messages`.

| Field                 | Type   | Description |
|-----------------------|--------|-------------|
| `seq`                 | u32    | Compaction sequence number within the run |
| `summarized_messages` | u32    | How many messages were summarised |
| `backup_path`         | string | Where the pre-compaction conversation was saved |
| `messages`            | array  | The whole conversation after compaction |

The summariser call itself is billed under a `usage` line with `purpose: "compaction"`.

---

### `notice`

A runtime intervention worth showing that is not part of the conversation.

| Field     | Type   | Description |
|-----------|--------|-------------|
| `kind`    | string | `model_limits` (the run's input and output limits and where each came from, written at start and on a fallback hop), `model_limits_clamped`, `context_trim`, `provider_retry`, `server_side_fallback_disabled`, `permission_mode_degraded` (ask mode with no operator to ask) or `tool_unavailable` (a tool named in `tools:` that this run can't serve; one per tool, right after `tool_grant`). Expect new kinds; render an unknown one by its `message` |
| `message` | string | Human-readable text |

```json
{"type":"notice","data":{"kind":"provider_retry","message":"overloaded; retrying in 2s (attempt 1/3)"}}
```

---

### `net_flow`

One outbound network connection the run made, carrying a netflow `FlowRecord` (host, port,
scheme, method, query-stripped path, status, outcome, bytes, timing and a `fidelity` saying how
much of it was observed). Written as flows are recorded, so these lines can appear anywhere in
the file. The record is described in [netflow.md](netflow.md#flow-fields).

| Field  | Type       | Description |
|--------|------------|-------------|
| `flow` | FlowRecord | The flow |

```json
{"type":"net_flow","data":{"flow":{"id":"01JB7Q3V9F8M2K4N6P0R5T7W9X","ts":"2026-10-07T09:14:03Z","ctx":{"origin":{"kind":"provider","name":"anthropic"}},"fidelity":"http","method":"POST","scheme":"https","host":"api.anthropic.com","port":443,"path":"/v1/messages","status":200,"outcome":"ok","body_complete":false}}}
```

---

### `turn_end`

Emitted at the end of each agent turn, after all tool calls for that turn are complete.

| Field        | Type           | Description                                    |
|--------------|----------------|------------------------------------------------|
| `turn_idx`   | u32            | Matches the `turn_start` `turn_idx`            |
| `tokens_in`  | u64, optional  | Input tokens consumed this turn (if reported)  |
| `tokens_out` | u64, optional  | Output tokens produced this turn (if reported) |
| `stop_reason` | string, optional | The provider's wire stop value (e.g. `end_turn`, `max_output_tokens`) |
| `response_id` | string, optional | Provider response id, when non-empty         |
| `stop`       | StopRecord, optional | The typed stop (see below). Absent on transcripts written before the response-outcomes work |
| `discarded`  | bool, optional | `true` when the turn's content was thrown away (a refused, blocked or retried turn). Replay drops a discarded turn and `final_turn_text` ignores it. Omitted when `false` |

`StopRecord`: `reason` (string; the normalized stop such as `end_turn`, `max_tokens`, `refusal`,
`pause_turn`), `wire` (the provider's own words as JSON: provider name, raw value, extras),
and optional `refusal` and `served_by` (JSON; `served_by` names the model that answered when a
server-side fallback served the turn).

```json
{"type":"turn_end","data":{"turn_idx":0,"tokens_in":1024,"tokens_out":312}}
```

---

### `outcome`

A classified non-normal reply or provider error, written ahead of the turn's content. See
[response-outcomes.md](response-outcomes.md) for the classes and what each one does.

| Field      | Type          | Description                                    |
|------------|---------------|------------------------------------------------|
| `turn_idx` | u32           | The turn the outcome belongs to                |
| `outcome`  | OutcomeRecord | The record below                               |

`OutcomeRecord`:

| Field         | Type             | Description |
|---------------|------------------|-------------|
| `id`          | string           | Unique within the run (`oc_1`, `oc_2`, ...); a `recovery` points back to it |
| `class`       | string           | `pause_turn`, `max_tokens`, `context_window_exceeded`, `refusal`, `safety`, `malformed_tool_call`, `incomplete`, `empty_reply`, `unrecognized_stop`, `unreported_stop` or `provider_error`. A string, so a class a newer writer adds still parses |
| `severity`    | string           | `info`, `warning` or `error` |
| `title`       | string           | One line, e.g. `refused · cyber` |
| `detail`      | string, optional | Longer text: a refusal explanation, the tool call that was cut off, the provider's error message |
| `error_class` | string, optional | For `provider_error`: the normalized class (`rate_limited`, `overloaded`, `server`, `timeout`, `context_overflow`, `quota`, `not_found`, `policy`, `auth`, `permission`, `invalid_request`, `too_large`, `unrecognized`) |
| `wire`        | object, optional | The provider's own stop value or error body, kept verbatim. Omitted when null |

```json
{"type":"outcome","data":{"turn_idx":5,"outcome":{"id":"oc_2","class":"refusal","severity":"error","title":"refused · cyber","wire":{"provider":"anthropic","value":"refusal"}}}}
```

---

### `recovery`

One recovery action taken for an outcome. Written after its `outcome`, so the pair reads as a
timeline.

| Field        | Type             | Description |
|--------------|------------------|-------------|
| `outcome_id` | string           | The `outcome` this action answers |
| `rung`       | u8               | `0` in place, `1` fallback on the same provider, `2` fallback on another provider, `3` nothing left (the run fails) |
| `action`     | string           | `continued`, `retried`, `compacted`, `fell_back`, `served_by_fallback`, `skipped`, `asked`, `parked` or `failed`. A reader treats an action it does not know as `other` |
| `attempt`    | u32, optional    | Which attempt this is, against `budget` (rung 0) |
| `budget`     | u32, optional    | The rung-0 budget for this outcome's turn |
| `provider`   | string, optional | The fallback's provider (`fell_back`, `served_by_fallback`, `skipped`) |
| `model`      | string, optional | The fallback's model |
| `reason`     | string, optional | Why: the hint on `failed`, the build error on `skipped` |
| `merge_into_previous` | bool, optional | The next turn's assistant content joins the previous assistant message (a `pause_turn` continuation). Omitted when `false` |
| `continues_output`    | bool, optional | The next turn's text continues this turn's answer (a truncation continuation); `final_turn_text` joins across the boundary. Omitted when `false` |

`asked` and `parked` are reserved for operator-decided recovery and are not written yet.

```json
{"type":"recovery","data":{"outcome_id":"oc_2","rung":1,"action":"fell_back","provider":"anthropic","model":"claude-opus-5"}}
```

---

### Unknown event types

A line whose `type` this reader does not know is not an error. Readers surface it as an
`Unknown { tag, data }` event holding the raw `type` and `data`, and writing it back emits the
same `type` and `data`, so a newer rupu's transcript survives being read and copied by an older
one. rupu itself never writes an unknown event. A line that is not a JSON object, has no `type`,
or has a non-string `type` is corruption and still fails to parse. The CLI prints an unknown
event as `unrecognized event · <type>` and the control plane shows an "unrecognized event" block.

---

### `run_complete`

Emitted once at the very end of a run. Its presence signals a clean (non-aborted) run.

| Field          | Type             | Description                                     |
|----------------|------------------|-------------------------------------------------|
| `run_id`       | string           | Matches `run_start` and the JSONL filename      |
| `status`       | RunStatus        | `ok`, `error`, or `aborted`                     |
| `total_tokens` | u64              | Cumulative tokens across all turns              |
| `duration_ms`  | u64              | Total wall-clock duration, in ms                |
| `error`        | string, optional | Error description when `status` is `error`      |
| `outcome`      | OutcomeRecord, optional | The outcome the run failed on, when it ended because the recovery ladder ran out (see `outcome`) |

```json
{"type":"run_complete","data":{"run_id":"run_01HXX3Y7K8NQ","status":"ok","total_tokens":4096,"duration_ms":12340}}
```

---

## Aborted runs

A run that crashes or is killed mid-execution will leave a JSONL file with no `run_complete`
event. Readers must treat absence of `run_complete` as `RunStatus::Aborted`:

- `rupu transcript list` shows them with status `aborted`.
- `rupu transcript show <id>` renders all events that were written before the crash.
- Do not skip these files — the partial event log is valid and often diagnostic.

---

## Event order guarantees

Within a single run file the event ordering is:

```
run_start
notice?                     # model_limits
tool_grant                  # the tools offered, and why (W2)
notice*                     # tool_unavailable, one per named tool the run can't serve
seed?                       # a run seeded from earlier history
user_message?
(turn_start
   usage+                   # one per provider call; a compaction call adds one with purpose
   outcome?                 # a non-normal reply, ahead of the turn's content
   (thinking_delta* thinking | assistant_delta* assistant_message | assistant_block)*
   (tool_call  tool_result  file_edit?  command_run?  tool_audit)*
 turn_end
 recovery*
 notice*  compaction?)*
net_flow*                   # interleaved anywhere
run_complete
```

Reasoning, text, tool calls and other blocks appear in the order the model produced them. An
`outcome` follows the turn's `usage` and precedes its content. Its `recovery` events follow it,
usually after `turn_end`. A `notice` can appear wherever the runtime intervened; a hop to a
fallback model writes a `notice` of kind `model_limits` just before its `fell_back` recovery,
and a note sent back to the model is a `user_message`.

---

## Replay

A version 2 transcript is enough to rebuild the exact conversation the provider saw, and rupu
does so when it continues a run (`rupu run --continue <run-id>`, `rupu workflow resume`
continuing an interrupted step, a session's next turn). The rules:

- Start from the `seed`, if any. A `source_transcript` seed means "rebuild that transcript
  first"; its `sha256` must match what was rebuilt.
- Add the `user_message`. Per turn, fold `thinking` (its `raw` block), `assistant_message`,
  `tool_call` and `assistant_block` events, in file order, into one assistant message,
  skipping `abandoned` blocks; the turn's `tool_result`s become the next user message.
- Drop a turn with no `turn_end` (it was in flight) and a turn whose `turn_end` is
  `discarded`. A `recovery` with `merge_into_previous` joins the next turn's reply onto the
  previous assistant message.
- A `compaction` replaces everything so far with its `messages`.

A version 1 transcript rebuilds without reasoning blocks. Deltas, `usage`, `notice`,
`outcome`, `tool_grant`, `tool_audit`, `net_flow` and unknown events don't affect the
conversation.
