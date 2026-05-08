# rupu — Architecture Reference (Slice A as built)

> Full reference docs: [agent-format.md](agent-format.md) · [workflow-format.md](workflow-format.md) · [transcript-schema.md](transcript-schema.md)

---

## As-built notes

Deviations from the brainstorm spec (`docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`)
discovered during implementation:

- **MSRV 1.77** — spec said 1.75; raised to 1.77 to use `std::io::IsTerminal` without a shim.
- **phi-providers Message/Event constructors** — lifted `rupu-providers` API has small naming
  differences from what plan documents assumed (Plan 2 Task 7). When wiring a new provider,
  grep `crates/rupu-providers/src/anthropic.rs` first; do not guess.
- **Transcript events carry no per-event run_id/workspace_id** — the filename `<run_id>.jsonl`
  is the canonical run identifier. Only `run_start` and `run_complete` carry `run_id`.
  Slice C streaming wraps each event in a `{run_id, workspace_id, event}` transport envelope
  rather than inflating every intermediate event payload.
- **`default_branch` renamed `initial_branch`** — the workspace record field was renamed for
  clarity (it records the branch at first registration, not necessarily the repo default branch).
  The TOML alias `default_branch` is retained for backward compatibility.
- **Sample agents + workflows at `<repo>/.rupu/`** — the original spec described embedding them
  via `include_str!`; the shipped approach commits them to `.rupu/agents/` and `.rupu/workflows/`
  in the rupu repository itself. They are discovered by rupu's normal project-discovery logic
  when running `rupu` inside the rupu checkout, and serve as copy-paste templates for new users.
- **Provider wiring expanded beyond the original Slice A notes** — current `rupu` ships working Anthropic, OpenAI, Gemini, and Copilot provider integrations. Prefer the provider-specific docs for current auth and model details.

---

## Vision

`rupu` is a CLI for orchestrating coding agents against local repositories. A developer describes
a task as a `.md` agent file (system prompt + frontmatter) or a `.yaml` workflow (a linear
sequence of agent steps), then invokes `rupu run` or `rupu workflow run`. The agent loop drives
an LLM through tool calls — bash, file read/write/edit, grep, glob — logging every event to an
immutable JSONL transcript. The action protocol contract is present from day one so Slice B can
wire real effects (open PR, post comment) without schema changes. Slice A is local-only; no
SaaS control plane and remote sandboxing are still out of scope here; SCM and issue integrations are now part of the shipped local CLI.

---

## Slice A scope

### Shipped

- Single Rust binary `rupu` (crate `rupu-cli`).
- Provider stack lifted from phi-cell; Anthropic, OpenAI, Gemini, and Copilot are wired in the local CLI.
- Agent file format: `.md` + YAML frontmatter, `#[deny_unknown_fields]`.
- Six tools: `bash`, `read_file`, `write_file`, `edit_file`, `grep`, `glob`.
- Permission modes: `ask`, `bypass`, `readonly`.
- Workflow runner with sequential steps plus `for_each`, `parallel`, `panel`, `when`, approval gates, and trigger-aware context.
- JSONL transcript schema with 11 event types.
- Two-tier config: global `~/.rupu/config.toml` + project `<repo>/.rupu/config.toml`.
- Credential storage: OS keychain via `keyring` crate, chmod-600 JSON fallback.
- `rupu agent list|show`, `rupu workflow list|show|run`, `rupu transcript list|show`,
  `rupu config get|set`, `rupu auth login|logout|status`.
- Curated starter samples in `crates/rupu-cli/templates/` plus richer repo-local examples under `examples/`.
- `cargo install`-able; tag-triggered GitHub Releases (macOS arm64/x86_64, Linux x86_64/arm64).

### Deferred to Slice B+

- Bitbucket, Linear, and Jira connectors.
- General DAG scheduling beyond the current sequential-step engine.
- Workflow action effects (open PR, post comment). v0 logs `action_emitted` only.
- Workflow action effects beyond today's transcripted action-protocol validation.
- Transcript compaction, resume from aborted run, concurrent-run locking.
- SaaS control plane, remote runs, OAuth flows.
- Sandbox / microVM / session save-restore.
- Native desktop app, billing, marketplace.

---

## Filesystem layout

### Global (`~/.rupu/`)

```
~/.rupu/
  config.toml       # provider defaults, model defaults, permission mode, log level
  auth.json         # API keys / tokens (chmod 600 fallback; keychain preferred)
  agents/           # global agent library (*.md)
  workflows/        # global workflow library (*.yaml)
  workspaces/       # one <id>.toml per discovered workspace
  transcripts/      # default JSONL transcript archive
  cache/            # model catalog cache, auth-backend probe cache, crash logs
```

### Project (`<repo>/.rupu/`)

```
<repo>/.rupu/
  config.toml       # project-specific overrides (deep-merged over global)
  agents/           # project-local agents (shadow globals by name)
  workflows/        # project-local workflows
  transcripts/      # if present, runs in this project write here instead of global
  .gitignore        # ignores transcripts/ and cache/; commits agents/, workflows/, config.toml
```

### Resolution rules

1. **Agents & workflows — name-based shadow.** A project file with the same `name:` value as a
   global file wins entirely. No frontmatter merging. `rupu agent list` labels each entry
   `(global)` or `(project)`.
2. **Config — deep merge, project wins.** TOML merged key-by-key. Arrays in the project config
   replace the global array (no concatenation; concatenation makes "remove an item" impossible).
3. **Auth — global only.** `auth.json` is never read from a project directory.
4. **Discovery** — walk up from `$PWD` looking for the first `.rupu/` directory (like `git`
   does for `.git`). If none found, global-only mode.
5. **Workspace identity** — keyed by canonicalized path. First `rupu run` in a directory upserts
   `~/.rupu/workspaces/<id>.toml` with a ULID id.

### Workspace record (`~/.rupu/workspaces/<id>.toml`)

```toml
id             = "ws_01HXXX..."
path           = "/Users/matt/Code/Oracle/rupu"
repo_remote    = "git@github.com:section9labs/rupu.git"   # optional
initial_branch = "main"                                    # branch at first registration
created_at     = "2026-05-01T17:00:00Z"
last_run_at    = "2026-05-01T17:42:00Z"                   # optional; updated each run
```

Note: the TOML alias `default_branch` is accepted for backward compatibility.

---

## Agent file format

Full reference: [agent-format.md](agent-format.md)

Agent files live at `<dir>/agents/<name>.md` where `<dir>` is `~/.rupu` or `<project>/.rupu`.
The file is a Markdown document with YAML frontmatter. The frontmatter is validated with
`#[serde(deny_unknown_fields)]` — typos like `permision_mode` produce a parse error immediately.

Key frontmatter fields:

| Field            | Type           | Required | Default            |
|------------------|----------------|----------|--------------------|
| `name`           | string         | yes      | —                  |
| `description`    | string         | no       | —                  |
| `provider`       | string         | no       | `anthropic`        |
| `model`          | string         | no       | `claude-sonnet-4-6`|
| `tools`          | array\<string\>| no       | all six            |
| `maxTurns`       | u32            | no       | `50`               |
| `permissionMode` | string         | no       | `ask`              |

The body (everything after the closing `---`) is used verbatim as the LLM system prompt.

---

## Tool surface

Six tools in v0. No tool plugins or external tools.

| Tool         | Description                                                     | Default timeout |
|--------------|-----------------------------------------------------------------|-----------------|
| `bash`       | Execute a shell command in workspace cwd                        | 120 s           |
| `read_file`  | Read full file contents (line-numbered)                         | 30 s            |
| `write_file` | Create or overwrite a file                                      | 30 s            |
| `edit_file`  | Exact-match replacement; error if string not found              | 30 s            |
| `grep`       | Search across workspace (ripgrep-backed)                        | 30 s            |
| `glob`       | File pattern matching                                           | 30 s            |

`bash` env is controlled: PATH, HOME, USER, TERM, LANG, plus a per-workspace allow-list. No env
var injection from agent input. `bash` timeout sends SIGTERM then SIGKILL; yields a
`tool_result` with `error: "timeout"`.

Git operations use `bash git ...` in v0. Dedicated git tools land in Slice B.

---

## Permission model

| Mode       | bash   | write_file | edit_file | read_file | grep  | glob  |
|------------|--------|------------|-----------|-----------|-------|-------|
| `readonly` | deny   | deny       | deny      | allow     | allow | allow |
| `ask`      | prompt | prompt     | prompt    | allow     | allow | allow |
| `bypass`   | allow  | allow      | allow     | allow     | allow | allow |

**Mode resolution** (highest precedence first): CLI flag → agent frontmatter →
project config → global config → default (`ask`).

**Non-TTY + `ask` aborts.** When stdin is not a terminal (CI, daemon, pipe), running in `ask`
mode is rejected before the first tool call. Silently degrading to `bypass` is how accidents
happen in CI.

**Denied tool calls** return a `tool_result` with `error: "permission_denied"`. The agent
receives this and can adapt (e.g., switch to read-only investigation).

**Prompt UX** (when `ask`): shows tool name + full input (truncated to ~200 chars/field with
a `more` option) + workspace path. Choices: `[y]es` / `[n]o` / `[a]lways for this tool this run` /
`[s]top run`. There is no "always for all tools" option.

---

## CLI surface

```
rupu run <agent> [prompt]           # one-shot agent run
rupu agent list                     # list available agents (global + project)
rupu agent show <name>              # display agent frontmatter + system prompt
rupu workflow run <name> [--input KEY=VALUE ...]
rupu workflow list
rupu workflow show <name>
rupu transcript list
rupu transcript show <id>
rupu config get <key>
rupu config set <key> <value>
rupu auth login  [--provider <p>] [--key <k>]
rupu auth logout [--provider <p>]
rupu auth status
```

Slots reserved for later slices: `daemon`, `jobs`, `node`, `enroll`, `workspace`.

---

## Transcript schema

Full reference: [transcript-schema.md](transcript-schema.md)

JSONL on disk (`<run_id>.jsonl`). One event per line, tagged JSON:
`{"type": "<variant>", "data": {...}}`. The filename is the canonical run identifier; individual
events do not repeat `run_id` (only `run_start` and `run_complete` carry it).

11 event types (v0):

`run_start`, `turn_start`, `assistant_message`, `tool_call`, `tool_result`, `file_edit`,
`command_run`, `action_emitted`, `gate_requested` (reserved; not emitted in v0), `turn_end`,
`run_complete`.

Files with no `run_complete` event represent aborted (crashed) runs. Readers treat them as
`RunStatus::Aborted` rather than skipping the file.

---

## Orchestrator (linear workflow runner)

Full reference: [workflow-format.md](workflow-format.md)

YAML, one file per workflow at `<dir>/workflows/<name>.yaml`. Validated at parse time.

Current `rupu` workflows run steps in declaration order, but each step can also use `when:`, `for_each:`, `parallel:`, `panel:`, and `approval:`. For the up-to-date schema, rely on `docs/workflow-format.md` rather than the original Slice A narrative below.

Engine behavior per step:

1. Render `prompt:` as a minijinja template with `inputs.*` and `steps.<id>.output` variables.
2. Spawn an agent run with the rendered prompt.
3. For each `action_emitted` event, check the step's `actions:` allowlist. Log `applied:
   true|false` in the transcript. v0 does not execute effects — Slice B wires them.
4. On step failure, abort the workflow.
5. After all steps, emit `run_complete` with cumulative token counts.

---

## Agent loop sequence

```
rupu run my-agent "fix the failing test"
        │
        ▼ rupu-cli: parse argv, resolve workspace + config, build provider
        │
        ▼ rupu-workspace: walk up from $PWD for .rupu/; canonicalize; upsert workspace record
        │
        ▼ rupu-config: layer global config + project config + agent frontmatter + CLI flags
        │
        ▼ rupu-agent: load agent file (project shadows global by name), parse frontmatter + body
        │
        ▼ rupu-auth: resolve credential for provider (keychain → auth.json fallback)
        │
        ▼ rupu-transcript: open <transcripts>/<run_id>.jsonl, write run_start event
        │
        ▼ provider loop (rupu-providers + rupu-agent):
              1. send messages → stream response
              2. on tool_use → rupu-tools dispatch (gated by permission mode)
              3. on text → emit assistant_message event
              4. append tool_result to messages
              5. derive file_edit / command_run / action_emitted events
              6. repeat until end_turn signal or maxTurns reached
        │
        ▼ rupu-transcript: write run_complete, close file
```

---

## Authentication

- **Primary storage:** `keyring` crate — macOS Keychain on macOS, Linux Secret Service (D-Bus)
  on Linux.
- **Fallback:** `~/.rupu/auth.json` at mode 0600. Used when the keychain is unavailable (headless
  server, no D-Bus). A one-time warning is printed. Mode bits checked on every read.
- **Probe cache:** `~/.rupu/cache/auth-backend.json` records which backend was chosen so rupu
  does not re-probe on every invocation. Invalidated on `rupu auth login` and `--probe-auth`.
- **`rupu auth login`** — reads the API key from stdin or `--key <K>`. Stores via the chosen
  backend. OAuth flows (Copilot, Gemini) are deferred.
- **`rupu auth status`** — shows configured providers + storage backend. Never prints credentials.

---

## Error handling

Four buckets:

1. **User errors** — bad agent name, missing config key, malformed YAML. Exit non-zero with a
   single clear message, no stack trace. `thiserror`-typed in libraries; formatted at the CLI
   boundary.
2. **Provider errors** — rate limit, network, auth failure. Transient errors (rate limit, 5xx,
   network) retry with exponential backoff. Non-transient (401, 400) fail fast. Retries logged as
   `tool_result` events with `error: "provider_retry"`.
3. **Tool errors** — bash exit non-zero, file not found, edit did not match. These are not
   agent-loop errors; they surface to the agent as `tool_result` with failure detail. The agent
   decides the next step.
4. **Internal errors** — bugs, panics. Caught at the CLI boundary, written to
   `~/.rupu/cache/crash-<rfc3339>.log` with full backtrace. Exit code 2, brief message pointing
   at the log. `RUST_BACKTRACE=1` is set internally at startup.

---

## Failure modes

- **Half-written transcripts** — mid-run crash leaves a JSONL with no `run_complete`. Readers
  treat absence of `run_complete` as `RunStatus::Aborted`. `rupu transcript list` shows them
  with status `aborted`.
- **Concurrent runs in the same workspace** — allowed in v0. Each run gets a unique `run_id`;
  JSONL files do not collide. Working-directory-level conflicts are a user concern in v0; Slice C
  sandboxing sidesteps this.
- **Provider context overflow** — abort with `run_complete { status: "error", error:
  "context_overflow" }`. v0 does not auto-summarize or truncate. Compaction belongs in a later
  slice.
- **Tool timeouts** — SIGTERM then SIGKILL on the bash process; `tool_result { error:
  "timeout" }` to the agent.

---

## Security posture (v0)

`bypass` mode shells out arbitrary commands. The runtime minimizes accidents but cannot prevent
deliberate agent misbehavior:

- `auth.json` always 0600; rupu warns loudly on read if permissions are wrong.
- `bash` runs in workspace cwd (never an agent-controlled path) with a controlled environment.
- No env var injection from agent input.
- README has an explicit "agents are code that can do anything you can do — review what you run"
  section. No false safety promises in `bypass`.

---

## Build and release

- **Toolchain:** Rust stable, MSRV 1.77 (pinned in workspace `Cargo.toml`).
- **Lints:** `#![deny(clippy::all)]` workspace-wide; `clippy::pedantic` opt-in per crate.
- **Format:** `rustfmt`; CI fails on diff.
- **PR checks:** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`,
  `cargo build --release` on macOS arm64 + Linux x86_64.
- **Releases:** tag-triggered (`v*`). Build matrix: macOS arm64, macOS x86_64, Linux x86_64,
  Linux arm64. Strip + tar.gz + SHA256 checksum. Upload via `softprops/action-gh-release`.
- **Distribution:** GitHub Releases + `cargo install --git https://github.com/section9labs/rupu`.
  No Homebrew tap or package repositories in v0.

---

## Telemetry

None in v0. No phone-home, no anonymous metrics, no first-run prompt. Adding telemetry in a
future slice requires explicit user opt-in.

---

## Repository

Public: `github.com/section9labs/rupu`.
