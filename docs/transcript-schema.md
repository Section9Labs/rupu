# Transcript Schema Reference

> Part of the rupu reference docs: [spec.md](spec.md) · [agent-format.md](agent-format.md) ·
> [workflow-format.md](workflow-format.md) · **transcript-schema.md**

---

## Overview

Every rupu run produces an immutable append-only log in JSONL format. Each line is one event.
The schema is the single source of truth for what happened in a run; there is no separate run
database in v0. `rupu transcript list` globs JSONL files and reads the `run_start` event from
each for metadata.

---

## File location

```
<transcripts-dir>/<run_id>.jsonl
```

Where `<transcripts-dir>` is:

- `<project>/.rupu/transcripts/` if the directory exists.
- `~/.rupu/transcripts/` otherwise (global fallback).

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

```json
{"type":"run_start","data":{"run_id":"run_01HXX3Y7K8NQ","workspace_id":"ws_01HXX…","agent":"fix-bug","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-05-01T17:00:00Z","mode":"ask"}}
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

Emitted once per LLM response, right after the provider call returns and before any
`assistant_delta`/`assistant_message` events for that response. `output_tokens` is already the
billable output figure — Gemini's reasoning/"thinking" tokens (reported separately by the
provider as `thoughtsTokenCount`) are folded in here upstream, so no separate reasoning-token
field exists on this event.

| Field           | Type             | Description                                                  |
|-----------------|------------------|----------------------------------------------------------------|
| `provider`      | string           | Provider name (e.g., `anthropic`)                             |
| `model`         | string           | Requested model id (used for pricing attribution)              |
| `served_model`  | string, optional | Actual model the provider served, if it differs from `model`   |
| `input_tokens`  | u32              | Input tokens for this response                                 |
| `output_tokens` | u32              | Billable output tokens (includes any reasoning tokens)         |
| `cached_tokens` | u32              | Cached-input tokens included in `input_tokens` (default `0`)    |

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
| `thinking` | string, optional | Extended thinking text (omitted if not present) |

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
workflow step dispatches through the in-process MCP `ToolDispatcher`. `kind`
is the real MCP catalog tool name (e.g. `issues.create`, not a bespoke verb);
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

Per-catalog-tool-call audit trail for a workflow step's `actions:` enforcement. Emitted from two
choke points: the agent runtime's `on_tool_call` hook (wrapped by `rupu-orchestrator::step_factory`)
for agent-driven MCP calls, and `execute_action_step` for `action:`-node calls — for an action-node
call it is written immediately after that call's `action_emitted` line, covering the same call.
Never confuse `tool_audit` with `action_emitted`: `tool_audit` is the general catalog-call audit
line, and it is what the CP transcript panel renders a badge from.

| Field        | Type   | Description                                                                                     |
|--------------|--------|---------------------------------------------------------------------------------------------------|
| `tool`       | string | The MCP catalog tool name (e.g. `issues.create`)                                                  |
| `declared`   | bool   | Whether `tool` appears in the step's `actions:` allowlist. `false` both when `actions:` is empty/absent (unrestricted — not a violation) and when `actions:` is non-empty but doesn't name this tool — use `restricted` to tell those apart |
| `granted`    | bool   | Whether `tool` is covered by the agent's `tools:` grant, evaluated before any `actions:` narrowing. Always `true` for an `action:` node (no agent-grant concept applies there) |
| `blocked`    | bool   | Whether the call was actually denied — either narrowed out of the agent's roster before reaching the registry, or denied by the MCP mode/permission gate |
| `restricted` | bool   | Whether the step declared a non-empty `actions:` allowlist at all (disambiguates `declared: false`) |

```json
{"type":"tool_audit","data":{"tool":"issues.create","declared":true,"granted":true,"blocked":false,"restricted":true}}
```

---

### `gate_requested`

Reserved for Slice B workflow approval gates. **Not emitted in v0.** The schema is defined and
stable so Slice B can emit these events without a schema migration.

| Field        | Type             | Description                                       |
|--------------|------------------|---------------------------------------------------|
| `gate_id`    | string           | Unique gate identifier within the workflow run    |
| `prompt`     | string           | Human-readable description of what to approve     |
| `decision`   | string, optional | `approved` or `rejected` (set when gate resolves) |
| `decided_by` | string, optional | Identity of the approver                          |

```json
{"type":"gate_requested","data":{"gate_id":"gate_01HXX","prompt":"Apply the proposed edit to parser.rs?","decision":null,"decided_by":null}}
```

---

### `turn_end`

Emitted at the end of each agent turn, after all tool calls for that turn are complete.

| Field        | Type           | Description                                    |
|--------------|----------------|------------------------------------------------|
| `turn_idx`   | u32            | Matches the `turn_start` `turn_idx`            |
| `tokens_in`  | u64, optional  | Input tokens consumed this turn (if reported)  |
| `tokens_out` | u64, optional  | Output tokens produced this turn (if reported) |

```json
{"type":"turn_end","data":{"turn_idx":0,"tokens_in":1024,"tokens_out":312}}
```

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
  (turn_start
    usage
    assistant_delta*
    assistant_message*
    (tool_call  tool_result  file_edit?  command_run?  action_emitted?  tool_audit?)*
  turn_end)*
run_complete
```

`gate_requested` events (Slice B) appear between `turn_end` and the next `turn_start` when a
gate interrupts the loop.
