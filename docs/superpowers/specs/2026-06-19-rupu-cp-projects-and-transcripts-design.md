# rupu Control Plane — Projects-as-root + Transcript Viewer (Slice A) — Design

**Date:** 2026-06-19
**Author:** matt + Claude
**Status:** Design draft (visual design validated via brainstorm companion)
**Builds on:** the Control Plane (PR #319), the Live Run View depth (PR #320), and the node-overlap fix (PR #321). Stacks on those; rebase onto `main` as they merge.

## Summary

The Control Plane should mirror rupu's true entity model, which a deep code investigation confirmed:

- **The project/workspace is the root.** rupu keeps a real registry at `~/.rupu/workspaces/<ws_id>.toml` (`rupu_workspace::WorkspaceStore::list()`); the `ws_<ULID>` id is **stable per canonical directory**. Every run/session/coverage record fans out from a `workspace_id`. A project owns its runs, sessions, coverage targets (under `<project>/.rupu/coverage/`), and project-local definitions (`<project>/.rupu/{agents,workflows,contracts}`).
- **The run is the atom of work:** one agent + one rendered prompt → exactly one `<run_id>.jsonl` transcript (`rupu_transcript::Event` stream). Orchestrators (workflows, autoflows, sessions) spawn many runs. The three run forms (`RunRecord`, `StandaloneRunMetadata`, `SessionRunRecord`) are unified by the path convention "every run id resolves to one transcript."

This slice makes that model navigable and **closes the single biggest gap: there is no way to see a run's transcript.** It delivers:

1. **Projects-as-root** navigation + a project overview page (the firehose host-wide views remain as a secondary lens).
2. A **transcript viewer** — click any run (or any graph node / list row) → its rendered agent transcript (conversation style), **static for completed runs and live-tailed for in-flight ones**.
3. **Split-pane run detail** — the run graph on top, the selected node's transcript below.
4. **Sessions-as-containers** — a session shows its turn-runs, each → its transcript.
5. A **basic per-project coverage rollup** (targets, findings, and the audit's assessed-% headline).

**Out of scope → Slice B (deep coverage):** the concern×file grid, file-touch heatmap, findings-with-evidence detail, per-run coverage contribution, run-to-run diff, and cross-model disagreement. This slice exposes only the rollup; Slice B exposes the ledger.

---

## Design decisions (locked with matt)
- **IA:** Project-as-root + global firehose. (Projects is the primary lens; host-wide Runs/Coverage stay as the firehose.)
- **Transcript placement:** split-pane (graph on top, transcript panel below, both live).
- **Transcript content:** conversation style (user/assistant bubbles, thinking + tool I/O collapsed by default, findings called out, usage footer); a density toggle to a compact log is a nice-to-have.
- **Transcript liveness:** static load for completed runs + SSE live-tail for in-flight runs.
- **Project page:** overview dashboard (identity header + rollup tiles + sections with "see all" drill-in).

---

## Part 1 — Backend (`rupu-cp`, read adapters only)

New deps for `rupu-cp`: `rupu-workspace` (registry) and `rupu-transcript` (transcript parsing). Both are existing workspace crates; add as path deps. `rupu-cp` stays a read adapter — no `rupu-cli` dependency.

### 1.1 Projects
- `GET /api/projects` → `WorkspaceStore::new(<global>/workspaces).list()` mapped to `ProjectRow { ws_id, name, path, repo_remote?, branch?, created_at, last_run_at? }` (`name` = the path's final component). Sort by `last_run_at` desc. Tolerate a missing registry dir → `[]`.
- `GET /api/projects/{ws_id}` → the rollup. Load the `Workspace` (404 if absent), then aggregate **everything scoped to this workspace**:
  - **runs**: scan the orchestrator `RunStore` for `RunRecord`s whose `workspace_id == ws_id`, bucket by status + by surface (workflow vs autoflow via the trigger derivation); count running.
  - **agent/session runs**: from the existing agent-run sources (`.meta.json` + session `runs[]`), count those whose metadata `workspace_id`/`workspace_path` matches this workspace (standalone `.meta.json` carries `workspace_path`; sessions carry `workspace_id`).
  - **sessions**: count sessions whose `workspace_id == ws_id` (+ active count).
  - **coverage**: `discover_targets(workspace.path)` → target count; sum findings; and the **assessed-% headline** via `rupu_coverage::run_audit(paths)` per target → `complete_concerns / total_concerns` aggregated (best-effort; omit the % when no catalog exists, never fabricate it). Surface `{ targets, findings, assessed_pct? }`.
  Return `ProjectDetail { project: ProjectRow, runs: {total, running, by_surface, by_status}, sessions: {total, active}, coverage: {targets, findings, assessed_pct?}, recent_runs: RunListRow[] (10 newest scoped) }`.
- `GET /api/projects/{ws_id}/runs`, `/api/projects/{ws_id}/sessions`, `/api/projects/{ws_id}/coverage` — the scoped full lists for the "see all" drills. Implemented as workspace-filtered variants of the existing list endpoints (reuse the existing handlers' readers + a `workspace_id` filter).

### 1.2 Transcript viewer (the centerpiece)
A run's transcript is a JSONL of `rupu_transcript::Event`. The frontend already holds the `transcript_path` for every addressable run (step `transcript_path` from `/api/runs/{id}/graph` step_results; unit `transcript_path` from units; agent/session run `transcript_path` from `/api/runs/agents`; sub-run paths from events). So the endpoint is **path-driven with strict validation**:

- `GET /api/transcript?path=<abs path>` →
  - **Validate**: canonicalize `path`; it MUST be a `.jsonl` file located under an allowed root — `<global>` (`~/.rupu`) OR the `path` of any registered workspace (their `<project>/.rupu/`). Reject (400) anything else — no traversal, no arbitrary-file read.
  - Read via `rupu_transcript::JsonlReader::iter` → return `{ events: Event[], summary: RunSummary }` (the `Event` enum + `JsonlReader::summary` both already `Serialize`). Tolerate truncated/partial files (the reader already skips bad lines).
- `GET /api/transcript/stream?path=<abs path>` (SSE) → live-tail the transcript JSONL: same validation, then tail the file (a `FileTailRunSource`-style tailer reused/generalized for an arbitrary JSONL path, emitting each new `Event` as an SSE `data:` line) + a keep-alive. For a completed run the stream simply replays then idles; the client uses the static endpoint for completed and the stream for in-flight (it knows the run status).
- Path validation lives in one helper (`fn validate_transcript_path(path, allowed_roots) -> Result<PathBuf>`), unit-tested for traversal/exfil rejection.

(Rationale for `?path=` over `run_id` resolution: the frontend already has the exact path for all five run forms — step, unit, sub-run, agent, session — so path+validation is uniform and avoids a fragile multi-store run-id search. Safety comes from the allowed-roots canonicalization check.)

---

## Part 2 — Frontend (`crates/rupu-cp/web`)

### 2.1 Nav restructure (Project-as-root)
`lib/sidebarNav.ts` gains a top **Projects** leaf (`/projects`, `FolderGit2` icon) directly under Dashboard. The existing groups stay as the **firehose** (host-wide, all projects): Runs (Agents/Workflows/Autoflows), Observe (Live Events, Coverage), Build, Fleet. So Projects is the primary lens; the firehose is the "everything on this host" lens.

### 2.2 Projects pages
- `pages/Projects.tsx` — the registry list (`getProjects()`): each row = name, path, repo/branch, last-run relative time, a small run/coverage badge. Links to `/projects/:wsId`.
- `pages/ProjectDetail.tsx` — **overview dashboard** (layout A): identity header (name, `path`, `repo_remote`, `branch`, `last_run_at`, `ws_id`); rollup tiles (Runs + running, Sessions + active, **Coverage %** + bar, Findings + high count); sections — Recent runs (→ run detail), Coverage (→ scoped coverage), Sessions (→ scoped, as containers) — each with "see all" to the scoped list (`/projects/:wsId/{runs,sessions,coverage}`). Reuse `ListCard`/`SectionHeader`/`StatusPill`/`lib/time`.

### 2.3 Transcript panel
- `lib/transcript.ts` — TS types for the `rupu_transcript::Event` union (`run_start`, `turn_start`, `assistant_message {content, thinking}`, `tool_call {tool, input}`, `tool_result {output, error, duration_ms}`, `file_edit`, `command_run`, `usage`, `turn_end`, `run_complete`, …) + `getTranscript(path)` and `subscribeTranscript(path, onEvent)` api methods (path URL-encoded).
- `components/TranscriptPanel.tsx` — **conversation rendering** (layout A): a header (agent · model · status · token total), then the event stream as: user/assistant bubbles (assistant content as light markdown), **thinking collapsed** behind a toggle, **tool calls as collapsible cards** (name + input; result/error/duration in a collapsed body), file-edit/command chips, findings highlighted (red), and a usage footer. Props `{ path: string; live: boolean }`: on mount `getTranscript(path)`; if `live`, also `subscribeTranscript` and append new events (dedupe by sequence/position), with a connection indicator. A density toggle (conversation ⇄ compact log) is optional/stretch.

### 2.4 Split-pane run detail
`pages/RunDetail.tsx` becomes two panes: the existing `RunGraph` on top, a `TranscriptPanel` below. Selecting a graph node (step/unit — they carry a `transcript_path` + run state) sets `{ selectedPath, live }` (live = the node/run is running) → the panel loads/streams it. Default selection = the active/most-recent node. Keep the Events tab available (Graph+Transcript is the default split; Events remains a tab/toggle). The existing single run-log SSE subscription is unchanged; the transcript stream is a **separate** SSE on the transcript file (distinct from `events.jsonl`).

### 2.5 Click-to-transcript from list rows + sessions-as-containers
- Agent/session/standalone run rows (`/api/runs/agents`) and workflow/autoflow run rows already carry a transcript path (or resolve to one) → clicking a row opens its transcript. For runs **with** a graph (workflow/autoflow), route to the split-pane run detail. For runs **without** a graph (agent/session/standalone), a `pages/RunTranscript.tsx` (transcript-only page: header + `TranscriptPanel`).
- **Sessions-as-containers:** a session is shown as a container of its turn-runs. `pages/SessionDetail.tsx` lists the session's `runs[]` (each: prompt preview, status, tokens, time) → clicking a turn opens its transcript (`RunTranscript`). The session's runs come from grouping `/api/runs/agents` by `session_id` (or a `runs` field added to the session DTO — pick the cheaper at plan time). Project detail's Sessions section + the global Fleet ▸ Sessions both link here.

### 2.6 api client additions
`lib/api.ts`: `getProjects()`, `getProject(wsId)`, `getProjectRuns/Sessions/Coverage(wsId)`, `getTranscript(path)`, `subscribeTranscript(path, onEvent, onError?)`, with typed `ProjectRow`, `ProjectDetail`, and the transcript `Event` types (in `lib/transcript.ts`).

---

## Testing

- **Backend:** fixture a `WorkspaceStore` (a couple `<ws>.toml`) → assert `/api/projects`; seed a run/session/coverage target under a workspace path → assert `/api/projects/{id}` rollup counts. **Transcript:** write a small `<run>.jsonl` of real `rupu_transcript::Event`s → assert `/api/transcript?path=` returns the parsed events; assert the **path validator rejects traversal/out-of-root paths** (the security-critical test); a streaming test (append lines → SSE emits them), time-bounded.
- **Frontend:** unit-test the transcript event→view mapping (each event type → the right rendered shape; thinking/tool collapse state); `getTranscript`/`subscribeTranscript` typed against the backend JSON. `tsc -b && vite build` green. Rendering validated by matt (same rule as the rest of the CP).

---

## Open decisions for review
1. **Session runs source** — group `/api/runs/agents` by `session_id` (no backend change) vs. add a `runs[]` array to the session DTO (one endpoint change). Lean: group client-side first; add the DTO field only if needed.
2. **Transcript density toggle** — ship the conversation view only, or include the compact-log toggle in Slice A. Lean: conversation only first; toggle as a fast follow.
3. **Coverage % source** — the rollup `assessed_pct` from `run_audit` (real, but pulls a slice of Slice B's machinery into A). Lean: include the headline % (it's the "how far" signal matt wants) but keep the grid/heatmap/diff in Slice B.
