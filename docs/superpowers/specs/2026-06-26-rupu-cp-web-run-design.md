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

## Sub-project A — Run + Cancel (full detail)

### CLI change (`rupu-cli`)
- Add a `--run-id <id>` flag to `workflow run` (`crates/rupu-cli/src/cmd/workflow.rs`
  `Run` clap variant). The plumbing already exists (`run_id_override` flows to
  the orchestrator); the flag just exposes it so the CP can pre-generate the id
  and link the web straight to `/runs/<id>`. Default unchanged (generate a ULID
  when absent).

### Run status (`rupu-orchestrator`)
- Add a `RunStatus::Cancelled` variant (today: Pending | Running | Completed |
  Failed | AwaitingApproval | Rejected). Serializes `"cancelled"`. Treated as a
  terminal status everywhere status is matched (lists, filters, UI color).

### Dispatch endpoint (`rupu-cp`)
- `POST /api/workflows/:name/dispatch` with body
  `{ inputs: Record<string,string>, mode: "ask"|"bypass"|"readonly", target: Target }`
  where `Target` is one of:
  - `{ kind: "project", ws_id }` → working dir = that workspace's path.
  - `{ kind: "directory", path }` → working dir = that path.
  - `{ kind: "repo", repo_ref }` → passed as the CLI positional target (the CLI
    clones it); working dir = a neutral dir (e.g. `<global>`).
- Handler:
  1. Validate the workflow exists (stem under `<global>/workflows/`) and the
     mode is one of the three.
  2. Generate `run_id = "run_<ULID>"`.
  3. Build argv: `<current_exe> workflow run <name> [repo_ref] --run-id <id>
     --mode <m> --input k=v … --plain`.
  4. Spawn **detached** in a new process group (`setsid` / `process_group(0)`)
     with `current_dir` set to the resolved working dir, stdout+stderr
     redirected to `<global>/runs/<id>/launch.log`.
  5. Write a sidecar `<global>/runs/<id>/launch.json` = `{ pid, pgid,
     spawned_at }`.
  6. Return `{ run_id }` immediately.
- `--plain` is passed so the spawned process doesn't start the interactive TUI.
- The CP never runs the orchestrator in-process; it only launches + records.

### Cancel endpoint (`rupu-cp`)
- `POST /api/runs/:id/cancel`:
  1. Load the run record. If status is terminal (Completed/Failed/Rejected/
     Cancelled) → no-op, return current state.
  2. Read `launch.json`; if present and status is `Running`/`Pending`, send
     `SIGTERM` to the **process group** (`-pgid`), then `SIGKILL` after a short
     grace window if still alive. (Signal helper via `nix`/`libc`; macOS target.)
  3. Set the run record status to `Cancelled` (CP marker-write, mirroring how
     `reject` sets status directly), with `finished_at = now`.
  4. Return the updated run.
- Guards: only signal when status is non-terminal (avoids PID-reuse hits on an
  already-exited run). Works after a CP restart since pid/pgid live on disk.

### Frontend (`rupu-cp/web`)
- **api.ts**: `dispatchWorkflow(name, body): Promise<{ run_id }>`,
  `cancelRun(id): Promise<RunDetail>`, plus `DispatchBody` / `Target` types.
- **Run modal** (new lightweight Tailwind modal component — no new deps) opened
  by a **Run** button on `WorkflowDetail`:
  - Inputs: one field per the workflow's declared `inputs` (string/int/bool),
    read from `detail.workflow.inputs`.
  - Mode: select Ask / Bypass / Read-only.
  - Target (basic in A): radio — Existing project (dropdown from `getProjects`)
    / Directory (text path) / Repository (text repo-ref).
  - A note: "Runs start in the background and continue even if this page closes."
  - Submit → `dispatchWorkflow` → navigate to `/runs/<run_id>`.
- **Cancel button** on `RunDetail` (and run rows) shown when status is
  `Running`/`Pending`/`AwaitingApproval` → `cancelRun(id)` → reflect `Cancelled`.

### Testing (A)
- `rupu-cli`: `--run-id` flag parsed and threaded to `run_id_override` (arg-parse
  test).
- `rupu-orchestrator`: `RunStatus::Cancelled` serde round-trip + terminal-status
  treatment.
- `rupu-cp`: dispatch handler builds the expected argv + writes `launch.json`
  (factor argv construction into a pure, testable function); cancel handler sets
  status `Cancelled` and no-ops on terminal runs (use a tempfile run store
  fixture).
- web (vitest): Run-form renders fields from a workflow-inputs definition and
  builds the correct `DispatchBody`; Cancel button visibility by status.

### Risks / notes (A)
- Spawn-failure (bad args, missing binary): validate workflow + mode before
  spawn; surface spawn errors; `launch.log` captures CLI stderr for debugging.
- If the spawned CLI fails before writing `run.json`, `/runs/<id>` would 404
  briefly — mitigated by pre-spawn validation; acceptable for v1.
- Hard-kill cancel may leave a partially-written transcript; the CP-written
  `Cancelled` status keeps the UI correct regardless.

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
