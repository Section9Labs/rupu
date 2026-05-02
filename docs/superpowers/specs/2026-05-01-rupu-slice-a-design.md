# rupu — Slice A Design

**Date:** 2026-05-01
**Status:** Draft (pending implementation plan)
**Slice:** A of D (see "Roadmap context" below)

## Vision

`rupu` is "Okesu, but for code development in an agentic engineering world": a SaaS + native desktop app + CLI for orchestrating coding agents across repositories from any SCM, triggered by issues from any tracker, with sandboxed sessions that can be saved and resumed.

The full vision spans many independent subsystems. This document specifies **Slice A only** — the foundation everything else builds on.

## Roadmap context

The full project is sequenced as four slices, each with its own brainstorm → spec → plan → implementation cycle:

- **Slice A (this spec):** Single Rust CLI binary. Provider stack + agent runtime + linear workflow orchestrator + action protocol contract. Local-only. No SaaS, no SCM connectors, no sandbox.
- **Slice B (later):** SCM connectors (GitHub/GitLab/Bitbucket), issue-tracker triggers, full DAG workflow engine (fan-out, gates, conditionals), plugin extraction.
- **Slice C (later):** Control plane (Go web app + React UI), auth (SSO + API keys), sandboxed remote runs (containers/microVMs), session save/restore, workspace remote sync.
- **Slice D (later):** Billing/metering, native desktop app (Rust), marketplace polish.

Slice A's exit criterion is "another developer can install and use it on their own repos" — `cargo install`-able, GitHub Release binaries available, README good enough for first-run, 4-5 example agents shipped.

## Heritage

Slice A draws directly from two existing projects in this org:

- **Okesu** (`../Okesu`): Go-based "single binary CLI + Go control plane" for security ops. Provides the architectural shape: hexagonal ports/adapters, agent definitions as `.md` files with YAML frontmatter, orchestrations as YAML DAGs with an action protocol that lets the engine validate and apply state mutations on the agent's behalf, JSONL transcripts that are invariant regardless of execution location.
- **phi-cell** (`~/Code/phi-cell`): Rust workspace implementing a "universal cellular agent." The `crates/phi-providers` crate (Anthropic, OpenAI, GitHub Copilot, local providers + auth, SSE, model catalog, routing history) is **lifted near-verbatim** into `crates/rupu-providers`. This is the single largest piece of pre-existing code we reuse and saves the equivalent of weeks of work on multi-provider plumbing.

## Goals

- Single Rust binary `rupu` that runs coding agents one-shot or in linear workflows against the local working directory.
- Multi-provider support (Anthropic, OpenAI, Copilot, local) via the lifted provider stack.
- Agent file format compatible with Okesu/Claude conventions (`.md` + YAML frontmatter).
- JSONL transcripts on disk in a rupu-native normalized event schema, invariant across providers.
- Action protocol contract present from day one — even though v0 only logs actions, the shape locks downstream consumers.
- Two-tier config (global `~/.rupu/`, project `<repo>/.rupu/`).
- Cross-platform credential storage (OS keychain with chmod-600 fallback).
- Linear workflow runner; full DAG deferred to Slice B but the engine surface is forward-compatible.
- `cargo install`-able and shipped as prebuilt binaries on GitHub Releases (macOS arm64/x86_64, Linux x86_64/arm64).

## Non-goals (Slice A)

- No SCM connectors (GitHub/GitLab/Bitbucket integration). Local-only via `bash`.
- No issue-tracker triggers.
- No fan-out, parallel, conditional, or gated workflows. Linear `step → step → step` only.
- No SaaS, control plane, remote runs, or auth beyond local API keys.
- No sandbox, microVM, or session restore.
- No billing or metering.
- No native desktop app.
- No plugin system. Connectors land in Slice B as in-tree code; extraction comes later.
- No transcript compaction, no resume-from-aborted-run, no concurrent-run locking.
- No telemetry, analytics, or phone-home.

## Architecture

### Repository layout

```
rupu/
  Cargo.toml                    # workspace root
  crates/
    rupu-cli/                   # the `rupu` binary; clap subcommands; thin wiring only
    rupu-agent/                 # agent file parser; agent loop; permission gating
    rupu-tools/                 # tool harness: bash, read_file, write_file, edit_file, grep, glob
    rupu-providers/             # LIFTED from phi-cell; Anthropic/OpenAI/Copilot/local + auth/SSE
    rupu-orchestrator/          # workflow parser; linear sequential runner; action protocol
    rupu-transcript/            # JSONL writer + reader; rupu-native event schema
    rupu-workspace/             # workspace discovery; ~/.rupu/workspaces/<id>.toml read/write
    rupu-config/                # config layering (global + project), TOML parse, deep-merge
    rupu-auth/                  # `keyring` crate with chmod-600 fallback
  agents/                       # default agent library (embedded via include_str!)
  workflows/                    # default workflow library (embedded)
  docs/
    superpowers/specs/          # design docs (this file)
    spec.md                     # source-of-truth architecture
    agent-format.md             # agent frontmatter reference
    workflow-format.md          # linear workflow YAML reference
    transcript-schema.md        # event schema reference
  .github/workflows/release.yml # CI + GitHub Releases
  README.md
  CLAUDE.md                     # project memory for agents working on rupu
```

### Architectural rules

1. **Hexagonal separation.** `rupu-providers`, `rupu-tools`, `rupu-auth` define traits (ports). `rupu-agent` knows only the traits, never concrete impls. This is what enables Slice C (CLI talks to remote orchestrator), Slice D (sandboxed tools), and unit testing via mocks.
2. **`rupu-cli` is thin.** Every subcommand is ~20-50 lines: arg parsing + delegation. No business logic. Slice C adds `rupu run --remote` as a flag-flip rather than a refactor.
3. **Workspace dependencies only.** Versions pinned in root `Cargo.toml`. `#![deny(clippy::all)]` workspace-wide.
4. **Crate split rationale:** `rupu-config` and `rupu-workspace` are deliberately separate — config is read-mostly, workspace records are mutated at runtime. Different lifecycle, different ownership.

## Filesystem layout (runtime)

### Global (`~/.rupu/`)

```
~/.rupu/
  config.toml         # provider defaults, model defaults, permission mode, log level
  auth.json           # API keys / refresh tokens (fallback storage; chmod 600)
  agents/             # global agent library (*.md)
  workflows/          # global workflow library (*.yaml)
  workspaces/         # one TOML file per workspace (id, path, repo_remote, etc.)
  transcripts/        # default JSONL transcript archive
  cache/              # model catalog cache, schema cache, crash logs
```

### Project (`<repo>/.rupu/`)

```
<repo>/.rupu/
  config.toml         # project-specific overrides + additions
  agents/             # project-local agents (override globals by name)
  workflows/          # project-local workflows
  transcripts/        # if present, runs in this project go here instead of global
  .gitignore          # ignores transcripts/ and cache/; commits agents/, workflows/, config.toml
```

### Resolution rules (locked)

1. **Agents & workflows: name-based override.** Project file with same `name:` shadows global. `rupu agent list` shows both with `(global)`/`(project)` chip. No frontmatter merging.
2. **Config: deep merge, project wins.** TOML merged key-by-key. Arrays in project **replace** globals (do not concatenate; concatenation makes "remove an item" impossible).
3. **Auth: global only.** `auth.json` lives only in `~/.rupu/`. Never read from project dir — too easy to leak credentials into a repo.
4. **Discovery:** walk up from `$PWD` looking for the first `.rupu/` directory (like `git` does for `.git`). That's the project root. If none found, global-only.
5. **Workspace identity:** keyed by canonicalized path. First `rupu run` in a directory upserts `~/.rupu/workspaces/<id>.toml` with a ULID id.

### Workspace record

`~/.rupu/workspaces/<workspace-id>.toml`:

```toml
id              = "ws_01HXXX..."
path            = "/Users/matt/Code/Oracle/rupu"
repo_remote     = "git@github.com:section9labs/rupu.git"   # if detectable, else null
default_branch  = "main"
created_at      = "2026-05-01T17:00:00Z"
last_run_at     = "2026-05-01T17:42:00Z"
```

Every transcript event carries `workspace_id` so Slice C/D plug in without retrofit.

## Agent file format

Compatible with Okesu/Claude conventions. `.md` file with YAML frontmatter; body is the system prompt.

```yaml
---
name: fix-bug
description: Investigate a failing test and propose a fix.
provider: claude
model: claude-sonnet-4-6
tools: [bash, read_file, write_file, edit_file, grep, glob]
maxTurns: 30
permissionMode: ask    # ask | bypass | readonly
---

You are a senior engineer. When given a failing test, you...
```

Code-dev-specific frontmatter fields can be added later as **optional** keys so Okesu agents stay portable both ways.

## Tool surface (v0)

Six tools, no more:

- `bash` — execute shell command in workspace cwd; controlled environment (PATH, HOME, USER, TERM, LANG + per-workspace allow-list); default 120s timeout, configurable per-call.
- `read_file` — read full file contents (line-numbered output, like Claude Code's Read). No byte-range support in v0.
- `write_file` — create or overwrite file.
- `edit_file` — exact-match replacement; failure surfaced as `tool_result` with error.
- `grep` — search across workspace (ripgrep-backed).
- `glob` — file pattern matching.

Default timeouts: `bash` 120s, all others 30s. Timeout produces `tool_result { error: "timeout", killed: true }`; bash process is SIGTERM then SIGKILL.

Git operations are deferred to dedicated tools in Slice B; v0 agents use `bash git ...`.

## Permission model

| Mode | bash | write_file | edit_file | read_file | grep | glob |
|---|---|---|---|---|---|---|
| `readonly` | deny | deny | deny | allow | allow | allow |
| `ask` | prompt | prompt | prompt | allow | allow | allow |
| `bypass` | allow | allow | allow | allow | allow | allow |

- **Mode resolution:** CLI flag > agent frontmatter > project config > global config > default (`ask`).
- **Non-TTY + `ask` = abort.** Detect absence of controlling terminal (daemon, CI, piped). Abort the run with a clear error before the first prompt. Silently degrading to `bypass` is how `rm -rf` accidents happen in CI.
- **Denied tool calls** produce a `tool_result` with `error: "permission_denied"`. The agent sees it and can adapt (e.g., switch to investigation-only).
- **Prompt UX:** shows tool name, full input (truncated to ~200 chars per field with `more` option), workspace path. Decisions: `[y]es` / `[n]o` / `[a]lways for this tool this run` / `[s]top run`. No "always for all tools" — too dangerous.

## CLI surface

Verb-first flat (Okesu-style).

```
rupu run <agent> [prompt]            # one-shot agent run
rupu agent list | show <name>        # agent management
rupu workflow run <name> [args]      # linear DAG run
rupu workflow list | show <name>
rupu transcript list | show <id>     # transcript browse
rupu config get|set <key> [value]
rupu auth login|logout|status
```

Slots reserved for later slices: `daemon`, `jobs`, `node`, `enroll`, `workspace`.

## Transcript schema (rupu-native, normalized)

JSONL on disk. Single source of truth for what happened in a run. No separate run database in v0; `rupu transcript list` globs JSONL files and reads `run_start` events for metadata.

### v0 event types

| Event | Fields |
|---|---|
| `run_start` | run_id, workspace_id, agent, provider, model, started_at, mode |
| `turn_start` | turn_idx |
| `assistant_message` | content, thinking? |
| `tool_call` | call_id, tool, input |
| `tool_result` | call_id, output, error?, duration_ms |
| `file_edit` | path, kind (create / modify / delete), diff |
| `command_run` | argv, cwd, exit_code, stdout_bytes, stderr_bytes |
| `action_emitted` | kind, payload, allowed, applied, reason? |
| `gate_requested` | gate_id, prompt, decision?, decided_by? |
| `turn_end` | turn_idx, tokens_in, tokens_out |
| `run_complete` | run_id, status (ok / error / aborted), total_tokens, duration_ms |

### Schema rules (locked)

- `file_edit` and `command_run` are **derived events** — the runtime emits both `tool_result` AND a derived event when the tool kind is known. Consumers can index on derived events without parsing tool inputs.
- `action_emitted` carries the action-protocol verb even in v0, where the only effects are logged. Schema-stable from day one so Slice B can wire effects without renaming events.
- `gate_requested` reserved for Slice B (workflow approval gates); not emitted in v0 but defined in the schema.
- Every event carries `workspace_id` and `run_id`.

### Aborted runs

Crashes mid-run leave a JSONL with no `run_complete`. Readers treat absence of `run_complete` as `aborted`, do not skip the file. `rupu transcript list` shows them with status `aborted`.

## Orchestrator (linear workflow runner)

YAML, file-per-workflow, schema-validated at parse time. v0 honors only a linear `steps:` list — no `parallel:`, no `when:`, no `gates:` (these are reserved keywords that produce a parse error in v0 with a clear "deferred to Slice B" message).

```yaml
name: investigate-then-fix
description: Investigate a bug then propose a fix.
steps:
  - id: investigate
    agent: investigator
    actions:                    # action protocol allowlist
      - log_finding
    prompt: |
      Investigate the bug described in: {{ inputs.prompt }}

  - id: propose
    agent: fixer
    actions:
      - propose_edit
    prompt: |
      Based on the investigation:
      {{ steps.investigate.output }}

      Propose a minimal fix.
```

### Engine behavior (v0)

1. Parse YAML; reject unknown keys.
2. For each step in order:
   - Render `prompt:` template (minijinja) with `inputs.*` and prior `steps.<id>.output`.
   - Spawn agent run with the rendered prompt.
   - For each `action_emitted` event, validate `kind` against the step's `actions:` allowlist. Log `applied: true | false, reason: ...` in the transcript. v0 does not actually *do* anything on applied actions — Slice B wires effects.
   - On step failure (provider error, agent abort), abort the workflow.
3. Emit `run_complete` for the workflow itself with cumulative token counts.

## Agent loop (single-run hot path)

```
rupu run my-agent "fix the failing test"
        │
        ▼
rupu-cli parses argv, delegates to rupu-agent::run(spec)
        │
        ▼
rupu-workspace: discover (.rupu/ walk-up + canonicalize $PWD), upsert workspace record.
        │
        ▼
rupu-config: layer global + project + agent frontmatter + CLI flags.
        │
        ▼
rupu-agent: load agent file (project shadows global by name), parse YAML, body = system prompt.
        │
        ▼
rupu-auth: resolve credential for provider (keychain → fallback).
        │
        ▼
rupu-transcript: open <transcripts>/<run-id>.jsonl, write run_start.
        │
        ▼
rupu-providers: instantiate provider client. Loop:
                  1. send messages → stream response
                  2. on tool_use → rupu-tools dispatch (gated by permission mode)
                  3. on text → emit assistant_message event
                  4. append tool_result to messages
                  5. derive file_edit / command_run / action_emitted events
                  6. continue until end_turn or maxTurns
        │
        ▼
rupu-transcript: write run_complete, close file.
```

## Authentication

- **Storage:** `keyring` crate. v0 ships binaries for macOS (arm64/x86_64) and Linux (x86_64/arm64) only — `keyring` uses macOS Keychain and Linux Secret Service via D-Bus on those platforms. Windows Credential Manager is supported by the crate but not exercised in v0 (no Windows release binary). On probe failure (no D-Bus, headless server, etc.), fall back to `~/.rupu/auth.json` mode 0600 with a one-time warning. Probe result cached at `~/.rupu/cache/auth-backend.json` so we don't re-probe every invocation; cache invalidated on `rupu auth login` and on `--probe-auth` flag.
- **`auth.json` permission enforcement:** mode bits checked on every read; warn loudly if not 0600.
- **`rupu auth login`:** interactive flow per provider; stores result via the chosen backend.
- **`rupu auth status`:** shows configured providers + storage backend; never prints credentials.

## Error handling

Four buckets, each handled differently:

1. **User errors** (bad agent name, missing config key, malformed workflow YAML) → exit non-zero, single-line clear message, no stack trace. `thiserror`-typed in libraries; formatted at CLI boundary.
2. **Provider errors** (rate limit, network, auth failure) → retry with exponential backoff for transient (rate limit, 5xx, network); fail-fast for non-transient (401, 400). Backoff config in `rupu-config`. Retries logged as `tool_result` events with `error: "provider_retry"` for transparency.
3. **Tool errors** (bash exit non-zero, file not found, edit didn't match) → not errors at the agent-loop level. Surfaced to the agent as `tool_result` with failure detail. Agent decides next step.
4. **Internal errors** (crate-level bugs, panics) → caught at CLI boundary, logged to `~/.rupu/cache/crash-<timestamp>.log` with full backtrace, exit code 2, brief user message pointing at the log. `RUST_BACKTRACE=1` always set internally.

## Failure modes (pre-decided)

- **Half-written transcripts** (mid-run crash): no `run_complete` event present. Readers treat as `aborted`, do not skip.
- **Concurrent runs in the same workspace:** allowed in v0. Each gets unique run_id; transcripts don't collide. Working-directory-level conflicts are a user problem in v0; Slice C sandboxing sidesteps this.
- **Provider context overflow:** abort the run with `run_complete { status: "error", error: "context_overflow" }`. v0 does not auto-summarize or truncate. Compaction is a real feature with real tradeoffs and belongs in a later slice.
- **Tool timeouts:** SIGTERM then SIGKILL on the bash process; `tool_result { error: "timeout", killed: true }` to the agent.

## Security posture (v0)

`bypass` mode shells out arbitrary commands. The runtime cannot prevent agent misbehavior in `bypass` — it can only minimize accidents:

- `auth.json` always 0600; warn on read if mode is wrong.
- `bash` always runs in workspace cwd (or configured cwd), never inherits arbitrary cwd from agent input.
- `bash` env is controlled (PATH, HOME, USER, TERM, LANG + per-workspace allow-list). No env var injection from agent input.
- README has explicit "agents are code that can do anything you can do — review what you run" section. No false safety promises in `bypass`.

## Testing strategy

### Layer 1 — unit tests, per crate

Standard `#[cfg(test)] mod tests`. Each crate's public surface tested in isolation:

- `rupu-config`: layering (project overrides global), array replacement vs merge, TOML parse errors, missing-file behavior.
- `rupu-workspace`: discovery walks up correctly, canonicalization handles symlinks, ULID generation collision-free under load.
- `rupu-transcript`: round-trip every event variant; reader handles missing `run_complete`; line-oriented parsing handles truncated last lines.
- `rupu-orchestrator`: linear runner executes in order; action allowlist denies unauthorized verbs; template rendering injects prior step outputs.
- `rupu-tools`: per-tool input validation; `edit_file` exact-match failure; `bash` timeout and signal handling.
- `rupu-auth`: keychain probe + fallback path; `auth.json` mode bits enforced.
- `rupu-providers`: lifted tests come with the lift.

### Layer 2 — integration tests in `tests/`

Black-box, exercise the binary against a **mock provider**, no network:

- `tests/mock_provider.rs`: `Provider` impl returning a scripted sequence of events. Other tests build scripts and assert on the resulting transcript.
- `tests/run_basic.rs`: `rupu run mock-agent "do thing"` produces a well-formed transcript.
- `tests/workflow_linear.rs`: 3-step workflow runs in order; second step sees first step's output via template.
- `tests/permissions_ask.rs`: pty-simulated TTY verifies `ask` prompts and respects responses; non-TTY abort verified.
- `tests/config_layering.rs`: project config overrides global; `auth.json` never read from project dir.
- `tests/transcript_aborted.rs`: kill mid-run; `transcript list` reports aborted; reader doesn't crash.

### Layer 3 — manual smoke (the "exit criterion B" gate)

Before declaring Slice A done:

- 4-5 shipped agents (`fix-bug`, `add-tests`, `review-diff`, `scaffold`, `summarize-diff`) run against a real repo with each provider that has credentials available. Skipped providers documented.
- Clean macOS arm64 install from `cargo install`. Clean install from a downloaded GitHub Release binary. First run works without `~/.rupu/` existing (auto-creation).
- One full linear workflow (`investigate → plan → implement → test`) on a real bug in a real repo.

### Test code rules

1. **No mocks for the agent loop except the provider boundary.** Tools run for real (against a tempdir). Real `bash`, real `edit_file`. Mocking tools too aggressively gives tests that pass against fiction.
2. **No network in unit or integration tests.** The mock provider is the only "model" the agent loop talks to in CI. Real-provider tests gated behind `--ignored` and run manually.

### Explicitly not in v0

No property-based testing (no protocol code yet), no fuzz testing, no snapshot testing for transcripts (event order too non-deterministic to be worth the brittleness).

## Build & release

- **Toolchain:** Rust stable; MSRV pinned in `Cargo.toml`.
- **Lints:** `#![deny(clippy::all)]` workspace-wide; `clippy::pedantic` opt-in per-crate.
- **Format:** `rustfmt` on commit; CI fails on diff.
- **PR builds:** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`, `cargo build --release` on macOS arm64 + Linux x86_64. PRs must be green.
- **Releases:** tag-triggered (`v*`); build matrix produces macOS arm64, macOS x86_64, Linux x86_64, Linux arm64. Strip + tar.gz + checksum. Upload via `softprops/action-gh-release`.
- **Distribution:** GitHub Releases + `cargo install --git https://github.com/section9labs/rupu`. No Homebrew tap, no apt repo, no signed installers in v0. Slice C/D revisits.

## Documentation surface

- `README.md`: install, first run, auth setup, where things live, 2-3 example agent runs.
- `docs/spec.md`: source-of-truth architecture (mirrors phi-cell pattern). Every implementation decision must trace to it.
- `docs/agent-format.md`: agent file frontmatter reference + worked examples.
- `docs/workflow-format.md`: linear workflow YAML reference.
- `docs/transcript-schema.md`: rupu-native event schema reference.
- `CLAUDE.md`: project memory for agents working on rupu itself.

## Telemetry

None in v0. Adding telemetry later requires explicit user opt-in. No phone-home, no anonymous metrics, no first-run prompt.

## Repository

Public repository at `github.com/section9labs/rupu`. Public from day one.
