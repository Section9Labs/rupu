# Netflow Plan 1 — per-run ledgers and per-run sink resolution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the process-global netflow sink with per-run resolution, give every run its own ledger file mirroring transcripts, and wire the agent-driven entry points that currently capture nothing.

**Architecture:** A ledger becomes one file per run (`<netflow_dir>/{run_id}.jsonl`) resolved exactly like transcripts. The sink stops being a process-wide `OnceLock` and instead threads explicitly through `provider_factory::build_for_provider_with_config`, the single chokepoint every agent path already funnels through. Because the file *is* the attribution, this deletes the ledger↔transcript merge, the global-ledger enumeration and the `pwd` fallback.

**Tech Stack:** Rust 2021, `tokio`, `chrono`, `serde`, existing `rupu-netflow` / `rupu-runtime` / `rupu-cli` / `rupu-orchestrator` crates.

**Spec:** `docs/superpowers/specs/2026-08-04-rupu-netflow-per-run-ledger-design.md`

## Global Constraints

- **Capture must never break a request.** No `unwrap()`/`expect()`/panic reachable from a sink or from `Middleware::handle`. A per-run writer that cannot open its file degrades to a logged no-op.
- **Loss must be visible, never silent.** Drop accounting and the `Dropped` ledger line survive this refactor unchanged.
- **`Fidelity` and `Option` semantics are untouched.** Unknown must never become a number; known must never become unknown.
- **Workspace deps only.** Versions pinned in the root `Cargo.toml`; never in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` and `unsafe_code = "forbid"` workspace-wide. The HTTP choke-point lint is at `deny`.
- **Run-less flows are not recorded.** If there is no run, there is no ledger and no capture. Do not invent a daemon-level fallback.
- **Format ONLY files you touch, with `rustfmt --edition 2021 <path>`.** Never `cargo fmt`; never `rustfmt` a crate root or `mod.rs` — it walks the whole `mod` tree. After ANY formatting run `git status --porcelain` and `git checkout --` anything unintended.
- **Never use `git stash`** — the stash stack is shared across worktrees.
- **Run tests in the FOREGROUND.** No `pgrep -f` wait loops — the polling shell's own command line matches the pattern and the loop never exits.
- Every task ends with `cargo build --workspace` green, not just its own package.

---

### Task 1: Trace autoflow's execution path

**Files:**
- Create: `docs/superpowers/plans/notes/2026-08-04-autoflow-provider-path.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a recorded finding that determines whether Task 7 wires autoflow. No code.

The spec mandates this first because it is genuinely unresolved: `crates/rupu-cli/src/cmd/autoflow_runtime.rs` contains no `build_for_provider*` call and no obvious subprocess spawn, so how autoflow executes an agent is not established. Both outcomes are fine; guessing is not.

- [ ] **Step 1: Trace it**

Start from `crates/rupu-cli/src/cmd/autoflow.rs` and `autoflow_runtime.rs`, and follow whatever path leads to an agent actually running. Useful probes:

```bash
grep -rn "build_for_provider\|run_inner\|CliAgentDispatcher\|AgentRunner" crates/rupu-cli/src/cmd/autoflow*.rs crates/rupu-runtime/src
grep -rn "current_exe\|Command::new\|tokio::process" crates/rupu-cli/src/cmd/autoflow*.rs
grep -rn "autoflow" crates/rupu-runtime/src/worker.rs
```

- [ ] **Step 2: Write the finding**

Create the note file recording, in a few sentences: which function actually starts an agent, whether it is in-process or a spawned `rupu` subprocess, and therefore whether autoflow needs its own sink wiring in Task 7.

If it spawns `rupu run` subprocesses, each is one run per process and autoflow inherits capture for free — Task 7 skips it. If it builds providers in-process, it is another call site and Task 7 must wire it.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/notes/2026-08-04-autoflow-provider-path.md
git commit -m "docs: trace autoflow's agent execution path"
```

---

### Task 2: `netflow_dir` mirroring `transcripts_dir`

**Files:**
- Modify: `crates/rupu-cli/src/paths.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn netflow_dir(global: &Path, project_root: Option<&Path>) -> PathBuf` and `pub fn archived_netflow_dir(netflow_dir: &Path) -> PathBuf`. Task 6 calls `netflow_dir`; Plan 3's prune calls `archived_netflow_dir`.

**Read `transcripts_dir` at `crates/rupu-cli/src/paths.rs:35` first.** The new function must be the same shape — the two must not drift, and the fallback behaviour is load-bearing: it returns the project-local path only when that directory *already exists*, so a repo that was never `rupu init`'d never gets a ledger written inside it.

- [ ] **Step 1: Write the failing tests**

Add to `crates/rupu-cli/src/paths.rs`'s test module:

```rust
    #[test]
    fn netflow_dir_prefers_an_existing_project_local_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".rupu/netflow")).unwrap();

        assert_eq!(
            netflow_dir(&global, Some(&project)),
            project.join(".rupu/netflow")
        );
    }

    #[test]
    fn netflow_dir_falls_back_to_global_when_the_project_dir_does_not_exist() {
        // Load-bearing: a repo that was never `rupu init`'d must never get a
        // ledger written inside it. This is what closes the git-leak class
        // structurally rather than by patching ensure_dir.
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        assert_eq!(netflow_dir(&global, Some(&project)), global.join("netflow"));
    }

    #[test]
    fn netflow_dir_falls_back_to_global_with_no_project_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        assert_eq!(netflow_dir(&global, None), global.join("netflow"));
    }

    #[test]
    fn archived_netflow_dir_nests_under_the_netflow_root() {
        let dir = std::path::Path::new("/tmp/x/netflow");
        assert_eq!(archived_netflow_dir(dir), dir.join("archive"));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli netflow_dir`
Expected: FAIL to compile — `cannot find function netflow_dir`.

- [ ] **Step 3: Implement**

Add next to `transcripts_dir`:

```rust
/// Pick the netflow directory. Project-local when
/// `<project>/.rupu/netflow/` exists; global default otherwise.
///
/// Deliberately the same shape as [`transcripts_dir`] — a netflow ledger
/// has the same lifecycle as a transcript and the two resolutions must not
/// drift. The existence check is load-bearing: a repo that was never
/// `rupu init`'d falls back to global, so no ledger is ever written inside
/// a project that has not opted in.
pub fn netflow_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        let local = p.join(".rupu/netflow");
        if local.is_dir() {
            return local;
        }
    }
    global.join("netflow")
}

/// Archive directory nested under a netflow root.
pub fn archived_netflow_dir(netflow_dir: &Path) -> PathBuf {
    netflow_dir.join("archive")
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cli netflow_dir` then `cargo test -p rupu-cli archived_netflow`
Expected: PASS — 4 tests.

- [ ] **Step 5: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/paths.rs
git status --porcelain
git add crates/rupu-cli/src/paths.rs
git commit -m "feat(cli): netflow_dir mirroring the transcripts_dir resolution"
```

---

### Task 3: Per-run `NetflowPaths`

**Files:**
- Modify: `crates/rupu-netflow/src/ledger/paths.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `NetflowPaths::for_run(netflow_dir: &Path, run_id: &str) -> NetflowPaths` with fields `root: PathBuf` (the netflow directory) and `flows: PathBuf` (`<root>/{run_id}.jsonl`). `NetflowPaths::new` is removed. Tasks 6 and 7 construct via `for_run`; Plan 3's prune reads the directory.

The old `NetflowPaths::new(workspace)` produced one shared `<workspace>/.rupu/netflow/flows.jsonl`. That single-file model is what grows without bound, and what forces run attribution to be a record field rather than the file itself. Replace it.

- [ ] **Step 1: Write the failing test**

Replace the existing `paths_layout_under_dotrupu_netflow` test with:

```rust
    #[test]
    fn for_run_puts_each_run_in_its_own_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = NetflowPaths::for_run(tmp.path(), "run-a");
        let b = NetflowPaths::for_run(tmp.path(), "run-b");

        assert_eq!(a.root, tmp.path());
        assert_eq!(a.flows, tmp.path().join("run-a.jsonl"));
        assert_eq!(b.flows, tmp.path().join("run-b.jsonl"));
        assert_ne!(a.flows, b.flows, "two runs must never share a ledger");
    }
```

Keep `ensure_dir_is_idempotent` and every `.gitignore` test from #580, updating their construction to `NetflowPaths::for_run(tmp.path(), "run-1")`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-netflow for_run`
Expected: FAIL to compile — `no function or associated item named for_run`.

- [ ] **Step 3: Implement**

```rust
impl NetflowPaths {
    /// One ledger per run, mirroring how transcripts are laid out.
    ///
    /// `netflow_dir` comes from `rupu_cli::paths::netflow_dir`, which
    /// resolves project-local-when-present with a global fallback. The
    /// per-run file is what makes a ledger's lifecycle match a
    /// transcript's: it ends when the run ends, so there is nothing to
    /// rotate, and the file itself is the run attribution.
    pub fn for_run(netflow_dir: &Path, run_id: &str) -> Self {
        Self {
            root: netflow_dir.to_path_buf(),
            flows: netflow_dir.join(format!("{run_id}.jsonl")),
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        self.ensure_self_ignore()
    }
}
```

Delete `NetflowPaths::new`. Leave `ensure_self_ignore` exactly as #580 left it — the self-ignoring `.gitignore` stays as belt-and-braces for projects that do have `.rupu/netflow/`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-netflow`
Expected: PASS for this package. Call sites of the deleted `new` will fail to compile — that is intended; Tasks 6 and 7 fix them and the workspace goes green at Task 7.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/ledger/paths.rs
git status --porcelain
git add crates/rupu-netflow/src/ledger/paths.rs
git commit -m "feat(netflow): one ledger file per run"
```

---

### Task 4: Providers take an explicit sink

**Files:**
- Modify: `crates/rupu-netflow/src/http/mod.rs`
- Modify: `crates/rupu-providers/src/anthropic.rs`, `openai_compatible.rs`, `github_copilot.rs`, `google_gemini.rs`, `openai_codex.rs`, `local.rs`, `broker_client.rs`
- Modify: `crates/rupu-netflow/tests/capture.rs`

**Interfaces:**
- Consumes: `FlowSink`, `FlowCtx`, `Origin`.
- Produces: `http::client_with(ctx, builder, sink)` as the only client constructor. `http::client`, `http::client_from`, `http::init` and `http::sink()` are removed. Every provider constructor takes an `Arc<dyn FlowSink>`. Task 5 wires the factory; Task 7 supplies real sinks.

This is the heart of the change. `init`/`sink()` are removed rather than deprecated: once resolution is per-run a process-global sink has no correct use, and leaving it in place is a live footgun — the next provider added would reach for it and silently reintroduce cross-run attribution.

- [ ] **Step 1: Write the guard test**

Add to `crates/rupu-netflow/tests/capture.rs`:

```rust
#[tokio::test]
async fn two_clients_with_different_sinks_do_not_cross_contaminate() {
    // The regression this whole plan exists to prevent: under the old
    // process-global OnceLock the second sink was silently ignored and
    // both clients wrote to the first.
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/ping");
            then.status(200).body("pong");
        })
        .await;

    let sink_a = Arc::new(MemorySink::default());
    let sink_b = Arc::new(MemorySink::default());

    let client_a = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("a".into())),
        reqwest::Client::builder(),
        sink_a.clone(),
    )
    .unwrap();
    let client_b = rupu_netflow::http::client_with(
        FlowCtx::system(Origin::Provider("b".into())),
        reqwest::Client::builder(),
        sink_b.clone(),
    )
    .unwrap();

    client_a.get(server.url("/ping")).send().await.unwrap();
    client_b.get(server.url("/ping")).send().await.unwrap();

    assert_eq!(sink_a.records().len(), 1, "sink A saw only its own flow");
    assert_eq!(sink_b.records().len(), 1, "sink B saw only its own flow");
    assert_eq!(
        sink_a.records()[0].ctx.origin,
        Origin::Provider("a".to_string())
    );
    assert_eq!(
        sink_b.records()[0].ctx.origin,
        Origin::Provider("b".to_string())
    );
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p rupu-netflow --features http two_clients`
Expected: this may already PASS, because `client_with` already takes an explicit sink. That is fine — it is the guard that must survive the rest of this task. Note the result in your report and continue; the real work is removing the global.

- [ ] **Step 3: Remove the global from `http/mod.rs`**

Delete `static SINK`, `init`, `sink()`, `client`, `client_from`, and `complete`'s global lookup. What remains:

```rust
/// Build an instrumented client bound to an explicit sink.
///
/// There is deliberately no process-global sink. Resolution is per-run: a
/// long-lived process (`rupu session`, `rupu cp serve`) hosts many runs,
/// and a `OnceLock` would pin the first run's sink and route every later
/// run's flows into the first run's ledger and transcript. Callers thread
/// their run's sink through `provider_factory`.
pub fn client_with(
    ctx: FlowCtx,
    builder: reqwest::ClientBuilder,
    sink: Arc<dyn FlowSink>,
) -> reqwest::Result<ClientWithMiddleware> {
    // body unchanged
}
```

`complete` becomes a method on whatever owns the sink, or is dropped entirely if `FlowCompletionGuard` in `anthropic.rs` is its only consumer. Check which, and say so in your report.

- [ ] **Step 4: Give every provider constructor a sink**

Each provider currently resolves `http::sink()` internally. Change each to accept `sink: Arc<dyn FlowSink>`, store it, and pass it to `client_with`. `AnthropicClient` already has a `sink` field from the earlier fix wave — follow that shape for the other six.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-netflow --features http` then `cargo test -p rupu-providers`
Expected: PASS. `rupu-runtime` and `rupu-cli` will not compile yet — Task 5 and Task 7 fix that.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-netflow/src/http/mod.rs
git status --porcelain
git add crates/rupu-netflow/ crates/rupu-providers/
git commit -m "feat(netflow): remove the process-global sink; providers take one explicitly"
```

---

### Task 5: Thread the sink through `provider_factory`

**Files:**
- Modify: `crates/rupu-runtime/src/provider_factory.rs:226-262`

**Interfaces:**
- Consumes: provider constructors from Task 4.
- Produces: `build_for_provider_with_config(name, model, auth_hint, resolver, config, sink: Arc<dyn FlowSink>)` and `build_for_provider(name, model, auth_hint, resolver, sink)`. Task 7's call sites supply real sinks.

`build_for_provider` is a thin wrapper delegating to `build_for_provider_with_config`, so the parameter lands on the latter and the wrapper forwards it — one chokepoint, not two. Both take it explicitly rather than defaulting, so every call site must make a decision and none can silently fall back to dropping flows.

- [ ] **Step 1: Write the failing test**

Read the existing `rupu-runtime` factory tests first. If none of them drives a real HTTP call (the `RUPU_MOCK_PROVIDER_SCRIPT` seam at the top of `build_for_provider_with_config` short-circuits before a client is built), place this test in `crates/rupu-cli/tests/` alongside `netflow_capture.rs`, which already does, and say so in your report.

```rust
#[tokio::test]
async fn the_factory_binds_the_supplied_sink_to_the_built_provider() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET);
            then.status(200).body("{}");
        })
        .await;

    // The mock-provider seam short-circuits before a real client exists.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    let sink = std::sync::Arc::new(rupu_netflow::MemorySink::default());
    // Build through the real factory with this sink, drive one request,
    // and assert the flow landed in THIS sink — not a global one.
    // Model the construction on crates/rupu-cli/tests/netflow_capture.rs.

    assert_eq!(sink.records().len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-runtime the_factory_binds` (or `-p rupu-cli` if you placed it there)
Expected: FAIL to compile — the factory takes no sink.

- [ ] **Step 3: Implement**

```rust
pub async fn build_for_provider(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
    build_for_provider_with_config(
        name,
        model,
        auth_hint,
        resolver,
        &ProviderConfig::default(),
        sink,
    )
    .await
}

pub async fn build_for_provider_with_config(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
    config: &ProviderConfig,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
```

Pass `sink` into each provider constructor at its build site inside this function. Add `rupu-netflow = { workspace = true, features = ["http"] }` to `crates/rupu-runtime/Cargo.toml` if it is not already a dependency.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-runtime`
Expected: PASS for this crate. Downstream crates still break — Task 7.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-runtime/src/provider_factory.rs
git status --porcelain
git add crates/rupu-runtime/
git commit -m "feat(runtime): provider factory takes an explicit netflow sink"
```

---

### Task 6: A shared per-run sink builder

**Files:**
- Create: `crates/rupu-cli/src/netflow_sink.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (module declaration)
- Modify: `crates/rupu-cli/src/cmd/run.rs` (remove the local `build_netflow_sink` and `netflow_root`)

**Interfaces:**
- Consumes: `paths::netflow_dir` (Task 2), `NetflowPaths::for_run` (Task 3).
- Produces:

```rust
pub fn for_run(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
) -> (
    std::sync::Arc<dyn rupu_netflow::FlowSink>,
    Option<rupu_netflow::NetflowWriterHandle>,
)
```

Task 7's sink-building entry points — `run`, `session`, `step_factory` and `dispatch` — all use this. Extracting it is what stops each of them hand-rolling a subtly different sink.

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn two_runs_in_one_process_get_separate_ledgers() {
        // This is the test that would have failed under the OnceLock, and
        // the exact shape of `rupu session` and `rupu cp serve`.
        let tmp = tempfile::TempDir::new().unwrap();
        let global = tmp.path().join("global");
        let t_a = tmp.path().join("a.jsonl");
        let t_b = tmp.path().join("b.jsonl");
        rupu_transcript::JsonlWriter::create(&t_a).unwrap();
        rupu_transcript::JsonlWriter::create(&t_b).unwrap();

        let (sink_a, h_a) = for_run(&global, None, "run-a", &t_a);
        let (sink_b, h_b) = for_run(&global, None, "run-b", &t_b);

        rupu_netflow::FlowSink::record(sink_a.as_ref(), flow("a.example")).await;
        rupu_netflow::FlowSink::record(sink_b.as_ref(), flow("b.example")).await;
        if let Some(h) = h_a {
            h.shutdown().await;
        }
        if let Some(h) = h_b {
            h.shutdown().await;
        }

        let a = rupu_netflow::ledger::read_flows(&global.join("netflow/run-a.jsonl")).unwrap();
        let b = rupu_netflow::ledger::read_flows(&global.join("netflow/run-b.jsonl")).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].host, "a.example");
        assert_eq!(b[0].host, "b.example");
    }
```

Write the `flow(host)` helper by copying a `FlowRecord` literal from `crates/rupu-netflow/src/ledger/writer.rs`'s test module and parameterising its `host` field.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli two_runs_in_one_process`
Expected: FAIL to compile — module does not exist.

- [ ] **Step 3: Implement**

Move `build_netflow_sink`'s body from `run.rs` into the new module, renamed `for_run`, taking `global` / `project_root` / `run_id` instead of `pwd`, and constructing via `paths::netflow_dir` + `NetflowPaths::for_run`. Keep the existing degrade-on-error behaviour verbatim: a ledger that cannot be opened logs at debug and the run continues with transcript-only capture.

Delete `run.rs`'s `netflow_root` — the `pwd` fallback it existed for is now handled by `netflow_dir`'s own global fallback.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cli netflow`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/netflow_sink.rs
git status --porcelain
git add crates/rupu-cli/
git commit -m "feat(cli): shared per-run netflow sink builder"
```

---

### Task 7: Wire every agent-driven entry point

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`, `crates/rupu-cli/src/cmd/session.rs`, `crates/rupu-cli/src/cmd/dispatch.rs`, `crates/rupu-cli/src/cmd/cp.rs`
- Modify: `crates/rupu-orchestrator/src/step_factory.rs`, `crates/rupu-orchestrator/src/generate.rs`
- Modify: autoflow's path **only if Task 1 found it builds providers in-process**
- Test: `crates/rupu-cli/tests/netflow_workflow.rs` (create)

**Interfaces:**
- Consumes: `netflow_sink::for_run` (Task 6), the factory signature (Task 5).
- Produces: a workspace that compiles, with every agent path supplying its run's sink.

Every `build_for_provider*` call site now needs a sink argument — there are 8. For each, supply the sink belonging to *that* run.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/netflow_workflow.rs`. Model it on the existing `crates/rupu-cli/tests/netflow_run.rs`, which already sets `RUPU_HOME`, writes an auth file, points an `OpenAiCompatibleClient` at `httpmock`, and asserts on the ledger — reuse that setup wholesale.

```rust
#[tokio::test]
async fn a_workflow_agent_step_records_flows_to_its_own_ledger() {
    // Workflow runs currently capture nothing — this is the gap the plan
    // exists to close. Drive a workflow whose agent step hits a mock
    // server, then assert that run's ledger is non-empty.
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli a_workflow_agent_step`
Expected: FAIL — the workflow path installs no sink, so the ledger is absent or empty.

- [ ] **Step 3: Wire the call sites**

- `run.rs` — already builds a sink; switch it to `netflow_sink::for_run` and pass it to the factory.
- `session.rs` (3 sites) — build a sink **per turn/run**, not once at daemon start. This is the whole point; a daemon-lifetime sink is the bug.
- `step_factory.rs` (2 sites) — the workflow's run id is on the step context; build the sink there.
- `generate.rs` (2 sites) — definition generation is an inference call. Give it the invoking run's sink if one is reachable, otherwise `Arc::new(NullSink)`; say which you chose and why.
- `dispatch.rs` — sub-agents belong to their parent's run; pass the parent's sink.
- `cp.rs` — per the spec the daemon's own traffic is no longer recorded. Pass `Arc::new(NullSink)` and delete the `http::init` call and its `NetflowWriterHandle`.

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace` then `cargo build --workspace`
Expected: PASS. Investigate any failure before committing.

- [ ] **Step 5: Commit**

```bash
git status --porcelain
git add -A
git commit -m "feat: wire per-run netflow sinks through every agent path"
```

---

### Task 8: Delete what the per-run model makes redundant

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
- Modify: `crates/rupu-cp/web/src/components/netflow/ScopeDisclosure.tsx`, `ScopeDisclosure.test.tsx`

**Interfaces:**
- Consumes: per-run ledgers from Task 7.
- Produces: a smaller API surface. Plan 3 builds time filtering on it.

Because the file is now the attribution, several mechanisms exist only to compensate for its previous absence.

- [ ] **Step 1: Delete the ledger↔transcript merge**

`get_run_netflow` merges the ledger and the run transcript, deduping by `FlowId`, purely because `Origin::Scm` / auth / `System` flows carried `run_id: None` and would otherwise vanish from run scope. With a per-run sink those flows land in that run's ledger by construction. Delete `merge_with_transcript`, its dedupe, and the `filter_by_run` filtering around it. Run scope becomes: read `<netflow_dir>/{run_id}.jsonl`.

- [ ] **Step 2: Delete the global-ledger enumeration**

`read_all_workspaces_sync`'s union of `$RUPU_HOME/netflow/flows.jsonl` exists for run-less flows, which the spec no longer records. Global scope becomes: enumerate the per-run ledger files in the netflow directories.

- [ ] **Step 3: Correct the scope disclosure**

`ScopeDisclosure.tsx` names "CP fleet traffic" at global scope. That traffic is no longer recorded anywhere, so the claim comes out — at every scope, along with the `scope`-conditional branch that existed only to qualify it. This paragraph has already taken three rounds to make honest; touch it once, deliberately.

Update `ScopeDisclosure.test.tsx` to assert `/CP fleet traffic/i` is absent at **every** scope, replacing the current global-only assertion.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp`, then `cd crates/rupu-cp/web && npx vitest run`, then `cargo build --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git status --porcelain
git add -A
git commit -m "refactor(netflow): drop the merge, the global enumeration and the fleet-traffic claim"
```

---

## Done when

- `cargo test --workspace` and the full web suite pass; `cargo clippy --workspace --all-targets 2>&1 | grep disallowed` prints nothing.
- Two runs in one process write to separate ledgers — the test that would have failed under the `OnceLock`.
- A workflow agent step produces a non-empty ledger.
- `http::init` and `http::sink()` no longer exist.
- No ledger is written into a project that was never `rupu init`'d.
