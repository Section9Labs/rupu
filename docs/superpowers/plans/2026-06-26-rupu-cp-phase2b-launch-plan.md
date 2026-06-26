# rupu-cp Phase 2b — Launch runs (subprocess) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Launch a workflow run from the web via a spawned `rupu` subprocess; retrofit the resume worker to subprocess (cancellable). Hexagonal: rupu-cp defines the `RunLauncher` port, rupu-cli the `SubprocessLauncher` adapter.

**Spec:** `docs/superpowers/specs/2026-06-26-rupu-cp-phase2b-launch-design.md`

**Constraints:** no `any` (TS); static Tailwind; recharts out of main chunk; stage only specific files (`git add <paths>`, never `-A`, never `.rupu/*`); never package-wide `cargo fmt`. **Toolchain:** worktree Rust 1.95 — `rupu-orchestrator`/`rupu-cp`/web are clean gates; `rupu-cli` has a pre-existing red TEST baseline (verify it *compiles*; CI on 1.88 authoritative).

**PR split:** Tasks 1–5 = **2b-backend** PR. Task 6 = **2b-frontend** PR. (No version cut until all of Phase 2.)

---

### Task 1: CLI — `rupu workflow run --run-id <id>`

**Files:** Modify `crates/rupu-cli/src/cmd/workflow.rs`.

**Context:** `Action::Run` (line ~169) → the run handler builds `OrchestratorRunOpts` (which has `run_id_override: Option<String>`, runner.rs:154). Read how `Action::Run` dispatches (the `run_by_name`/`run` path) and where the opts are assembled.

- [ ] **Step 1:** Add `#[arg(long)] run_id: Option<String>` to `Action::Run`. Thread it into the `OrchestratorRunOpts.run_id_override` the run path builds (find where opts are constructed for a fresh run; set `run_id_override = run_id`). No behavior change when `None`.
- [ ] **Step 2:** `cargo build -p rupu-cli` compiles. Manual sanity: `rupu workflow run --help` shows `--run-id`.
- [ ] **Step 3: Commit.** `git add crates/rupu-cli/src/cmd/workflow.rs` → `feat(cli): workflow run --run-id (pre-assign the run id)`.

---

### Task 2: rupu-cp — `RunLauncher` port + AppState/ServeOpts wiring

**Files:** Create `crates/rupu-cp/src/launcher.rs`; Modify `crates/rupu-cp/src/lib.rs` (module + ServeOpts + serve), `crates/rupu-cp/src/state.rs` (AppState field).

**Context:** `ServeOpts { bind, token, global_dir, open_browser }` (lib.rs:22); `serve` builds `AppState::new(global_dir, pricing)` (lib.rs:81). `AppState` (state.rs) holds `global_dir`, `run_store`, `pricing`, etc. — read it.

- [ ] **Step 1:** `launcher.rs`:
  ```rust
  use std::collections::BTreeMap;
  #[derive(Debug, Clone)] pub struct LaunchRequest { pub workflow: String, pub inputs: BTreeMap<String,String>, pub mode: Option<String>, pub target: Option<String> }
  #[derive(Debug, thiserror::Error)] pub enum LaunchError { #[error("{0}")] Spawn(String), #[error("{0}")] Invalid(String) }
  #[async_trait::async_trait] pub trait RunLauncher: Send + Sync { async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError>; }
  ```
  (Add `async-trait` to rupu-cp deps if absent — check Cargo.toml; orchestrator already uses it so it's a workspace dep.)
- [ ] **Step 2:** `AppState` gains `pub launcher: Option<std::sync::Arc<dyn crate::launcher::RunLauncher>>`. `AppState::new` keeps its current signature but defaults `launcher: None`; add a builder `with_launcher(mut self, l) -> Self` (or a new constructor) so `serve` can set it. Fix existing `AppState::new` call sites/tests (they default to `None`).
- [ ] **Step 3:** `ServeOpts` gains `pub launcher: Option<Arc<dyn RunLauncher>>`. `serve` does `AppState::new(...).with_launcher(opts.launcher)`. `pub mod launcher;` in lib.rs.
- [ ] **Step 4:** `cargo build -p rupu-cp` + `cargo test -p rupu-cp` compile/green (existing ServeOpts/AppState constructors updated). Commit. `git add crates/rupu-cp/src/launcher.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/state.rs` → `feat(cp): RunLauncher port + AppState/ServeOpts wiring`.

---

### Task 3: rupu-cp — launch endpoint

**Files:** Modify `crates/rupu-cp/src/api/workflows.rs` (+ `server.rs` if routes are registered there).

**Context:** Read `api/workflows.rs` (the existing GET workflow handlers + how it lists/locates workflows) and how routes are merged in `server.rs`.

- [ ] **Step 1: Write failing tests.** With a **mock `RunLauncher`** (a test impl capturing the `LaunchRequest` + returning a fixed run_id), assert: `POST /api/workflows/:name/run` with `{inputs, mode, target}` calls `launch` with the matching `LaunchRequest` and returns 202 `{ run_id }`; with `launcher = None` → 501.
- [ ] **Step 2:** Implement `async fn launch_run(State(s), Path(name), body: Option<Json<LaunchBody>>) -> ApiResult<Json<...>>` where `LaunchBody { #[serde(default)] inputs: BTreeMap<String,String>, #[serde(default)] mode: Option<String>, #[serde(default)] target: Option<String> }`. If `s.launcher` is `None` → `ApiError` 501 (add a `service_unavailable`/501 constructor in error.rs if absent). Else `launcher.launch(LaunchRequest{ workflow: name, inputs, mode, target }).await` → 202 `{ run_id }`; map `LaunchError::Invalid` → 400, `Spawn` → 500. Register `.route("/api/workflows/:name/run", post(launch_run))`.
- [ ] **Step 3:** `cargo test -p rupu-cp` + clippy clean. Commit. `git add crates/rupu-cp/src/api/workflows.rs crates/rupu-cp/src/server.rs crates/rupu-cp/src/error.rs` → `feat(cp): POST /api/workflows/:name/run launch endpoint`.

---

### Task 4: rupu-cli — `SubprocessLauncher` + wire into `cp serve`

**Files:** Create `crates/rupu-cli/src/cp_launcher.rs`; Modify `crates/rupu-cli/src/cmd/cp.rs` (+ lib.rs module decl).

**Context:** `cmd/cp.rs` builds `global_dir` and calls `rupu_cp::serve(ServeOpts{...})`. The CP run-id format is `run_<ULID>` — find the existing run-id generator (`crates/rupu-orchestrator` likely has one, e.g. a `new_run_id()`/ULID helper used by `run_workflow`); reuse it so the pre-generated id matches what the store expects.

- [ ] **Step 1:** `cp_launcher.rs`: `pub struct SubprocessLauncher { exe: PathBuf, global_dir: PathBuf }` impl `rupu_cp::launcher::RunLauncher`:
  - `launch`: generate `run_<ULID>`; build argv `["workflow","run",&req.workflow,"--run-id",&id,"--plain"]` + for each input `--input k=v` + `mode` → `--mode <m>` + `target` (if Some) as the positional `[TARGET]`; `std::process::Command::new(&self.exe).args(argv).spawn()` (detached — do NOT wait; the child writes its own run state). On spawn error → `LaunchError::Spawn`. Return the id.
  - Factor the **argv construction into a pure fn** `build_run_argv(&self, req, run_id) -> Vec<String>` so it's unit-testable without spawning.
- [ ] **Step 2: Wire into `cp serve`.** In `cmd/cp.rs`, build `let launcher = Arc::new(SubprocessLauncher { exe: std::env::current_exe()?, global_dir: global_dir.clone() });` and pass `launcher: Some(launcher)` into `ServeOpts`.
- [ ] **Step 3: Test** `build_run_argv` (a request with 2 inputs + mode bypass + a target → the exact argv incl. `--run-id`, both `--input`, `--mode bypass`, and the target positional). `cargo build -p rupu-cli` compiles; `cargo test -p rupu-cli cp_launcher` (the pure argv test) passes if runnable on 1.95.
- [ ] **Step 4: Commit.** `git add crates/rupu-cli/src/cp_launcher.rs crates/rupu-cli/src/cmd/cp.rs crates/rupu-cli/src/lib.rs` → `feat(cli): SubprocessLauncher spawns workflow-run children; wired into cp serve`.

---

### Task 5: rupu-cli — resume worker spawns a subprocess

**Files:** Modify `crates/rupu-cli/src/cmd/cp.rs` (`run_resume_worker`).

**Context:** The worker today: claim → `store.approve(...)` → in-process `resume::resume_run(...)` → `clear_resume`. Change to spawn `rupu workflow approve <id> [--mode <m>]` as a child (which does approve+resume in its own process → killable `runner_pid`).

- [ ] **Step 1:** On a claimed pending-resume run: read `resume_mode` (as now), then **spawn** `Command::new(std::env::current_exe()?).args(["workflow","approve",&run_id])` + `["--mode", m]` if a mode is set; **do not** call `store.approve` or `resume::resume_run` in-process anymore (the child does both). After a successful spawn, `store.clear_resume(&run_id, now)` (the child owns the run). On spawn error → log + `clear_resume` (don't retry forever). Keep the claim/lease + the per-run `tokio::spawn` wrapper (so spawning many is non-blocking). The worker no longer needs `resume::resume_run` — leave the `resume` module (used by the CLI `workflow approve` the child runs).
- [ ] **Step 2:** `cargo build -p rupu-cli` compiles. Commit. `git add crates/rupu-cli/src/cmd/cp.rs` → `refactor(cli): resume worker spawns workflow-approve subprocess (cancellable resumes)`.

---

### Task 6 (2b-frontend PR): Launcher UI

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`; create a `LauncherSheet` component; add a Run button + nav wiring; fetch workflow inputs.

- [ ] **Step 1:** `api.ts`: `launchRun(workflow: string, opts: { inputs?: Record<string,string>; mode?: 'ask'|'bypass'|'readonly'; target?: string }): Promise<{ run_id: string }>` → `POST /api/workflows/${workflow}/run`. Confirm the workflow-definition fetch returns declared `inputs` (extend the type if needed).
- [ ] **Step 2:** `components/LauncherSheet.tsx` — a modal: workflow name (prop), an inputs form (a field per declared input; free-form key/value fallback), a mode picker (Ask/Bypass/Read-only, default Ask), an optional target text field. **Launch** → `launchRun(...)` → on success `navigate('/runs/' + run_id)`. Loading/disable/inline-error. Static Tailwind; no `any`.
- [ ] **Step 3:** A **Run** button on the Workflows list rows + workflow detail (opens the sheet pre-set to that workflow). Optional: a primary "New run" entry.
- [ ] **Step 4: Test** (`LauncherSheet.test.tsx`): filling inputs + picking Bypass + Launch calls `launchRun(name, { inputs, mode:'bypass' })`. `npm test -- --run` + `npm run build` green; recharts grep = 0.
- [ ] **Step 5: Commit.** `git add <the files>` → `feat(cp/web): workflow launcher sheet + Run button`.

---

### Final verification (per PR)
- Backend: `cargo test -p rupu-orchestrator -p rupu-cp` green; clippy clean; `cargo build -p rupu-cli` compiles.
- Frontend: `npm test -- --run` green; `npm run build` strict; recharts out of main chunk.
- Final whole-branch review (the launch chain endpoint→port→subprocess argv; the resume retrofit; 501 when no launcher; cancel now kills launched/resumed children), then matt visual-validates.
