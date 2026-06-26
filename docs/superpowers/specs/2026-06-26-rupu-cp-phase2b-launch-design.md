# rupu-cp — Phase 2b: Launch runs from the web (subprocess execution) — Design

**Date:** 2026-06-26
**Surfaces:** `rupu-cli` (`--run-id` flag, subprocess launcher, resume retrofit), `rupu-cp` (RunLauncher port + launch endpoint), `rupu-cp/web` (launcher UI)
**Status:** pending matt's spec review
**Decision (matt):** cp-serve executes web-launched/resumed runs as **subprocesses** (spawn the `rupu` binary), for isolation + cancellability.

## Goal
Start a workflow run from the browser, and make all cp-serve-driven execution run as **child `rupu` processes** so runs are cancellable (real PIDs) and isolated from the control plane. Retrofit the existing in-process resume worker to the same model (fixing the Phase-2a cancel limitation).

## Execution model: subprocess
`cp serve` is the `rupu` binary, so it spawns itself via `std::env::current_exe()`:
- **Launch** → `rupu workflow run <name> --run-id <id> [--input k=v]… [--mode m] [<target>] --plain`.
- **Resume** (retrofit) → `rupu workflow approve <run_id> [--mode m]`.
Each child's `run_workflow` stamps `runner_pid = its own pid`, so `RunStore::cancel` (Phase 2a) SIGTERMs a *separate, killable* process. A child crash/OOM cannot take down `cp serve`.

## ① CLI — `rupu workflow run --run-id <id>`
`crates/rupu-cli/src/cmd/workflow.rs` `Action::Run` gains `--run-id: Option<String>`, threaded into `OrchestratorRunOpts.run_id_override` (the field already exists). Lets the launcher pre-generate the run id so the web response can return it immediately and the UI can navigate straight to the new run. (No behavior change when absent.)

## ② RunLauncher port (rupu-cp) + SubprocessLauncher (rupu-cli)
Hexagonal: rupu-cp defines the **port**, rupu-cli provides the **adapter** (keeps rupu-cp free of process-spawning business logic).

- `crates/rupu-cp/src/launcher.rs` (new):
  ```rust
  pub struct LaunchRequest { pub workflow: String, pub inputs: BTreeMap<String,String>, pub mode: Option<String>, pub target: Option<String> }
  #[async_trait] pub trait RunLauncher: Send + Sync {
      async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError>; // returns run_id
  }
  ```
  `AppState` gains `launcher: Option<Arc<dyn RunLauncher>>` (None ⇒ read-only deploy ⇒ launch endpoint returns 501). `ServeOpts` gains `launcher: Option<Arc<dyn RunLauncher>>`; `serve` passes it to `AppState::new`.
- `crates/rupu-cli/src/cp_launcher.rs` (new): `SubprocessLauncher { exe: PathBuf, global_dir: PathBuf }` implements `RunLauncher::launch` by (a) generating a `run_<ULID>` id, (b) spawning `Command::new(&self.exe).args(["workflow","run",&req.workflow,"--run-id",&id, …inputs…, mode, target, "--plain"])` **detached** (spawn, don't await — the child writes its own `run.json`/events), (c) returning the id. Validation (workflow exists, inputs) is the child's job; a spawn failure → `LaunchError`. `cmd/cp.rs` constructs it (`current_exe()` + `global_dir`) and passes it into `ServeOpts`.

## ③ Launch endpoint (rupu-cp)
`POST /api/workflows/:name/run` (or `/api/runs` with `{workflow}` — pick the RESTful `/workflows/:name/run`). Body `{ inputs?: Record<string,string>, mode?: "ask"|"bypass"|"readonly", target?: string }`. Handler: if `state.launcher` is `None` → 501 ("control actions require `rupu cp serve`"); else `launcher.launch(LaunchRequest{ workflow: name, inputs, mode, target }).await` → 202 `{ run_id }`. Maps `LaunchError` → 400/500. The new run appears in the run list/SSE as the child writes it.

## ④ Resume retrofit (rupu-cli) — subprocess instead of in-process
`crates/rupu-cli/src/cmd/cp.rs` `run_resume_worker`: on a claimed pending-resume run, instead of `store.approve(...)` + in-process `resume::resume_run(...)`, **spawn `rupu workflow approve <run_id> [--mode <resume_mode>]`** as a child (it does approve+resume in its own process → killable `runner_pid`). Then `clear_resume` once the child is spawned (the child owns the run from there). The worker no longer links the heavy resume runtime in its own process. `resume::resume_run` stays — it's what the spawned `workflow approve` uses internally. This is what fixes the Phase-2a "can't cancel an in-process resume" limitation: a resumed run is now a separate process.

## ⑤ Launcher UI (rupu-cp/web)
- `api.ts`: `launchRun(workflow, { inputs?, mode?, target? }): Promise<{ run_id: string }>` → `POST /api/workflows/:name/run`.
- A **Run** affordance: a toolbar/“New run” button (and a Run button on each Workflow row / the workflow detail page) opens a **Launcher sheet** (modal), mirroring rupu-app's launcher:
  - Workflow (pre-selected when launched from a workflow row; else a picker).
  - **Inputs form** — render a field per the workflow's declared `inputs` (fetch the workflow definition, which carries `inputs`). Free-form key/value fallback if none declared.
  - **Mode** picker — Ask / Bypass / Read-only (default Ask).
  - **Target** — optional text field (a repo/PR/issue ref like `github:owner/repo`; `workflow run` clones/fetches as needed). Default: this workspace (empty).
  - **Launch** → `launchRun(...)` → on success, navigate to `/runs/:run_id` (live view). Disable + spinner while launching; inline error on failure.
- Nav: a primary "Run" action in the Workflows area (and/or the sidebar). Static Tailwind; no `any`.

## Files
**rupu-cli:** `cmd/workflow.rs` (`--run-id`), `cp_launcher.rs` (new SubprocessLauncher), `cmd/cp.rs` (construct launcher + pass to ServeOpts; resume worker spawns subprocess).
**rupu-cp:** `launcher.rs` (new port), `state.rs` (+launcher field), `lib.rs` (ServeOpts +launcher, pass through), `api/workflows.rs` or `api/runs.rs` (launch endpoint), `server.rs` (route).
**rupu-cp/web:** `lib/api.ts` (`launchRun`), a `LauncherSheet` component, a Run button + nav wiring, the workflow-inputs fetch.

## Phasing (plan into PRs; no version cut until all of Phase 2)
- **2b-backend:** `--run-id` + RunLauncher port + SubprocessLauncher + launch endpoint + **resume retrofit**. Shippable/testable via curl; fixes the 2a cancel limitation.
- **2b-frontend:** the Launcher sheet + Run button.

## Testing
- `rupu-cli`: compiles; `--run-id` threads to `run_id_override`; SubprocessLauncher builds the right argv (unit-test the argv construction without actually spawning); resume worker spawns the approve subprocess (test the command construction). CI (1.88) for full cli tests.
- `rupu-cp`: launch endpoint → 501 when no launcher; with a mock `RunLauncher`, calls `launch` with the right `LaunchRequest` and returns the run_id; route registered. clippy clean.
- web: `launchRun` client; LauncherSheet renders inputs/mode/target + calls `launchRun`; suite green; build strict; no `any`.
- Visual validation by matt: launch a workflow from the browser → a new run appears and goes live; cancel a launched (or resumed) run → it actually stops (separate PID).

## Non-goals / deferred
- No clone-target picker UI (just a target text field); the rupu-app-style RepoRef clone flow can come later.
- No run-queue/concurrency caps for launches (v1 spawns immediately).
- Retry/re-run of a finished run = a follow-up (it's a launch with the same inputs, or `workflow resume` for failed) — not in 2b.
