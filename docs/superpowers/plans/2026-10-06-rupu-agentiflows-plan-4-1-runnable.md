# Agentiflows Plan 4-1 — Runnable (CLI `run` + provider wiring) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an agentiflow actually runnable from the CLI — `rupu agentiflow run <def>` loads a definition, builds the production provider + generation closures, assembles `RunAgentiflowOpts`, and runs the envelope to completion end-to-end; `list` and `status` read the run back. This is the gate for the rest of Plan 4 (`run_agentiflow` has zero production callers today).

**Architecture:** `run_agentiflow` is complete but caller-injected: `make_provider`, `generation`, credentials, the active profile set, and the run id are all supplied by a caller that does not exist yet. This sub-plan adds that caller in `rupu-cli` (a thin `cmd/agentiflow.rs`), reusing the exact provider-build machinery `rupu run` uses (`accounts::resolver_for` + `rupu_runtime::provider_factory`). The one hard problem is **how the two provider closures build a `Box<dyn LlmProvider>`**: they run in contexts where `.await`/`block_on` are unavailable (see the Binding ruling), so they must construct providers **synchronously from credentials resolved once at launch**.

**Tech Stack:** Rust 2021, tokio, async-trait, thiserror (libs) / anyhow (cli), serde/serde_yaml, clap, ULID. Crates touched: `rupu-agentiflow`, `rupu-runtime`, `rupu-cli`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` §18 (the def file + `AgentiflowDef::parse`), §19 (run model: run dir, `agentiflow.json`, `agentiflow.yaml` snapshot, `RunTriggerSource::Agentiflow`), §21 (CLI surface — `run`/`list`/`status` this sub-plan; `serve`/`attach`/`send`/`stop` are Plan 4-3), §22 (crate placement — thin `rupu-cli`).

## Global Constraints

- **Binding ruling (CORRECTED mid-execution — the two closures need DIFFERENT mechanisms).** Provider construction is NOT uniformly sync: Anthropic **OAuth** build `.await`s `bootstrap_oauth_session()` (`provider_factory.rs:640`, a live HTTP call), so it is irreducibly async. The two closures are at different call sites:
  - **`make_provider`** (`FnMut() -> Box<dyn LlmProvider>`, `lead.rs:220`, invoked synchronously at `lead.rs:366` on `run_agentiflow`'s plain `std::thread::scope` worker — NOT inside any tokio runtime): builds via the EXISTING async `build_for_provider_with_config` using a **dedicated tokio `Runtime` the closure owns + `block_on`**. Safe (nests nothing; the worker is a plain thread). Supports OAuth **and** gives per-round credential refresh. No sync split needed for it.
  - **`generation.factory`** (`Arc<dyn Fn() -> Box<dyn LlmProvider> + Send + Sync>`, `lead.rs:225`, invoked inside the `generate_workflow` tool's async `invoke`, which runs in the lead driver's **current-thread** runtime `RunAgentLeadDriver.rt`, `lead.rs:318,421` — where `block_on` AND `block_in_place` panic): MUST be synchronous. It uses Task 2's sync `build_provider_from_credential` over a credential pre-resolved once at launch. The sync fn cannot do the Anthropic-OAuth bootstrap, so when the generation provider needs it the launch path **sets `generation: None` + warns** (generate_workflow is simply unavailable under Anthropic-OAuth — degraded cleanly, never broken).
  - **Net v1 behavior:** API-key runs — everything incl. `generate_workflow` works. Anthropic-OAuth runs — the lead, dispatched units, and all goals work (lead uses block_on; units are separate `rupu run` processes with their own async auth); only `generate_workflow` is unavailable (tracked follow-up: TODO.md). `make_provider`'s block_on gives the lead per-round credential refresh (better than pre-resolve-once).
- **Thin CLI (rule #2).** `cmd/agentiflow.rs` is arg-parsing + delegation. Provider/credential assembly reuses `rupu_runtime::provider_factory` + `crate::accounts`; no business logic in the CLI crate beyond wiring.
- **Reuse, don't reinvent, the run-wiring.** Mirror `crates/rupu-cli/src/cmd/run.rs` `run_inner` (resolver → `ProviderConfig` → `build_for_provider_with_config` → run-id mint → netflow sink). Do not hand-roll provider resolution.
- **`#![deny(clippy::all)]`; no `unsafe`; `thiserror` in libs, `anyhow` only in `rupu-cli`; workspace-pinned deps; no new crate dep** (rupu-cli already deps rupu-agentiflow, rupu-runtime, rupu-auth, rupu-coverage).
- **Integration tests: one binary per crate** — modules under `crates/<c>/tests/it/` listed in `main.rs`; `rupu-cli` env/cwd-mutating tests go in `tests/serial/` holding `ENV_LOCK`, run with `< /dev/null`.
- **No regression to `rupu run` / session / workflow** provider wiring — the new sync construction path, if factored in `provider_factory`, must leave the existing async path behavior-identical.

## Binding ruling detail (read before Task 2)

> **CORRECTED during execution:** the uniform-sync reasoning below is SUPERSEDED by the Global-Constraints Binding ruling above. `make_provider` uses the existing async builder via `block_on` on a dedicated runtime (OAuth works); ONLY `generation.factory` is synchronous (and generate_workflow is disabled under Anthropic-OAuth). The call-site analysis below is still accurate and explains why.

Why synchronous construction is required for `generation.factory`, not merely preferred:
- `make_provider` call site (`lead.rs:366`): invoked while building `AgentRunOpts`, **before** `self.rt.block_on(run_agent_full(...))` at `lead.rs:421`, on `run_agentiflow`'s plain `std::thread::scope` worker (`run.rs:443`). A sync closure here *could* own a dedicated runtime and `block_on` — but see the next point.
- `generation.factory` call site: invoked inside `GenerateWorkflowTool::invoke` (async), which `run_agent_full` drives inside `RunAgentLeadDriver.rt` — a `new_current_thread()` runtime (`lead.rs:335`). `block_on`/`block_in_place` inside a current-thread runtime panic. So this closure **must** be synchronous.
- Both closures therefore construct synchronously. The approach: `provider_factory` exposes (or Task 2 factors) a **sync** `build_provider_from_credential(provider, model, &resolved_credential, &ProviderConfig) -> Result<Box<dyn LlmProvider>, _>` that is the non-I/O tail of `build_for_provider_with_config` (which is async only because it calls `resolver.get(...).await`). The launch path resolves the credential(s) once (async) and the closures call the sync tail. If `provider_factory`'s client construction cannot be cleanly separated from the async credential fetch, Task 2 STOPS and reports — do not fake it with `block_on`.
- **Empirical safety net:** Task 5's e2e runs a real `rupu agentiflow run` whose lead calls `generate_workflow` once (exercising `generation.factory` inside the current-thread runtime). A wrong bridge panics that test. The e2e is the guarantee; the ruling is the design.

## File Structure

- `crates/rupu-agentiflow/src/def.rs` (or a new `catalog`-style fn) — **modify/add.** `load_agentiflow_def(global, project, id)` — resolve `<project>/.rupu/agentiflows/<id>.yaml` shadowing `<global>/agentiflows/<id>.yaml` (mirroring `rupu_orchestrator::catalog::load_workflow`), parse via `AgentiflowDef::parse`.
- `crates/rupu-agentiflow/src/run.rs` — **modify.** Write the `agentiflow.yaml` definition snapshot into the run dir (§19:611 gap); mint the codename if cheap (else leave for 4-3).
- `crates/rupu-runtime/src/provider_factory.rs` — **modify.** Factor a sync `build_provider_from_credential(...)` (the non-I/O tail of `build_for_provider_with_config`); keep the async fn delegating to it after `resolver.get`.
- `crates/rupu-cli/src/cmd/agentiflow.rs` — **create.** The thin subcommand: `run` / `list` / `status`; the launch assembly (load def → resolve active → resolver → pre-resolve creds → sync provider/generation closures → mint `af_<ULID>` → `run_agentiflow`).
- `crates/rupu-cli/src/cmd/mod.rs`, `crates/rupu-cli/src/lib.rs` — **modify.** Register `cmd::agentiflow`; add `Cmd::Agentiflow { action }`, the dispatch arm, the `ensure_output_format_supported` arm, and `owns_live_terminal`/SIGTERM handling for `run`.
- Test modules: `crates/rupu-agentiflow/src/def.rs` tests; `crates/rupu-runtime/` tests; `crates/rupu-cli/tests/serial/` (the e2e).

---

### Task 1: Agentiflow def loader + `agentiflow.yaml` snapshot (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/def.rs` (add `load_agentiflow_def`) + `src/lib.rs` (re-export)
- Modify: `crates/rupu-agentiflow/src/run.rs` (write `agentiflow.yaml` snapshot next to `agentiflow.json`)
- Test: `crates/rupu-agentiflow/src/def.rs` tests + a run.rs test asserting the snapshot

**Interfaces:**
- Consumes: `AgentiflowDef::parse(&str)` (exists), `agentiflow_dir` (`run.rs:143`), the run-dir layout.
- Produces: `pub fn load_agentiflow_def(global: &Path, project: Option<&Path>, id: &str) -> Option<AgentiflowDef>` — resolves `<project>/.rupu/agentiflows/<id>.yaml` (when `project` is `Some`; `project` is the `.rupu` dir, matching `load_workflow`) shadowing `<global>/agentiflows/<id>.yaml`; `None` on not-found / parse-fail / a non-stem id (empty, `/`, `\`, `..`). Only `.yaml`. **Note:** `<global>/agentiflows/<id>/` is a RUN dir; the DEFINITION is `<global>/agentiflows/<id>.yaml` (a file beside the run dirs) — mirror `load_workflow`'s `workflows/<id>.yaml`, i.e. defs live in `<global>/agentiflows/*.yaml` and runs in `<global>/agentiflows/<af_ULID>/`. Confirm this directory convention against §18/§19 and keep defs and run dirs from colliding (run ids are `af_<ULID>`, def ids are names — they don't collide, but document it).

- [ ] **Step 1: Write the failing test** — `load_agentiflow_def` finds a global def by stem, a project def shadows it, a bad/missing id returns `None`:
```rust
#[test]
fn loads_agentiflow_def_by_stem_project_shadows_global() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("proj").join(".rupu");
    write(&global, "agentiflows/acme.yaml", VALID_DEF);                 // a parseable AgentiflowDef
    assert!(load_agentiflow_def(&global, None, "acme").is_some());
    write(&project, "agentiflows/acme.yaml", VALID_DEF_VARIANT);
    let got = load_agentiflow_def(&global, Some(&project), "acme").unwrap();
    assert_eq!(got.name, "<the project variant's name>");
    assert!(load_agentiflow_def(&global, None, "nope").is_none());
    assert!(load_agentiflow_def(&global, None, "../x").is_none());
}
```
(Use a minimal VALID_DEF that `AgentiflowDef::parse` accepts — copy the shape from an existing `AgentiflowDef::parse` test.)
- [ ] **Step 2: Run it, confirm it fails** (fn not found).
- [ ] **Step 3: Implement** `load_agentiflow_def` (mirror `rupu_orchestrator::catalog::load_workflow`'s stem-resolution + path-traversal guard). Re-export from `lib.rs`.
- [ ] **Step 4: agentiflow.yaml snapshot** — in `run_agentiflow`, right after the run dir is created / the record is first written (`run.rs:~502`), write `serde_yaml::to_string(&opts.def)` to `<run_dir>/agentiflow.yaml` (atomic tmp+rename like `AgentiflowRecord::write`). Add a test asserting the snapshot exists + re-parses to the same def after a run.
- [ ] **Step 5: Run tests** — `cargo test -p rupu-agentiflow --lib -- def:: run::` green; `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`.
- [ ] **Step 6: Commit** — `feat(agentiflow): load_agentiflow_def + agentiflow.yaml run snapshot`.

---

### Task 2: Sync provider construction from a resolved credential (`rupu-runtime`)

**Files:**
- Modify: `crates/rupu-runtime/src/provider_factory.rs`
- Test: `crates/rupu-runtime/` tests (or the provider_factory test module)

**Interfaces:**
- Consumes: the existing `build_for_provider_with_config(provider, model, auth_hint, resolver, provider_config, netflow_sink) -> Result<(AuthMode, Box<dyn LlmProvider>), _>` (`provider_factory.rs:395`, async) and the per-provider client constructors it calls; `rupu_auth`'s resolved credential type (`StoredCredential` / whatever `resolver.get` returns); `ProviderConfig`.
- Produces: a **sync** `pub fn build_provider_from_credential(provider: &str, model: &str, auth_hint: Option<..>, credential: &<resolved credential>, provider_config: &ProviderConfig, netflow_sink: Arc<dyn ..>) -> Result<(AuthMode, Box<dyn LlmProvider>), ProviderFactoryError>` — the non-I/O tail of the async fn. `build_for_provider_with_config` becomes: `let credential = resolver.get(...).await?; build_provider_from_credential(..., &credential, ...)`.

- [ ] **Step 1: Read `build_for_provider_with_config` fully** and identify the exact boundary between the async credential fetch (`resolver.get(...).await`) and the sync client construction (per-provider `Client::new(credential, config)` etc.). If the construction is entangled with other `.await`s (not just the credential fetch), STOP and report — the sync split is the whole task's premise.
- [ ] **Step 2: Write the failing test** — resolve a credential from an `InMemoryResolver` (test helper), then `build_provider_from_credential` returns a provider whose reported provider id matches; and `build_for_provider_with_config` still returns the identical result for the same inputs (behavior-preserving).
- [ ] **Step 3: Extract** the sync tail into `build_provider_from_credential`; rewrite the async fn to `resolver.get` then delegate. Preserve every existing behavior: `AuthMode` return, server-side-fallback config, provider-tuning, `openai_compatible_params`, the netflow sink, the exact error types.
- [ ] **Step 4: Run tests** — the new test + the EXISTING provider_factory tests green (behavior-preserving); `cargo test -p rupu-runtime`; `cargo clippy -p rupu-runtime --all-targets -- -D warnings`. Also `cargo build -p rupu-cli -p rupu-orchestrator` (the async fn's callers must be untouched).
- [ ] **Step 5: Commit** — `refactor(runtime): factor sync build_provider_from_credential out of build_for_provider_with_config`.

---

### Task 3: `rupu agentiflow run` — the launch assembly + dispatcher wiring (`rupu-cli`)

**Files:**
- Create: `crates/rupu-cli/src/cmd/agentiflow.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`, `crates/rupu-cli/src/lib.rs`
- Test: covered by Task 5's e2e (plus any pure-unit helper tests here)

**Interfaces:**
- Consumes: `load_agentiflow_def` (Task 1); `crate::accounts::resolver_for(&cfg)` (`accounts.rs:53`); `rupu_runtime::provider_factory::{build_provider_from_credential (Task 2), resolve_provider_name, resolve_model}`; the `ProviderConfig` assembly from `cmd/run.rs:881-891`; `rupu_agentiflow::{run_agentiflow, RunAgentiflowOpts, LeadInputs, new_run_id, GenerationCapability, GenerationProviderFactory, ProviderFactory}`; `rupu_coverage::builtin_registry().active_set(...)` for `active`; `rupu_orchestrator::generate::pick_default_gen_model` for the generation model.
- Produces:
  - `cmd/agentiflow.rs`: `pub enum Action { Run { def: String, #[arg(long)] input: Vec<(String,String)>, #[arg(long)] detach: bool, #[arg(long)] mode: Option<String> }, List, Status { id: String } }` and `pub async fn handle(action, format, absolute, all_columns) -> anyhow::Result<()>`. (`serve`/`attach`/`send`/`stop` are added in Plan 4-3 — leave them out of the enum now, or add them erroring "not yet" ONLY if clap ergonomics require; prefer leaving them out.)
  - `lib.rs`: `Cmd::Agentiflow { #[command(subcommand)] action: cmd::agentiflow::Action }` + the dispatch arm + the `ensure_output_format_supported` arm + inclusion in `owns_live_terminal` for `run`.
- The `run` launch assembly (the heart):
  1. Load layered config (`load_cli_config`), resolve `global` + `project` (`paths::global_dir` / `project_root_for`).
  2. `let def = load_agentiflow_def(&global, project_rupu.as_deref(), &def_name).ok_or(...)?;` apply `--input` (if the def supports inputs — check `AgentiflowDef`; if not, `--input` is reserved/ignored for now, document).
  3. `let active = rupu_coverage::builtin_registry().active_set(&def.engagement_profiles)?;` then `def.validate(&active)?` early (fail-closed before building anything).
  4. `let resolver = Arc::new(accounts::resolver_for(&cfg));` resolve the lead's provider/model (`resolve_provider_name`/`resolve_model` from def.lead's agent frontmatter or config default) and the generation provider/model (`pick_default_gen_model(&resolver).await`).
  5. Build the `ProviderConfig` (mirror `cmd/run.rs:881-891`). For the LEAD: pre-validate once (async) by building a provider via `build_for_provider_with_config` and erroring at launch if it fails (so the per-round closure never panics on a config/auth error); then drop it. For GENERATION: `resolver.get(gen_provider, ..).await?` the gen credential + AuthMode once.
  6. Build the closures per the corrected Binding ruling:
     - `make_provider = Box::new({ let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?; let resolver = resolver.clone(); let pc = cfg_pc.clone(); let sink = sink.clone(); move || rt.block_on(build_for_provider_with_config(&lead_provider, &lead_model, hint, resolver.as_ref(), &pc, sink.clone())).expect("lead provider build (pre-validated at launch)").1 })` — a dedicated runtime owned by the closure, `block_on` the EXISTING async builder each round (OAuth + per-round refresh). The `expect` is safe because step 5 pre-validated; still, prefer surfacing a round failure over a panic if the closure contract allows.
     - `generation`: call `build_provider_from_credential(&gen_provider, &gen_model, gen_auth_mode, &gen_cred, refresher, &cfg_pc_gen, sink.clone())` ONCE at launch to classify. If it returns `Ok(_)`, set `generation = Some(GenerationCapability { provider: gen_provider, model: gen_model, factory: Arc::new(move || build_provider_from_credential(..).expect("pre-validated").1) })` (sync, no runtime). If it returns `Err(RequiresAsyncBootstrap { .. })` (Anthropic-OAuth), set `generation = None` and log a warning that `generate_workflow` is unavailable under OAuth for `<gen_provider>` (use an API key to enable it). Any other error → fail the launch.
  7. Assemble `LeadInputs` (agent_name from `def.lead`, system_prompt from the lead agent's file, provider_name/model, agent_tools from the lead agent frontmatter), `workspace` (cwd or `def.workspace`), `run_id = new_run_id()`, `started`/`now`, `unit_launcher: None` (prod default).
  8. Call `run_agentiflow(opts)` from a BLOCKING context (`tokio::task::spawn_blocking`, per `lead.rs:300-305`) for the foreground run; print the run id + the `EnvelopeOutcome` (stop reason, per-goal pass/fail). `--detach` MINIMAL for now: spawn a detached `rupu agentiflow run <def> ...` child with stdio null and return the run id immediately (full supervised `serve` is Plan 4-3); if minimal detach is awkward, defer `--detach` to 4-3 and document.

- [ ] **Step 1** (no TDD red for the pure wiring; it's covered by Task 5's e2e): implement the dispatcher touch-points first (`Cmd::Agentiflow` + arm + format-gate + live-terminal), with a stub `handle` that compiles, and confirm `rupu agentiflow --help` lists `run`/`list`/`status`.
- [ ] **Step 2** implement the `run` launch assembly per the interface above. Resolve each unknown (lead agent file load, workspace choice) against `cmd/run.rs`'s precedent; where a decision is ambiguous, pick the `rupu run` default and note it.
- [ ] **Step 3** build: `cargo build -p rupu-cli`; `rupu agentiflow run --help` shows the flags. Clippy clean.
- [ ] **Step 4: Commit** — `feat(cli): rupu agentiflow run — launch an agentiflow (sync provider closures)`.

---

### Task 4: `rupu agentiflow list` + `status <id>` (`rupu-cli`)

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agentiflow.rs`
- Test: a unit test over a seeded `<global>/agentiflows/` dir (+ Task 5 exercises them live)

**Interfaces:**
- Consumes: `agentiflow_dir(&global)` (`run.rs:143`), `AgentiflowRecord` (read `agentiflow.json` from each `<global>/agentiflows/<af_*>/`), `GoalStatus`.
- Produces: `list` — read each run dir's `agentiflow.json`, print newest-first (id, name, status, stop_reason, rounds, goals pass/fail count), honoring `--format`. `status <id>` — read one record + surface goals (predicate + pass/fail), budget state (from the record), coverage state, round count, stop reason. Reuse the crate's table/JSON output helpers.

- [ ] **Step 1: Write the failing test** — seed two `<global>/agentiflows/af_*/agentiflow.json` records; `list` returns both newest-first; `status <id>` returns the one record's goals/stop. (Drive the internal list/status fns directly, not the stdout.)
- [ ] **Step 2: Run it, confirm it fails.**
- [ ] **Step 3: Implement** `list` + `status` reading the run dir. Skip dirs without a parseable `agentiflow.json` (tolerant, like the run listers). Use `FileCache` if the crate's listers do (per the CP load-time lesson) — otherwise a plain read is fine for the CLI (not a per-request server path).
- [ ] **Step 4: Run tests** — `cargo test -p rupu-cli -- agentiflow < /dev/null`; clippy clean.
- [ ] **Step 5: Commit** — `feat(cli): rupu agentiflow list + status`.

---

### Task 5: End-to-end — `run` → `list` → `status` with a mock provider (`rupu-cli`)

**Files:**
- Create: `crates/rupu-cli/tests/serial/agentiflow_run.rs` (listed in `tests/serial/main.rs`)

**Interfaces:**
- Consumes: the whole launch path; `RUPU_MOCK_PROVIDER_SCRIPT` (the mock provider the other serial tests use — hold `ENV_LOCK`, run `< /dev/null`); a minimal agentiflow def + lead agent + a 1-agent pool written into a tempdir `RUPU_HOME`/project.
- Produces: a serial test proving the bridge and the full path.

- [ ] **Step 1: Write the test.** In an isolated `RUPU_HOME` + cwd (tempdir), write: an engagement-profile-free (or `code`) def `acme.yaml` with a trivial goal that the lead can satisfy (or a tiny `rounds`/`wall_clock` ceiling so it terminates fast), a lead agent file, and a 1-agent pool. Script the mock lead provider to (a) call `generate_workflow` once with a tiny description (exercising `generation.factory` INSIDE the lead's current-thread runtime — **this is the bridge's safety net; a wrong closure panics here**), then (b) produce a final answer. Run `rupu agentiflow run acme` to completion; assert it exits Ok, `agentiflow.json` + `agentiflow.yaml` + `events.jsonl` exist, the stop reason is as expected, and `list`/`status` report the run. If driving `generate_workflow` end-to-end is too heavy, the MINIMUM is: the lead run completes a round with a mock provider built by `make_provider` (proving the sync `make_provider` closure works in the real launch), plus a focused test that `generation.factory` builds a provider when invoked from inside a current-thread runtime (a direct unit test of the closure under `Runtime::new_current_thread().block_on(async { (factory)() })` — which must NOT panic).
- [ ] **Step 2: Run it, confirm it fails** (the subcommand/wiring not complete until Tasks 1-4).
- [ ] **Step 3:** make it pass (it exercises Tasks 1-4 together).
- [ ] **Step 4: Run** `cargo test -p rupu-cli --test serial agentiflow_run < /dev/null`; then `cargo test -p rupu-cli < /dev/null` (whole crate) + `cargo clippy -p rupu-agentiflow -p rupu-runtime -p rupu-cli --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** — `test(cli): agentiflow run→list→status e2e (mock provider, generation closure in current-thread rt)`.

---

## Self-Review

**Spec coverage:** §18 def loader → Task 1; §19 `agentiflow.yaml` snapshot → Task 1 (the rest of §19 — usage.jsonl fold, runner_pid, RunStore/parent — are Plans 4-2/4-3); §21 `run`/`list`/`status` → Tasks 3/4 (`serve`/`attach`/`send`/`stop` are 4-3); §22 thin CLI → Task 3. The provider-closure construction (the un-wired crux) → Task 2 + the Binding ruling, proven by Task 5.

**Placeholder scan:** none — each task has concrete interfaces; Task 2 and Task 3 each carry an explicit STOP-and-report if the provider_factory sync split or the lead-agent-load can't be done cleanly, rather than a fake.

**Type consistency:** `build_provider_from_credential` (Task 2) is the exact fn the Task 3 closures call; `GenerationCapability { provider, model, factory }` and `ProviderFactory`/`GenerationProviderFactory` (from `rupu-agentiflow::lead`, re-exported) are what Task 3 builds and `run_agentiflow` consumes; `load_agentiflow_def` (Task 1) → `AgentiflowDef` → `run_agentiflow` (Task 3); `AgentiflowRecord`/`agentiflow_dir` (Task 1/existing) → `list`/`status` (Task 4).

**Verify-before-commit:** Tasks 1/2/4 end green on their crate scope; Task 5 is the cross-crate e2e that empirically proves the two closures build providers in their real (sync and async-current-thread) call sites without panicking — the single most important check. The full suite is the release gate on `main`.
