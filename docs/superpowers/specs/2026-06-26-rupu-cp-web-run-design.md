# rupu CP web — Run (dispatch) + Cancel, with smart target pickers

Date: 2026-06-26
Status: approved (design)

## Problem

The CP web UI is read-only: you can view workflow/agent definitions and watch
runs, but you cannot **start** a run from the browser, nor cancel one. We want a
"Run" experience in the web CP for workflows and agents, with a smart target
picker (existing project / directory with browse + fuzzy-complete / repository
with fuzzy-complete from the logged-in repo list), and the ability to cancel an
in-flight run.

## Approach (decided in brainstorming)

**Spawn the existing CLI as a detached process** — the CP backend launches
`rupu workflow run …` (or `rupu run <agent> …`) as its own OS process, exactly
like a user typing the command. The run writes `run.json` / `events.jsonl` to
`<global>/runs/<id>/`, which the CP already tails for the graph / transcript /
approval UI. Rationale:

- Reuses the entire battle-tested run path — no embedded executor, no dispatch
  queue, no new worker loop. (Supersedes an earlier marker+worker idea.)
- The run is an independent process: **if the CP is closed or crashes, the run
  keeps going**; the CP reads its on-disk state when it returns.
- The CLI already clones `repo` / `PR` / `issue` targets to a tmpdir itself, so
  the repository target needs **no clone logic on the CP side**.
- The CP stays a thin launcher/viewer — it never runs the orchestrator
  in-process, preserving the read-only-runtime principle.

The CP serve resume worker (approvals) is unchanged and unrelated.

## Phasing

- **A — Run + Cancel foundation (this implements first).** Workflow Run from the
  web with a *basic* target (existing project / directory path / repo-ref text),
  plus Cancel. End-to-end working.
- **B — Smart target pickers.** Directory browse endpoint + directory
  fuzzy-complete from past projects; repository fuzzy-complete from the
  logged-in repo list. Layered onto A's form.
- **C — Agent Run.** Same spawn+cancel path for `rupu run <agent>` from
  AgentDetail (prompt + mode + target). Cancel comes for free.

Each phase is its own plan + PR. This spec details A fully and scopes B and C.

---

## Already built (discovered during design — do NOT rebuild)

The backend launch + cancel machinery already exists:
- `rupu workflow run` already accepts `--run-id` (`workflow.rs` `Run` variant)
  and `RunStatus::Cancelled` already exists.
- `rupu-cp` defines a `RunLauncher` port (`crates/rupu-cp/src/launcher.rs`):
  `LaunchRequest { workflow, inputs: BTreeMap<String,String>, mode: Option<String>,
  target: Option<String> }` → `launch() -> Result<String /*run_id*/, LaunchError>`.
  `AppState.launcher: Option<Arc<dyn RunLauncher>>`; `ApiError::not_available()`
  (501) is the "no launcher (read-only `rupu cp`)" response.
- `rupu-cli` provides the adapter `SubprocessLauncher` (`crates/rupu-cli/src/cp_launcher.rs`),
  installed by `cp serve` (`cmd/cp.rs`). `build_run_argv` →
  `workflow run <name> [target] --run-id <id> --plain [--input k=v]… [--mode m]`;
  `launch()` mints `run_<ULID>`, spawns the child, returns the id.
- Cancel is complete: `POST /api/runs/:id/cancel` → `RunStore::cancel`, which
  marks `Cancelled`, sets `finished_at`, and SIGTERMs the run's `runner_pid`
  (guarding against signalling the cp-serve PID itself); awaiting-approval →
  reject; terminal → 409. The run process records its own `runner_pid`.

So the run survives the CP because it's a separate child process the CP only
launched; cancel works because the run records its pid and the CP signals it.

## Sub-project A — gaps to close

### A1. Detach the spawned run (`rupu-cli` `cp_launcher.rs`)
The current `SubprocessLauncher::launch` spawns the child in the cp-serve
process group with inherited stdio. A `Ctrl-C`/SIGINT to `cp serve` would then
also hit the run. To honor "the run survives if the CP closes," harden the spawn:
- Put the child in its **own process group/session** (`CommandExt::process_group(0)`
  on Unix) so terminal signals to `cp serve` don't propagate.
- Redirect the child's stdout/stderr to `Stdio::null()` (it writes its own
  run.json/events.jsonl/transcripts; nothing useful goes to the inherited TTY).
- Keep `build_run_argv` and the returned `run_<ULID>` id unchanged.

### A2. Dispatch endpoint (`rupu-cp`)
Add `POST /api/workflows/:name/dispatch` with body
`{ inputs: Record<string,string>, mode?: "ask"|"bypass"|"readonly", target?: string }`
where `target` is the CLI positional target string (basic in A: a repo-ref like
`github:owner/repo`, or empty for "current"). Handler:
1. If `s.launcher` is `None` → `ApiError::not_available("run dispatch requires `rupu cp serve`")`.
2. Validate the workflow exists (stem under `<global>/workflows/`) and `mode`
   (when present) is one of the three.
3. Build `LaunchRequest { workflow: name, inputs, mode, target }`, call
   `s.launcher.launch(req).await`, map `LaunchError::Invalid` → 400 /
   `LaunchError::Spawn` → 500, and return `{ run_id }`.

Note: A's target is the CLI target string (project/working-dir selection and the
fuzzy pickers are sub-project B; the existing launcher runs the child in the
cp-serve cwd, and repo-refs clone themselves). B will extend `LaunchRequest`
with an explicit working directory.

### Frontend (`rupu-cp/web`)
- **api.ts**: `dispatchWorkflow(name, body): Promise<{ run_id: string }>` and
  `cancelRun(id): Promise<RunResponse>`, plus a `DispatchBody` type
  `{ inputs: Record<string,string>; mode?: string; target?: string }`.
- **Run modal** (new lightweight Tailwind modal component — no new deps) opened
  by a **Run** button on `WorkflowDetail`:
  - Inputs: one field per the workflow's declared `inputs` (string/int/bool),
    read from `detail.workflow.inputs` (narrowed defensively).
  - Mode: select Ask / Bypass / Read-only.
  - Target (basic in A): a single optional text field — a run-target string
    (e.g. `github:owner/repo`), blank = run in the cp-serve working dir. (The
    project/directory pickers come in B once `LaunchRequest` gains a working
    dir.)
  - A note: "Runs start in the background and keep running even if this page or
    the CP is closed."
  - Submit → `dispatchWorkflow` → navigate to `/runs/<run_id>`. On 501 show
    "dispatch requires `rupu cp serve`."
- **Cancel button** on `RunDetail` (and run rows) shown when status is
  `Running`/`Pending`/`AwaitingApproval` → `cancelRun(id)` → reflect the
  returned status. (Cancel endpoint already exists.)

### Testing (A)
- `rupu-cli`: extend `cp_launcher` tests if argv changes (it shouldn't); the
  detach change is behavioural — covered by the existing argv tests staying green
  plus a manual note.
- `rupu-cp`: dispatch handler — `launcher: None` → 501; with a mock
  `RunLauncher`, a valid body → returns the mock's run id and forwards the
  `LaunchRequest` (workflow/inputs/mode/target) unchanged; unknown workflow →
  404; bad mode → 400.
- web (vitest): Run-form renders fields from a workflow-inputs definition and
  builds the correct `DispatchBody`; Cancel button visibility by status.

### Risks / notes (A)
- Spawn-failure (bad args): `LaunchError::Spawn` → 500; the child also captures
  nothing useful (stdio null) — failures surface via the absent/failed run.
- If the spawned CLI fails before writing `run.json`, `/runs/<id>` shows
  "loading/not found" briefly — mitigated by validating the workflow exists
  before launch; acceptable for v1.
- Cancel is a hard SIGTERM to `runner_pid` (already implemented); a partially
  written transcript is possible, but the CP-written `Cancelled` status keeps
  the UI correct.

---

## Sub-project B — smart target pickers (scoped)

- **Directory browse**: `GET /api/fs/browse?path=<dir>` lists immediate
  subdirectories (name, is_dir), canonicalized, tolerant of unreadable dirs;
  starts at `$HOME` when no path. A directory picker component walks the tree;
  a fuzzy-complete input suggests paths from `getProjects()` (past projects)
  and the browse results.
- **Repository picker**: `GET /api/repos` wires `rupu-scm`'s
  `Registry::discover` → `list_repos()` across logged-in platforms (cached in
  memory with a short TTL since it's a live API call), returning
  `{ platform, repo (owner/name), default_branch, private }`. A fuzzy-complete
  input filters this list; the chosen `platform:owner/repo` becomes the repo
  target string.
- Both drop into A's Run modal target section, replacing the plain text inputs.

## Sub-project C — agent Run (scoped)

- `POST /api/agents/:name/dispatch` spawns `rupu run <agent> [target] [prompt]
  --run-id <id> --mode <m>` detached, same `launch.json` + cancel path. Add
  `--run-id` to `rupu run` too.
- **Run modal on `AgentDetail`**: a prompt textarea + mode + the same target
  picker (reuses B). Navigate to `/runs/<id>`. Cancel works unchanged.

## Out of scope
- Embedding an executor in the CP server (we spawn the CLI instead).
- Graceful between-steps cancellation (hard-kill the process group for v1).
- Non-macOS signal handling specifics (macOS is the target platform).
