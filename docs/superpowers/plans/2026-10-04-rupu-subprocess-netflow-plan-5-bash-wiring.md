# Subprocess netflow capture — Plan 5: bash wiring (the first real flows)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Checkbox steps.

**Goal:** Wire the bash tool to the capture backend so an agent's bash command's network connections are actually captured, attributed to the run/agent/tool-call, and written to the run's netflow ledger. Until now nothing calls `net_capture::shared`; this plan makes the subsystem live end-to-end. The CP views (Plan 6) then show these flows with no new endpoint.

**Architecture:** `ToolContext` gains the run's `FlowSink`, the process-wide `SubprocessCapture`, and the current `tool_call_id`. The bash tool, when all three are present, calls `capture.begin(attribution)` before spawning, prepends `shell_prefix()` to the script, calls `spawned(pid)`, and `finished()` after the wait. The runner stamps `tool_call_id` per invocation and calls `run_finished` at run end. Every run-assembly site that already builds a per-run `FlowSink` (`netflow_sink::for_run`, `step_netflow_sink`) also sets `net_capture` from a process-wide, lazily-warmed `net_capture::shared(&cfg)` — warmed OFF the async runtime because `shared()` blocks (Plan-3/4 hard requirement).

**Tech stack:** Rust 2021. `rupu-tools` gains a dep on `rupu-netflow` (default features — the capture port + `FlowSink` are there; no cycle, rupu-netflow is a leaf). The end-to-end live test runs on THIS Mac via the macOS backend.

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` §8 (and §7.5 field mapping already in the model).

## Global Constraints
- **No `unsafe`.** `#![deny(clippy::all)]`. Workspace dep versions only.
- **Capture never breaks the bash call** (spec §13): every capture step is best-effort; if `net_capture`/sink/ids aren't all present, bash runs exactly as today (unchanged path). A `begin`/`finished` that errors or an inert call never fails, slows, or alters the command.
- **`net_capture::shared` blocks** (`setup_root`/ntstat open) — it MUST be obtained off the async runtime (`tokio::task::spawn_blocking` or a one-time eager warm), never inline on an async worker. Memoized, so warm once per process.
- **No duplicate capture.** One `SubprocessCapture` per process (the memoized `shared`); every `ToolContext` references that same `Arc`. A sub-agent/child run uses its OWN sink but the SAME capture.
- **Additive ToolContext fields** are `#[serde(skip)]` (they carry `Arc`s / per-invocation ids), matching the existing `dispatcher`/`coverage_writer` pattern.

## File structure / sites
- `crates/rupu-tools/Cargo.toml` — add `rupu-netflow = { workspace = true }`.
- `crates/rupu-tools/src/tool.rs` — `ToolContext` + 3 fields.
- `crates/rupu-tools/src/bash.rs` — drive capture around the spawn.
- `crates/rupu-agent/src/runner.rs` — per-invocation `tool_call_id` (invoke site ~2887); `run_finished` at run end (~3264, by `coverage_writer = None`).
- `crates/rupu-cli/src/netflow_sink.rs` — a `net_capture_shared` warm helper (or a new small module), used by the sites.
- Wiring sites (set `netflow_sink` + `net_capture` on the `ToolContext` they build): `cmd/run.rs` (~1012), `cmd/session.rs` (~7780), `cmd/dispatch.rs` (~407, child ctx), and the orchestrator's `step_factory.rs` (~367). `resume.rs`/`workflow.rs` `for_run` sites feed the SCM registry sink; they build no agent `ToolContext` directly, but confirm whether the run they launch gets a ToolContext downstream and wire there.
- `crates/rupu-netwatch` — no change.

---
### Task 1: `ToolContext` fields + bash tool drives capture

**Files:** `crates/rupu-tools/Cargo.toml`, `src/tool.rs`, `src/bash.rs`; test in `bash.rs`.

**Interfaces — produces:** `ToolContext` gains (all `#[serde(skip)]`):
- `pub netflow_sink: Option<std::sync::Arc<dyn rupu_netflow::FlowSink>>`
- `pub net_capture: Option<std::sync::Arc<dyn rupu_netflow::SubprocessCapture>>`
- `pub tool_call_id: Option<String>`

- [ ] **Step 1: dep + fields.** Add `rupu-netflow = { workspace = true }` to rupu-tools deps. Add the three fields to `ToolContext` with `#[serde(skip)]` and doc comments; add them to `Default` (all `None`) and fix any `ToolContext { .. }` literal the compiler flags (add the three fields or use `..Default::default()` where present) — including the runner's test literals and `cmd/*` construction sites (set to `None` for now; real values wired in Task 3).
- [ ] **Step 2: write the failing test** in `bash.rs`: a recording `SubprocessCapture` test double (records `begin` attribution, `shell_prefix` handed out, `spawned(pid)`, `finished`). Build a `ToolContext` with `net_capture = Some(recorder)`, `netflow_sink = Some(Arc::new(MemorySink))`, `run_id = Some("run-x")`, `tool_call_id = Some("toolu_1")`, and invoke `BashTool` with `echo hi`. Assert: `begin` was called once with `tool_call_id == "toolu_1"` and `run_id == "run-x"`; the command actually ran (`stdout` contains `hi`); `spawned` got the child pid; `finished` was called once. A second test: with `net_capture = None`, bash runs normally and nothing is recorded (unchanged path).
- [ ] **Step 3: run → fail.**
- [ ] **Step 4: implement in `bash.rs` `invoke`:** when `ctx.net_capture`, `ctx.netflow_sink`, `ctx.run_id`, and `ctx.tool_call_id` are all `Some`, build a `rupu_netflow::CallAttribution { run_id, step_id: ctx.step_id?/None, agent: ctx.agent, codename: ctx.codename, tool_call_id, sink: netflow_sink.clone() }` and `let mut call = capture.begin(attribution)`. If `call.shell_prefix()` is `Some(p)`, run `/bin/sh -c "<p>\n<command>"` (the prefix moves the shell into its cgroup on Linux; `None` on macOS → unchanged script). After `cmd.spawn()`, `call.spawned(child.id())`. After the wait (success, error, OR timeout — all branches), `Box::new(call).finished()` (or `call.finished()` on the boxed value). Environment, cwd, timeout, `kill_on_drop`, and the `CommandRun` derived event (which still records the ORIGINAL command, not the prefixed one) are unchanged. When any of the four is `None`, keep the exact current path.
- [ ] **Step 5: run → pass; clippy.**
- [ ] **Step 6: commit.** `tools: bash drives subprocess capture (begin/shell_prefix/spawned/finished)`

Note (check `ToolContext` for a `step_id`/`agent`/`codename` field — the earlier plans reference `ctx.agent`/`ctx.codename`; use whatever the struct actually exposes, else pass `None`).

---

### Task 2: runner stamps `tool_call_id` + `run_finished`

**Files:** `crates/rupu-agent/src/runner.rs`; test.

- [ ] **Step 1: failing test** (mock provider driving one bash tool call through the runner): a recording capture on the `tool_context`; assert `begin` saw the real `call_id` the runner assigned, and `run_finished(run_id)` was called once when the run ended.
- [ ] **Step 2: implement.** At the invoke site (`tool.invoke(input.clone(), &opts.tool_context)`, ~2887): clone the context, set `ctx.tool_call_id = Some(call_id.clone())`, invoke with the clone (`ToolContext` is `Clone`; it holds `Arc`s + small fields). At run end (by `opts.tool_context.coverage_writer = None`, ~3264): `if let Some(cap) = &opts.tool_context.net_capture { cap.run_finished(&opts.run_id); }`.
- [ ] **Step 3: run → pass; clippy.**
- [ ] **Step 4: commit.** `agent: runner stamps tool_call_id per invocation and calls run_finished`

---
### Task 3: warm `net_capture::shared` off-runtime + wire it at every ToolContext site

**Files:** `crates/rupu-cli/src/netflow_sink.rs` (warm helper), `cmd/run.rs`, `cmd/session.rs`, `cmd/dispatch.rs`, `crates/rupu-orchestrator/src/step_factory.rs`; a choke-point test.

- [ ] **Step 1: warm helper.** In `rupu-cli` (e.g. `netflow_sink.rs`): `pub async fn net_capture(cfg: &NetflowConfig) -> Arc<dyn SubprocessCapture>` that does `let cfg = cfg.clone(); tokio::task::spawn_blocking(move || rupu_runtime::net_capture::shared(&cfg)).await.unwrap_or_else(|_| Arc::new(rupu_netflow::NoopCapture))`. `shared` memoizes, so this blocks only once per process; later calls return the cached Arc cheaply (still via spawn_blocking for uniformity). For the orchestrator `step_factory` (which may be sync / not have the CLI helper), obtain the capture the same way at the async boundary that builds the step's `ToolContext`, or thread it in from the caller; if `step_factory` has no async context, have its caller pass the `Arc<dyn SubprocessCapture>` in.
- [ ] **Step 2: wire the ToolContext sites.** At `cmd/run.rs` (~1012), `cmd/session.rs` (~7780), `cmd/dispatch.rs` (~407 child ctx — the child uses the child run's `netflow_sink` but the SAME process-wide capture), and `step_factory.rs` (~367): set `netflow_sink: Some(<the for_run/step sink>)` and `net_capture: Some(<warmed capture>)` on the `ToolContext`. The child dispatch ctx inherits `net_capture` from the parent ctx (same Arc) and uses the child's sink.
- [ ] **Step 3: choke-point test** (pattern: `crates/rupu-netflow/tests/it/choke_point.rs`). A test that greps the source for every `ToolContext {` construction in the production run paths (`cmd/run.rs`, `cmd/session.rs`, `cmd/dispatch.rs`, `step_factory.rs`) and fails if any of them does not set BOTH `netflow_sink:` and `net_capture:` (or inherit them). A text-level backstop; document it's a backstop like the netflow choke-point test.
- [ ] **Step 4:** `cargo build -p rupu-cli -p rupu-orchestrator`; `cargo test -p rupu-cli -p rupu-orchestrator` (touched areas); clippy. Commit: `cli/orchestrator: wire net_capture into every run's ToolContext (warmed off-runtime)`

---

### Task 4: end-to-end live test (this Mac) + fold Plan-1 deferred

**Files:** a new `#[ignore]` + `#[serial]` test in `crates/rupu-cli/tests/serial/` (cli has rupu-runtime + rupu-netwatch transitively; the macOS backend runs here).

- [ ] **Step 1: write the live end-to-end test** (`#[cfg(target_os = "macos")]`, `#[ignore]`, `#[serial]`): run a real `rupu run` (or the run-assembly path) of a one-step agent whose bash command is `curl -s -o /dev/null --max-time 6 http://example.com` (use the mock provider to emit exactly one bash tool call, OR drive the bash tool through the agent runner with a real macOS `net_capture`), with a real per-run netflow ledger. After the run, read the run's `netflow/<run_id>.jsonl` (via `rupu_netflow::ledger::read_flows`) and assert: at least one `Fidelity::Socket` flow, `origin == Subprocess(_)`, `ctx.tool_call_id == Some(<the call id>)`, remote port 80, and a folded `SocketComplete` (`body_complete == true`) — this is the first true end-to-end proof AND folds Plan-1's deferred end-to-end `complete_socket` assertion. Resilient asserts (IP varies; allow a short settle for the watcher).
- [ ] **Step 2: run it on this Mac** (`cargo test -p rupu-cli --test <serial binary> -- --ignored <name>`), confirm it passes. If flaky on timing, hold the connection (`curl --limit-rate`) so the watcher's poll catches it established.
- [ ] **Step 3: commit.** `cli: end-to-end live test — a bash curl flow is captured and attributed`

---

## Final verification
- [ ] macOS: `cargo test -p rupu-tools -p rupu-agent -p rupu-cli -p rupu-orchestrator` green; the `#[ignore]` macOS end-to-end test passes on THIS Mac; clippy --all-targets clean; `cargo build` workspace-wide (the new rupu-tools→rupu-netflow dep + wiring compile everywhere).
- [ ] Linux: rsync to kali6, `cargo build -p rupu-cli` + `cargo test -p rupu-tools -p rupu-agent` (the wiring compiles on Linux; the Linux backend attributes under a real run — optionally run the end-to-end there too if a Linux variant of the test exists). [controller]
- [ ] Capture-off path unchanged: with `[netflow] subprocess_capture = false`, bash runs with no capture and no behavior change (a test).

## Notes for Plan 6
The ledger now contains real `Fidelity::Socket` / `Origin::Subprocess` flows with `ctx.tool_call_id`. Plan 6 renders them in the CP views (badge, socket-row layout, `subprocess:*` topology nodes, the transcript link via `run_id`+`tool_call_id`) and reads `LedgerLine::Capture`'s `state.state` for the unavailable/loss disclosure.
