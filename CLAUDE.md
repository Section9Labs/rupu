# rupu — agentic code-development CLI

## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Slice B-2 spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`
- Slice C spec: `docs/superpowers/specs/2026-05-05-rupu-slice-c-tui-design.md`
- Plan 1 (foundation + GitHub connector, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-1-foundation-and-github.md`
- Plan 2 (GitLab + MCP server, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-2-gitlab-and-mcp.md`
- Plan 3 (CLI run-target + docs + nightly, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-3-cli-and-docs.md`
- Slice C plan: `docs/superpowers/plans/2026-05-05-rupu-slice-c-tui-plan.md`
- Slice D Plan 2 (Graph view, complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-2-graph-view.md`
- Slice D Plan 3 (live executor + status pulse, complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-3-live-executor.md`
- Slice D Plan 3 spec: `docs/superpowers/specs/2026-05-12-rupu-slice-d-plan-3-live-executor-design.md`
- Slice D Plan 4 (Launcher, operator-complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-4-launcher.md`
- Slice D Plan 4 spec: `docs/superpowers/specs/2026-05-12-rupu-slice-d-plan-4-launcher-design.md`
- Workflow triggers spec: `docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`
- Workflow triggers Plan 1 (polled events on cron tick): `docs/superpowers/plans/2026-05-07-rupu-workflow-triggers-plan-1-polled-events.md`

## Architecture rules (enforced)
1. **Hexagonal separation.** `rupu-providers`, `rupu-tools`, `rupu-auth` define traits (ports). The agent runtime in `rupu-agent` only knows traits.
2. **`rupu-cli` is thin.** Subcommands are arg parsing + delegation. No business logic in the CLI crate.
3. **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
4. `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code` forbidden.

### Crates

- **`rupu-agent`** — agent file format (`.md` + YAML frontmatter), agent loop, and permission resolver. Lifts spec/loader/permission/runner/tool_registry into one integration crate. Mock-provider tests use `MockProvider` + `BypassDecider` exposed from `runner`.
- **`rupu-app-canvas`** — pure-Rust view layer for rupu.app (Slice D). Walks a `rupu_orchestrator::Workflow` and emits a `Vec<GraphRow>` of structured cells (pipe / branch glyph / bullet / label / meta) for the git-graph view. Snapshot-tested with insta; no GPUI dep. consumed by `rupu-cli`'s workflow output (`cmd/workflow.rs`, `output/live_run.rs`).
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Thirteen subcommands: `init` / `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `issues` / `mcp` / `watch` / `update`. Releases publish `beta` (prerelease) + `stable` channels via CI (`.github/workflows/release.yml`, triggered by a `v*` tag push): `gh workflow run release-beta.yml` cuts a beta now (`-f force=true` even if `main` hasn't moved since the last one), while stable is promoted from a soaked beta weekly by `release-stable.yml` rather than published directly; `rupu update` follows the configured `[update].channel` (default `stable`). `rupu cp serve` runs a background gate sweep (`cmd/cp.rs`, alongside the cron tick) that fires overdue gates' `on_timeout` routing (reject → runs the same `on_reject` cleanup chain the CLI reject path runs; approve → spawns a detached `workflow approve`) and reaps orphaned `Running`/`Pending` runs (dead recorded `runner_pid`) as `Failed` — gated by `[cp].gate_sweep_enabled` / `[cp].gate_sweep_interval_secs` (`rupu-config`'s `CpConfig`, default on / 60s); landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-4-notify-and-sweep.md.
- **`rupu-update`** — pure-ish lib crate behind `rupu update`: `ReleaseSource`/downloader ports, channel-aware latest-release selection, sha256 checksum verification, and atomic in-place binary swap with backup/rollback. `rupu-cli`'s `update` subcommand and its `build_info` module (embeds `RUPU_RELEASE_CHANNEL`/`RUPU_RELEASE_VERSION` via `option_env!` at compile time) are the only consumers.
- **`rupu-mcp`** — embedded MCP server. Two transports (in-process for the agent runtime, stdio for `rupu mcp serve`); single tool catalog backed by `rupu-scm`'s Registry. Permission gating mirrors the six-builtin model: per-tool allowlist + per-mode (`ask` / `bypass` / `readonly`).
- **`rupu-orchestrator`** — workflow YAML parser + minijinja rendering + linear runner with pluggable `StepFactory`. Action-protocol allowlist validation lives here. **Executor module** (`crates/rupu-orchestrator/src/executor/`): `WorkflowExecutor` + `EventSink` traits + step-level `Event` enum. `InProcessExecutor` runs workflows in a tokio task and fans events through `InMemorySink` (broadcast for live subscribers) + `JsonlSink` (append-only `events.jsonl` next to the existing `run.json` / `step_results.jsonl`). `FileTailRunSource` is the disk-tail counterpart for runs the executor didn't start (CLI / cron / MCP). `rupu-cli` routes through this surface. Approval gate nodes (`approval:`-standalone step) landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-1-schema-and-runner.md; `action:` connector steps now execute for real through the in-process MCP `ToolDispatcher` (Plan 2), landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-2-action-execution.md. Gate/action nodes now render as first-class rows in both `rupu-app-canvas`'s `GraphRow` output and `rupu-cp`'s web run viewer (`GateNode`/`ActionNode`), and are authorable in the CP workflow editor (kind picker + StepForm bodies + `/api/tools` MCP catalog for the action `with:` editor) — landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-3-renderers-and-editor.md. That plan also ships an affordance for a legacy inline `approval:` on an agent step: a dashed gate badge marker plus a `workflowGraph.convertInlineApprovalToGate` "Convert to gate node" button that lifts the approval onto a new standalone gate step inserted just before it; full auto-synthesis of a phantom gate node is deferred. Plan 4 (docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-4-notify-and-sweep.md) closes the arc: a gate's `notify:` connector hooks fire best-effort right as it parks (before the `StepAwaitingApproval` emit, via the same `action_dispatcher`/`execute_action_step` Plan 2 wired in — never on auto-approve, never blocking the pause on a notify failure), and `RunStore::reap_if_orphaned` finalizes a `Running`/`Pending` run with a dead recorded `runner_pid` as `Failed` with a terminal event appended (closing the same "spins forever" class PR #501 fixed for the approval-timeout side). **The gate/action-node arc is now complete**: action steps execute, gates render + are authorable, notify hooks fire, and unattended timeout routing + orphan reaping run without an operator present. The run-detail graph shares the editor's per-kind palette unconditionally (kind-colored nodes + connectors, run status as a glyph/label/animation overlay) via `components/graph/kindBridge.ts` — the `[cp].workflow_editor_ui` gate was removed in Arc 3 (ISSUES.md I-30) when the classic renderers were deleted; landed per `docs/superpowers/plans/2026-07-23-rupu-run-graph-next-visuals.md`.
- **`rupu-scm`** — SCM/issue-tracker connectors. `RepoConnector` + `IssueConnector` traits per spec §4c; per-platform impls under `connectors/<platform>/`. Plan 1 ships GitHub; Plan 2 adds GitLab + the embedded MCP server.

**Run-time samples:** live at `<repo>/.rupu/agents/` and `<repo>/.rupu/workflows/`. Running `rupu` from inside the rupu checkout exercises the same project-discovery code path end-users use in their own repos.

## Code standards
- Rust 2021, MSRV pinned in `rust-toolchain.toml`.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary (Plan 2).
- Async: `tokio`.
- Logging: `tracing` + `tracing-subscriber`.

## Heritage
- **Okesu** (`/Users/matt/Code/Oracle/Okesu`) — Go security-ops sibling. Same architectural shape (agent files = `.md` + YAML, JSONL transcripts, action protocol).
- **phi-cell** (`/Users/matt/Code/phi-cell`) — Rust workspace; `crates/phi-providers` is lifted near-verbatim into `crates/rupu-providers`. Lift origin: `Section9Labs/phi-cell` commit `3c7394cb1f5a87088954a1ff64fce86303066f55`.
