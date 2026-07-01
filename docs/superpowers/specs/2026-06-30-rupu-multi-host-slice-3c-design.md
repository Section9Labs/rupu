# rupu multi-host — Slice 3c: cross-host workspace sync

Status: approved (design), pending implementation plan
Date: 2026-06-30

## Context

Slices 3a (distributed fan-out units) and 3b (per-step host placement), shipped
in v0.30.0 / v0.31.0, both rely on a **self-contained-step guardrail**: a step
run on a remote host is computable purely from its rendered prompt + prior-step
**string** outputs (which the coordinator holds), never from the shared file
workspace. The remote host has its own filesystem and never sees the
coordinator's working tree.

Slice 3c lifts that guardrail. When a step opts in, the coordinator makes its
**file workspace** available on the remote host so the step's agent can read and
write real project files, and the resulting file changes are propagated back so
downstream steps (local or remote) see them. This is distributed filesystem
coherence across hosts — the hard part of distributed workflows — and it is the
final part of Slice 3.

3c reuses the existing machinery: the `UnitDispatcher` port + `FleetUnitDispatcher`
(`launch_agent` on a `HostConnector` → poll `get_run` → read `run.final_output`),
the coordinator (`rupu workflow run`) building a `HostRegistry`, and the
supported-transports reality. The orchestrator (`rupu-orchestrator`) remains
hexagonal: it depends on **neither** `rupu-workspace` nor `rupu-cp`, and knows
only the `UnitDispatcher` trait + opaque workspace values.

## Spine decisions (approved)

1. **Auto git-or-tar sync model.** At pack time the workspace is probed: if it is
   a git repository (`git rev-parse` succeeds at the workspace path) → **git
   mode**; otherwise → **tar mode**. The transferred payload is self-describing
   (a `git` / `tar` tag) so the remote stages and the coordinator applies in the
   same mode. No new configuration — detection is automatic.
2. **Changed-files delta on the return path.** A remote step never ships its
   whole tree back. It returns only what it created / modified / deleted relative
   to the baseline it received: in git mode a diff/patch (or a bundle of the new
   commit), in tar mode a manifest of changed files (bytes) + a deletion list.
   The coordinator applies only those paths over its tree; untouched files are
   left exactly as-is.
3. **Opt-in via workflow default + per-step override.** `Workflow.defaults.workspace`
   sets a workflow-wide mode; `Step.workspace` overrides it per step. The effective
   value falls back to **self-contained** when neither is set, so 3a/3b behavior
   is byte-for-byte preserved and the sync cost is paid only where declared.
4. **v1 covers placed-linear AND distributed fan-out.** A `host:`-placed linear
   step (3b) and a `distribute:` fan-out step (3a) can both carry `workspace:
   sync`.
5. **Mode-aware conflict semantics, resolved behind the apply port.** Because a
   git 3-way merge is not pure path logic, all conflict detection / merging lives
   inside the apply port (in `rupu-workspace` / `rupu-cli`), not the orchestrator:
   - **git mode:** sequential 3-way merge of each unit's patch — overlapping
     *files* merge cleanly; only a true conflicting *hunk* fails (typed conflict
     error).
   - **tar mode:** file-level **disjoint-or-error** — two units changing the same
     path fail the step with a clear conflict error naming the paths.
   Either way the orchestrator surfaces the typed conflict error, honoring the
   step's `continue_on_error` exactly like any other step failure.
6. **No git required on hosts.** `git2` (libgit2, vendored) is already a workspace
   dependency, so git mode needs nothing installed on the remote — rupu carries
   it. New dependencies are only for the tar fallback: `tar` + `ignore`
   (gitignore-aware directory walk).

## Goals (Slice 3c)

- A `workspace: sync` step's agent runs against the coordinator's project files
  on the remote host, and its file changes feed downstream steps.
- git workspaces round-trip via git (diff/3-way-merge); non-git workspaces
  round-trip via tar (gitignore-aware, changed-files delta).
- Distributed fan-out units that touch disjoint paths (tar) or non-conflicting
  hunks (git) merge cleanly; a real overlap fails the step honoring
  `continue_on_error` with a clear conflict error.
- No `workspace:` / `workspace: none` ⇒ byte-for-byte the current self-contained
  behavior (3a/3b unchanged).
- An unsupported transport fails fast with a clear error — no silent
  self-contained fallback.

## Non-goals (later)

- Bucket and Tunnel workspace sync (v1 supports Local / SSH / HttpCp).
- A partial-paths allowlist (v1 syncs the whole gitignore-respected tree).
- Automatic resolution of genuinely conflicting edits (v1 errors instead of
  inventing a merge).
- mTLS (Slice 4) / sessions (Slice 5).

## Architecture

### The seam — extend the `UnitDispatcher` port (orchestrator stays opaque)

The orchestrator cannot tar, run git, or apply files (no `rupu-workspace` /
`rupu-cp` dependency). It therefore deals only in opaque workspace values across
the existing `UnitDispatcher` seam:

- **`UnitDispatch`** gains the coordinator `workspace_path: Option<PathBuf>` (set
  when the unit's effective workspace mode is `sync`). Absent ⇒ today's
  self-contained dispatch, unchanged.
- **`UnitOutcome`** gains `workspace_delta: Option<WorkspaceDelta>`, where
  `WorkspaceDelta` carries an opaque, self-describing payload (`git` patch/bundle
  or `tar` changed-files set) plus, for observability, the changed/deleted path
  lists. The orchestrator does **not** interpret the payload.
- A new port method **`apply_workspace_deltas(workspace_path, &[WorkspaceDelta])
  -> Result<(), WorkspaceConflict>`** applies the collected deltas to the
  coordinator workspace. It is **mode-aware** (git 3-way merge / tar
  disjoint-copy) and is where conflict detection lives. A `WorkspaceConflict`
  return becomes a step failure the orchestrator surfaces honoring
  `continue_on_error`.

This keeps a single port (as 3a/3b used), the orchestrator fully opaque, and
treats a placed linear step as fan-out-of-one through the same
collect → apply path.

### Workflow model (`crates/rupu-orchestrator/src/workflow.rs`)

- `WorkspaceMode` enum: `Sync` | `None` (serde `lowercase`).
- `Workflow.defaults.workspace: Option<WorkspaceMode>` (workflow-wide default).
- `Step.workspace: Option<WorkspaceMode>` (per-step override).
- Effective mode = `step.workspace` ↦ else `defaults.workspace` ↦ else `None`.
- Validation: `workspace: sync` is meaningful only on a **remote** step — i.e. a
  step with `host:` (3b) or `distribute:` (3a). `workspace: sync` on a purely
  local step is rejected at parse time with a clear error (a local step already
  has the real workspace; the flag would be a no-op and signals author confusion).

### Routing (`crates/rupu-orchestrator/src/runner.rs`)

- `run_linear_step`: when the unit is placed (`host:`) and the effective mode is
  `sync`, set `UnitDispatch.workspace_path = Some(opts.workspace_path)`. After the
  unit returns, collect its single `workspace_delta` and call
  `apply_workspace_deltas` (trivially conflict-free for one writer) **before**
  emitting `StepCompleted`.
- `run_fanout_step`: when the effective mode is `sync`, every unit is dispatched
  with `workspace_path`. After **all** units join, collect their deltas and call
  `apply_workspace_deltas` once with the full set; the port performs the
  mode-aware merge / disjoint check. A `WorkspaceConflict` aborts the step unless
  `continue_on_error` tolerates it.
- The apply happens before `StepCompleted` so downstream steps see the updated
  tree. No host/step-event shape changes (3b's `host` attribution is reused).

### Packing / staging / applying

- **`rupu-workspace`** gains a `WorkspacePacker`:
  - `pack(workspace_path) -> WorkspacePayload` — git mode: snapshot the working
    tree (tracked + dirty + untracked-not-ignored) into a throwaway commit and
    `git bundle` it via `git2`; tar mode: tar the `ignore`-walked tree.
  - `collect_delta(scratch_dir, baseline) -> WorkspaceDelta` (remote side) — git
    mode: diff the post-run tree vs the staged commit (patch or new-commit
    bundle); tar mode: re-hash the tree and emit changed files + deletion list vs
    the baseline manifest captured at stage time.
  - `apply_deltas(workspace_path, &[WorkspaceDelta]) -> Result<(), WorkspaceConflict>`
    — git mode: sequential `git apply --3way` / merge, reporting conflicting
    hunks; tar mode: union the changed-path sets, fail on overlap, else copy
    changed files and remove deleted ones.
  - The payload/delta are self-describing (mode tag) so pack and apply agree.
- **`rupu-cp` `HostConnector`** gains `stage_workspace(payload) -> remote_working_dir`
  and `collect_workspace_delta(working_dir) -> WorkspaceDelta`, implemented per
  transport (Local: extract/clone into a temp dir; SSH: ship the payload + run
  the staging/collection via the remote `rupu`, which carries `git2`/`tar`;
  HttpCp: new CP endpoints). Bucket / Tunnel return `Unsupported`.
- **`rupu-cli` `FleetUnitDispatcher`** composes: `stage_workspace` →
  `launch_agent(working_dir = staged)` → poll `get_run` → `collect_workspace_delta`
  → return `UnitOutcome { output, workspace_delta }`; and implements
  `apply_workspace_deltas` over the `rupu-workspace` packer. `build_dispatcher_if_needed`
  already fires for `distribute:` / `host:` steps; no trigger change is needed
  (workspace sync only ever rides on an already-placed/distributed step).

### Transports (v1) & clean failure

**Local / SSH / HttpCp** support workspace sync in v1. **Bucket** (dead-drop
staging) and **Tunnel** are out of scope; a `workspace: sync` step whose host
resolves to an unsupported transport fails fast with a clear "workspace sync not
supported on `<transport>` transport" error — **no silent self-contained
fallback**.

## Errors & security

- Scratch directories (staged baseline + run dir) are cleaned up after each unit.
- A configurable size guard caps the transferred payload (sane default) with a
  clear over-limit error, bounding the data moved.
- No new secrets and no new inbound surface — workspace material rides the
  existing authenticated transports.
- gitignore is respected in both directions (git: native; tar: the `ignore`
  crate), keeping build artifacts out of the transfer.
- `WorkspaceConflict` is a typed error naming the conflicting paths/hunks; it is
  surfaced as a step failure (never a silent wrong merge).
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`;
  workspace deps only; orchestrator depends only on the `UnitDispatcher` trait +
  opaque workspace values.

## Testing

- **Workflow model:** `workspace` serde on `Step` and `defaults`; effective-mode
  resolution (step over default over none); `workspace: sync` rejected on a
  purely-local step; absent ⇒ `None` (backward compatible).
- **Packer (`rupu-workspace`):** git mode round-trips a repo (pack → simulate
  remote edit → collect delta → apply, files updated); tar mode round-trips a
  non-git dir respecting `.gitignore`; deletion propagates; mode auto-detected.
- **Conflict semantics:** git mode merges non-overlapping hunks in the same file
  and reports a conflicting hunk; tar mode merges disjoint files and errors on an
  overlapping path (with the path named).
- **Routing (fake `UnitDispatcher` + fake apply):** a placed `workspace: sync`
  linear step sends `workspace_path`, applies the returned delta before
  `StepCompleted`, and a downstream step sees the change; a fan-out
  `workspace: sync` step collects N deltas and applies once; a `WorkspaceConflict`
  aborts under `continue_on_error: false` / is tolerated under `true`. A
  `workspace: none` step sends no `workspace_path` (unchanged path).
- **Transport failure:** a `workspace: sync` step on a Tunnel/Bucket host returns
  the clear unsupported-transport error.
- **Backward compat:** no `workspace:` anywhere ⇒ existing 3a/3b suites stay
  green; `workspace_path` is `None` and the dispatch path is byte-for-byte today.
- **e2e:** a 2-step workflow where step 1 (`host:` + `workspace: sync`) edits a
  file remotely and step 2 (local) reads the edited file; plus a non-git (tar)
  variant and a fan-out variant with disjoint per-unit edits.

## Open questions

- **git snapshot of the dirty tree:** whether to capture untracked-not-ignored
  files via a throwaway commit (preferred — the agent sees the real current tree)
  or restrict to `HEAD` + tracked changes. Resolve in the plan; the preferred
  approach is the throwaway-commit snapshot so a placed step sees uncommitted work.
- **Delta return encoding in git mode:** a `git diff` patch (simplest to
  `apply --3way`) vs a bundle of the remote's new commit (preserves authorship /
  history). Resolve in the plan; default to the patch unless history is needed.
