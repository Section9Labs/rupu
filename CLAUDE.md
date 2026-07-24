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

## rupu-app rules (learned from 8h debugging chain)
1. **Metal Toolchain is a build prerequisite on macOS.** Install once:
   `xcodebuild -downloadComponent MetalToolchain`. Without it the GPUI shader compile step fails.
2. **Never enable `gpui_platform`'s `runtime_shaders` feature.** It compiles Metal shaders at app startup to bypass the toolchain requirement at build time, but the runtime-compiled glyph shader silently produces no pixels — colored shapes render, text doesn't. Stay on the default (build-time shader compilation).
3. **Click handlers must defer entity updates via `cx.defer(...)`.** Calling `weak.update(cx, |this, cx| ...)` synchronously inside an `on_click` re-enters `AppCell::borrow_mut` while GPUI is still dispatching the click → `RefCell already borrowed` flood at the display refresh rate. `cx.spawn(async ...)` does NOT work as a substitute (the future is polled on the same foreground executor). `cx.listener(...)` is the same trap dressed up. `cx.defer(...)` is the correct primitive — see `gpui::App::defer` docstring.
4. **GUI changes require runtime validation before merge.** `cargo build` + `cargo test` cleanliness ≠ rendering cleanliness. Subagents cannot validate GPUI rendering. matt runs the binary before any rupu-app PR merges.

### Crates

- **`rupu-agent`** — agent file format (`.md` + YAML frontmatter), agent loop, and permission resolver. Lifts spec/loader/permission/runner/tool_registry into one integration crate. Mock-provider tests use `MockProvider` + `BypassDecider` exposed from `runner`.
- **`rupu-app-canvas`** — pure-Rust view layer for rupu.app (Slice D). Walks a `rupu_orchestrator::Workflow` and emits a `Vec<GraphRow>` of structured cells (pipe / branch glyph / bullet / label / meta) for the git-graph view. Snapshot-tested with insta; no GPUI dep. rupu-app's `view/graph.rs` consumes the rows and paints with GPUI text spans. D-6 will add `layout_canvas`/`layout_tree` here for the Canvas view's col×row grid.
- **`rupu-app`** — native macOS desktop app via GPUI. Owns an `AppExecutor` (wrapping `InProcessExecutor`) that starts workflows in-process AND tails disk runs via `FileTailRunSource`. `RunModel::apply(Event)` mutates per-run UI state. The Graph view paints `NodeStatus` per node from the model; the drill-down pane streams the focused step's transcript JSONL and exposes Approve / Reject buttons. The same Approve / Reject buttons also render inline on Awaiting nodes in the Graph view. Sidebar workflow rows show status dots when their workflow has an active run; menubar badge counts pending approvals across workspaces. The launcher sheet (D-4) is the canonical entry to dispatch a workflow from the app — toolbar Run button, ⌘R on a focused sidebar row, or right-click → Run all open the same floating sheet (inputs form, mode picker Ask/Bypass/Read-only, target picker workspace/directory/RepoRef-clone). Clones land in `~/Library/Caches/rupu.app/clones/<ULID>/`; a best-effort 7-day sweep runs on startup.
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Thirteen subcommands: `init` / `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `issues` / `mcp` / `watch` / `update`. Releases publish `beta` (prerelease) + `stable` channels (`make gh-beta` / `make gh-stable`, `scripts/gh-build.sh <beta|stable>`); `rupu update` follows the configured `[update].channel` (default `stable`). `rupu cp serve` runs a background gate sweep (`cmd/cp.rs`, alongside the cron tick) that fires overdue gates' `on_timeout` routing (reject → runs the same `on_reject` cleanup chain the CLI reject path runs; approve → spawns a detached `workflow approve`) and reaps orphaned `Running`/`Pending` runs (dead recorded `runner_pid`) as `Failed` — gated by `[cp].gate_sweep_enabled` / `[cp].gate_sweep_interval_secs` (`rupu-config`'s `CpConfig`, default on / 60s); landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-4-notify-and-sweep.md.
- **`rupu-update`** — pure-ish lib crate behind `rupu update`: `ReleaseSource`/downloader ports, channel-aware latest-release selection, sha256 checksum verification, and atomic in-place binary swap with backup/rollback. `rupu-cli`'s `update` subcommand and its `build_info` module (embeds `RUPU_RELEASE_CHANNEL`/`RUPU_RELEASE_VERSION` via `option_env!` at compile time) are the only consumers.
- **`rupu-mcp`** — embedded MCP server. Two transports (in-process for the agent runtime, stdio for `rupu mcp serve`); single tool catalog backed by `rupu-scm`'s Registry. Permission gating mirrors the six-builtin model: per-tool allowlist + per-mode (`ask` / `bypass` / `readonly`).
- **`rupu-orchestrator`** — workflow YAML parser + minijinja rendering + linear runner with pluggable `StepFactory`. Action-protocol allowlist validation lives here. **Executor module** (`crates/rupu-orchestrator/src/executor/`): `WorkflowExecutor` + `EventSink` traits + step-level `Event` enum. `InProcessExecutor` runs workflows in a tokio task and fans events through `InMemorySink` (broadcast for live subscribers) + `JsonlSink` (append-only `events.jsonl` next to the existing `run.json` / `step_results.jsonl`). `FileTailRunSource` is the disk-tail counterpart for runs the executor didn't start (CLI / cron / MCP). Both rupu-cli and rupu-app route through this surface. Approval gate nodes (`approval:`-standalone step) landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-1-schema-and-runner.md; `action:` connector steps now execute for real through the in-process MCP `ToolDispatcher` (Plan 2), landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-2-action-execution.md. Gate/action nodes now render as first-class rows in both `rupu-app-canvas`'s `GraphRow` output and `rupu-cp`'s web run viewer (`GateNode`/`ActionNode`), and are authorable in the CP `next` workflow editor (kind picker + StepForm bodies + `/api/tools` MCP catalog for the action `with:` editor) — landed via docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-3-renderers-and-editor.md. That plan also ships an affordance for a legacy inline `approval:` on an agent step (both classic and `next`): a dashed gate badge marker plus a `workflowGraph.convertInlineApprovalToGate` "Convert to gate node" button that lifts the approval onto a new standalone gate step inserted just before it; full auto-synthesis of a phantom gate node is deferred. Plan 4 (docs/superpowers/plans/2026-07-23-rupu-gate-nodes-plan-4-notify-and-sweep.md) closes the arc: a gate's `notify:` connector hooks fire best-effort right as it parks (before the `StepAwaitingApproval` emit, via the same `action_dispatcher`/`execute_action_step` Plan 2 wired in — never on auto-approve, never blocking the pause on a notify failure), and `RunStore::reap_if_orphaned` finalizes a `Running`/`Pending` run with a dead recorded `runner_pid` as `Failed` with a terminal event appended (closing the same "spins forever" class PR #501 fixed for the approval-timeout side). **The gate/action-node arc is now complete**: action steps execute, gates render + are authorable, notify hooks fire, and unattended timeout routing + orphan reaping run without an operator present. The run-detail graph shares the editor's per-kind palette behind `[cp].workflow_editor_ui = "next"` (kind-colored nodes + connectors, run status as a glyph/label/animation overlay) via `components/graph/kindBridge.ts`; landed per `docs/superpowers/plans/2026-07-23-rupu-run-graph-next-visuals.md`.
- **`rupu-scm`** — SCM/issue-tracker connectors. `RepoConnector` + `IssueConnector` traits per spec §4c; per-platform impls under `connectors/<platform>/`. Plan 1 ships GitHub; Plan 2 adds GitLab + the embedded MCP server.
- **`rupu-keychain-acl`** — macOS-only Security.framework FFI shim that pre-populates new keychain items' ACL with rupu's signing identity, eliminating the "Always Allow" first-prompt. Only crate in the workspace exempt from `unsafe_code = "forbid"`; FFI module opts in via `#![allow(unsafe_code)]`. No-op on non-macOS.

**Run-time samples:** live at `<repo>/.rupu/agents/` and `<repo>/.rupu/workflows/`. Running `rupu` from inside the rupu checkout exercises the same project-discovery code path end-users use in their own repos.

## Code standards
- Rust 2021, MSRV pinned in `rust-toolchain.toml`.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary (Plan 2).
- Async: `tokio`.
- Logging: `tracing` + `tracing-subscriber`.

## Heritage
- **Okesu** (`/Users/matt/Code/Oracle/Okesu`) — Go security-ops sibling. Same architectural shape (agent files = `.md` + YAML, JSONL transcripts, action protocol).
- **phi-cell** (`/Users/matt/Code/phi-cell`) — Rust workspace; `crates/phi-providers` is lifted near-verbatim into `crates/rupu-providers`. Lift origin: `Section9Labs/phi-cell` commit `3c7394cb1f5a87088954a1ff64fce86303066f55`.
