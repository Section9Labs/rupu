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

### `assistant_message`

Emitted when the LLM produces a text response (may be emitted multiple times per turn if the
provider streams partial results, but rupu emits one event per complete message block).

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

Emitted when the agent emits an action-protocol verb. In v0, no effect is executed; the event
records the allowlist check result only.

| Field     | Type             | Description                                          |
|-----------|------------------|------------------------------------------------------|
| `kind`    | string           | Action verb (e.g., `propose_edit`, `open_pr`)        |
| `payload` | object           | Action data                                          |
| `allowed` | bool             | Whether the action was on the step's `actions:` list |
| `applied` | bool             | Whether the effect was executed (always `false` in v0) |
| `reason`  | string, optional | Explanation when `allowed` or `applied` is `false`   |

```json
{"type":"action_emitted","data":{"kind":"log_finding","payload":{"message":"null pointer in parser.rs:142"},"allowed":true,"applied":false,"reason":"not wired in v0"}}
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
    assistant_message*
    (tool_call  tool_result  file_edit?  command_run?  action_emitted*)*
  turn_end)*
run_complete
```

`gate_requested` events (Slice B) appear between `turn_end` and the next `turn_start` when a
gate interrupts the loop.
