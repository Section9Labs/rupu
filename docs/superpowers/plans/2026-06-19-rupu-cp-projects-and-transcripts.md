# CP Projects-as-root + Transcript Viewer (Slice A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Control Plane reflect rupu's true model — Project (workspace) as the root, the run as the atom (one agent → one transcript) — by adding Projects-as-root navigation, project pages, and a transcript viewer (split-pane run detail with a live, conversation-style transcript), plus sessions-as-containers and a basic per-project coverage rollup.

**Architecture:** Additive, read-only on top of the existing `rupu-cp` crate + `crates/rupu-cp/web` app. New read endpoints over the workspace registry (`rupu-workspace`) and transcript files (`rupu-transcript`); the transcript endpoint is path-driven with strict allowed-root validation. The frontend gains a Projects section, a `TranscriptPanel`, and a split-pane run detail.

**Tech Stack:** Rust (axum, serde, rupu-workspace, rupu-transcript, rupu-coverage); React 18 + TypeScript + `@xyflow/react`.

**Spec:** `docs/superpowers/specs/2026-06-19-rupu-cp-projects-and-transcripts-design.md`

**Branch:** `feat-cp-projects-transcripts` (stacks on #320 depth + #321 overlap-fix; rebase onto `main` as they merge).

**Open-decision resolutions (from spec review):** session runs grouped client-side by `session_id` (no session-DTO change); conversation transcript view only (no density toggle yet); include the `run_audit` assessed-% headline in the project rollup.

---

## Verified facts (rely on these)
- `rupu_workspace::WorkspaceStore { root: PathBuf }` (pub field) with `list() -> Result<Vec<Workspace>, StoreError>` and `load(&id) -> Result<Option<Workspace>, StoreError>`. `Workspace { id: String, path: String, repo_remote: Option<String>, initial_branch: Option<String>, created_at: String, last_run_at: Option<String> }`. Registry root = `<global>/workspaces`.
- `rupu_transcript`: `Event` enum is **adjacently tagged** `#[serde(tag="type", content="data", rename_all="snake_case")]` → JSON `{"type":"tool_call","data":{...}}`. Variants: `run_start{run_id,agent,provider,model,started_at,mode}`, `turn_start`, `assistant_delta{content}`, `assistant_message{content,thinking?}`, `tool_call{call_id,tool,input}`, `tool_result{call_id,output,error?,duration_ms}`, `file_edit`, `command_run`, `action_emitted`, `gate_requested`, `turn_end{tokens_in?,tokens_out?}`, `usage{input_tokens,output_tokens,cached_tokens}`, `run_complete{run_id,status,total_tokens,duration_ms,error?}`. Reader: `JsonlReader::iter(path)` (iterator of Events, skips bad lines) + `JsonlReader::summary(path) -> Result<RunSummary, ReadError>`. `RunSummary { run_id, workspace_id, agent, provider, model, started_at, mode, status, total_tokens, duration_ms, error, first_assistant_text }`. Exports: `rupu_transcript::{Event, JsonlReader, RunSummary, RunStatus, RunMode}`.
- `RunRecord.workspace_id: String`; `SessionRecord.workspace_id`; `StandaloneRunMetadata.workspace_path` (NO workspace_id — match by path). Coverage: `rupu_coverage::ledger::discover::discover_targets(&Path)`, `run_audit(&CoveragePaths) -> AuditReport { complete_concerns, total_concerns, .. }`, `CoveragePaths::new(workspace, target_id)`.
- `FileTailRunSource` parses the ORCHESTRATOR event (`events.jsonl`); it does NOT fit transcript JSONL. The transcript SSE needs its own line-tailer parsing `rupu_transcript::Event`.

---

## PART A — Backend (Rust, TDD)

### Task 1: `GET /api/projects`

**Files:** Create `crates/rupu-cp/src/api/projects.rs`; modify `api/mod.rs`, `server.rs`, `Cargo.toml` (+`rupu-workspace` path dep).

- [ ] **Step 1: add dep + module.** Add `rupu-workspace = { path = "../rupu-workspace" }` to `crates/rupu-cp/Cargo.toml`. Add `pub mod projects;` to `api/mod.rs`.

- [ ] **Step 2: failing test** `crates/rupu-cp/tests/projects.rs`:
```rust
#[tokio::test]
async fn lists_projects_from_registry() {
    let dir = tempfile::tempdir().unwrap();
    // seed a workspace record: write <global>/workspaces/ws_test.toml matching the Workspace shape
    // (read crates/rupu-workspace/src/record.rs for the exact toml fields; or use WorkspaceStore to write one if it exposes a writer)
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();
    std::fs::write(ws_dir.join("ws_test.toml"),
        "id = \"ws_test\"\npath = \"/tmp/proj\"\ncreated_at = \"2026-06-19T00:00:00Z\"\n").unwrap();
    let state = rupu_cp::state::AppState::new(dir.path().into(), Default::default());
    // serve + GET /api/projects → 200, array contains ws_test with name "proj"
}
```
(Verify the minimal valid `Workspace` TOML by reading `record.rs` — include any required non-Option field. If `WorkspaceStore` has a write/`upsert` test helper, prefer it.)

- [ ] **Step 3: run → fails.**

- [ ] **Step 4: implement** `projects.rs`:
```rust
use axum::{extract::State, Json, routing::get, Router};
use crate::{state::AppState, error::{ApiError, ApiResult}};
use rupu_workspace::WorkspaceStore;

#[derive(serde::Serialize)]
pub struct ProjectRow {
    pub ws_id: String, pub name: String, pub path: String,
    pub repo_remote: Option<String>, pub branch: Option<String>,
    pub created_at: String, pub last_run_at: Option<String>,
}

fn store(s: &AppState) -> WorkspaceStore { WorkspaceStore { root: s.global_dir.join("workspaces") } }

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/projects", get(list_projects))
}

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<ProjectRow>>> {
    let mut rows: Vec<ProjectRow> = store(&s).list().unwrap_or_default().into_iter().map(|w| ProjectRow {
        name: std::path::Path::new(&w.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| w.path.clone()),
        ws_id: w.id, path: w.path, repo_remote: w.repo_remote, branch: w.initial_branch,
        created_at: w.created_at, last_run_at: w.last_run_at,
    }).collect();
    rows.sort_by(|a, b| b.last_run_at.cmp(&a.last_run_at));   // newest activity first; None sorts last via Option ord (None < Some, so reverse)
    Ok(Json(rows))
}
```
(Note the `last_run_at` sort: `Option<String>` orders `None < Some`; `b.cmp(a)` puts `Some` (recent) before `None`. Confirm the direction in the test by seeding one with and one without `last_run_at`.) Merge `projects::routes()` in `server.rs`. Tolerate a missing registry dir → `[]` (map `list()` error to default).

- [ ] **Step 5: run → PASS; clippy `-p rupu-cp --all-targets` exit 0.**

- [ ] **Step 6: commit** `feat(cp): GET /api/projects (workspace registry)`.

---

### Task 2: `GET /api/projects/{ws_id}` rollup + scoped lists

**Files:** Modify `crates/rupu-cp/src/api/projects.rs`. Test `tests/projects.rs`.

- [ ] **Step 1: failing test** — seed: the `ws_test.toml` (path `<tmp>/proj`); a `RunRecord` with `workspace_id == "ws_test"` (one running, one completed) via the run-store seeding helper from `tests/runs.rs`; a coverage target under `<tmp>/proj/.rupu/coverage/tgt/` with a couple assertion lines. Assert `GET /api/projects/ws_test` returns `runs.total>=2` with `running>=1`, `coverage.targets>=1`, and a `recent_runs` array.

- [ ] **Step 2: run → fails.**

- [ ] **Step 3: implement** `get_project` + the rollup:
```rust
#[derive(serde::Serialize)]
struct ProjectDetail {
    project: ProjectRow,
    runs: serde_json::Value,      // { total, running, by_status, by_surface }
    sessions: serde_json::Value,  // { total, active }
    coverage: serde_json::Value,  // { targets, findings, assessed_pct: Option<f64> }
    recent_runs: Vec<serde_json::Value>,
}
```
   - Route `/api/projects/:ws_id`. Load the workspace (404 if `load` returns `None`).
   - **runs**: `s.run_store.list()` filtered to `r.workspace_id == ws_id`; bucket by status (reuse the `RunStatus::as_str` keys) and by surface (workflow vs autoflow via the existing trigger derivation in `api/runs.rs` — reuse/extract `trigger_of`); count running; `recent_runs` = 10 newest scoped (slim `RunListRow`-style).
   - **sessions**: reuse the session scan from `api/sessions.rs` filtered to `workspace_id == ws_id`; `{ total, active }` (active = a non-archived / status running — best-effort from the session DTO).
   - **coverage**: `discover_targets(Path::new(&workspace.path))` → target count; for each target build `CoveragePaths::new(&workspace.path, &target_id)`, `read_findings(&paths).len()` summed; and `assessed_pct` = aggregate `run_audit(&paths)` `complete_concerns/total_concerns` across targets that HAVE a catalog (omit when no catalog — `None`, never fabricate). Wrap the audit call defensively (it may error on a sparse ledger → skip that target, warn).
   - Add `route("/api/projects/:ws_id/runs", ...)`, `.../sessions`, `.../coverage` returning the scoped full lists (reuse the existing list readers + the `workspace_id`/path filter). Keep these slim and consistent with the firehose list DTOs.

- [ ] **Step 4: run → PASS; clippy clean.**

- [ ] **Step 5: commit** `feat(cp): GET /api/projects/{id} rollup + scoped lists`.

---

### Task 3: transcript path validator + `GET /api/transcript`

**Files:** Create `crates/rupu-cp/src/api/transcript.rs`; modify `api/mod.rs`, `server.rs`, `state.rs` (allowed-roots), `Cargo.toml` (+`rupu-transcript`).

- [ ] **Step 1: add dep.** `rupu-transcript = { path = "../rupu-transcript" }` to `crates/rupu-cp/Cargo.toml`.

- [ ] **Step 2: failing tests (security-critical)** `crates/rupu-cp/tests/transcript.rs`:
```rust
#[test]
fn validator_accepts_in_root_jsonl_rejects_traversal() {
    let root = tempfile::tempdir().unwrap();
    let global = root.path().to_path_buf();
    let good = global.join("transcripts").join("run_x.jsonl");
    std::fs::create_dir_all(good.parent().unwrap()).unwrap();
    std::fs::write(&good, "").unwrap();
    let roots = vec![global.clone()];
    assert!(rupu_cp::api::transcript::validate_transcript_path(good.to_str().unwrap(), &roots).is_ok());
    // out-of-root absolute path → Err
    assert!(rupu_cp::api::transcript::validate_transcript_path("/etc/passwd", &roots).is_err());
    // traversal escaping root → Err
    let bad = format!("{}/transcripts/../../etc/passwd", global.display());
    assert!(rupu_cp::api::transcript::validate_transcript_path(&bad, &roots).is_err());
    // non-.jsonl → Err
    let notjsonl = global.join("transcripts/run_x.txt"); std::fs::write(&notjsonl, "").unwrap();
    assert!(rupu_cp::api::transcript::validate_transcript_path(notjsonl.to_str().unwrap(), &roots).is_err());
}

#[tokio::test]
async fn get_transcript_returns_events() {
    // write a <global>/transcripts/run_x.jsonl with 2 real rupu_transcript::Event lines
    // (construct Event values, serialize each with serde_json::to_string, join with \n)
    // GET /api/transcript?path=<that path> → 200, body.events length 2, first event "type" == "run_start"
}
```

- [ ] **Step 3: run → fails.**

- [ ] **Step 4: implement** `transcript.rs`:
```rust
use std::path::{Path, PathBuf};
use axum::{extract::{State, Query}, Json, routing::get, Router};
use crate::{state::AppState, error::{ApiError, ApiResult}};

/// Canonicalize `raw`; require it to be a `.jsonl` file whose canonical path is
/// inside one of `allowed_roots` (themselves canonicalized). Rejects traversal,
/// symlink-escape, and out-of-root absolute paths.
pub fn validate_transcript_path(raw: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    let p = Path::new(raw);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") { return Err("not a .jsonl".into()); }
    let canon = std::fs::canonicalize(p).map_err(|_| "cannot resolve path".to_string())?;
    for root in allowed_roots {
        if let Ok(rc) = std::fs::canonicalize(root) {
            if canon.starts_with(&rc) { return Ok(canon); }
        }
    }
    Err("path outside allowed roots".into())
}

#[derive(serde::Deserialize)]
struct PathQ { path: String }

fn allowed_roots(s: &AppState) -> Vec<PathBuf> {
    // global ~/.rupu + every registered workspace's dir (project-local .rupu lives under the workspace path)
    let mut roots = vec![s.global_dir.clone()];
    if let Ok(list) = (rupu_workspace::WorkspaceStore { root: s.global_dir.join("workspaces") }).list() {
        roots.extend(list.into_iter().map(|w| PathBuf::from(w.path)));
    }
    roots
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/transcript", get(get_transcript))
}

async fn get_transcript(State(s): State<AppState>, Query(q): Query<PathQ>) -> ApiResult<Json<serde_json::Value>> {
    let path = validate_transcript_path(&q.path, &allowed_roots(&s))
        .map_err(|e| ApiError(axum::http::StatusCode::BAD_REQUEST, e))?;
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(&path)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .filter_map(Result::ok)   // adapt to the real iter() item type (Result<Event> or Event)
        .collect();
    let summary = rupu_transcript::JsonlReader::summary(&path).ok();
    Ok(Json(serde_json::json!({ "events": events, "summary": summary })))
}
```
   Adapt `JsonlReader::iter`'s exact item type (read `reader.rs` — it may yield `Event` or `Result<Event, _>`; the `.filter_map(Result::ok)` is for the Result case). Add `pub mod transcript;` + merge routes. Ensure `ApiError` has a tuple-construct or a `bad_request` ctor (add `ApiError::bad_request` if cleaner than the tuple).

- [ ] **Step 5: run → PASS (incl. the traversal-rejection test); clippy clean.**

- [ ] **Step 6: commit** `feat(cp): GET /api/transcript (path-validated, rupu-transcript)`.

---

### Task 4: `GET /api/transcript/stream` (SSE live-tail)

**Files:** Modify `crates/rupu-cp/src/api/transcript.rs`; create a transcript tailer (e.g. `crates/rupu-cp/src/transcript_tail.rs`).

- [ ] **Step 1: build the tailer.** `FileTailRunSource` is typed to the orchestrator event and won't parse transcript JSONL. Create a focused tailer that watches a file path and emits each newly-appended line parsed as `rupu_transcript::Event`. Mirror `FileTailRunSource`'s structure (read `crates/rupu-orchestrator/src/executor/file_tail.rs`): an initial drain of existing lines + a `notify` watcher + a poll fallback, pushing `rupu_transcript::Event`s into an `mpsc`/stream. Keep it generic over "parse one line → T" if cheap, else hardcode the transcript Event.

- [ ] **Step 2: failing test** — write a transcript JSONL with one event; connect to `GET /api/transcript/stream?path=...` with a streaming client; read the first SSE `data:` line within a `tokio::time::timeout(5s)`; assert it parses to an Event with `type == "run_start"`. Also assert an out-of-root path → 400 (validation runs first).

- [ ] **Step 3: run → fails.**

- [ ] **Step 4: implement** `stream_transcript`: validate the path (same `validate_transcript_path`), open the transcript tailer, map each `Event` → `SseEvent::default().json_data(&ev)` (degrade serialize errors to a comment), `.keep_alive(...)`. Mirror the existing `crate::sse::tail_events_sse` shape (return `Sse<...>` or `.into_response()`). Add `route("/api/transcript/stream", get(stream_transcript))`.

- [ ] **Step 5: run → PASS (no hang); clippy clean.**

- [ ] **Step 6: commit** `feat(cp): SSE live-tail of a run transcript`.

---

## PART B — Frontend (logic TDD'd, visuals by contract)

### Task 5: api client + transcript types

**Files:** Create `crates/rupu-cp/web/src/lib/transcript.ts`; modify `src/lib/api.ts`, `src/lib/api.test.ts`.

- [ ] **Step 1: transcript types** (`lib/transcript.ts`) — the **adjacently-tagged** union (matching `{"type":...,"data":{...}}`):
```ts
export type TranscriptEvent =
  | { type: 'run_start'; data: { run_id: string; agent: string; provider: string; model: string; started_at: string; mode: string } }
  | { type: 'turn_start'; data: Record<string, unknown> }
  | { type: 'assistant_delta'; data: { content: string } }
  | { type: 'assistant_message'; data: { content: string; thinking?: string | null } }
  | { type: 'tool_call'; data: { call_id: string; tool: string; input: unknown } }
  | { type: 'tool_result'; data: { call_id: string; output: string; error?: string | null; duration_ms: number } }
  | { type: 'file_edit'; data: Record<string, unknown> }
  | { type: 'command_run'; data: Record<string, unknown> }
  | { type: 'action_emitted'; data: Record<string, unknown> }
  | { type: 'gate_requested'; data: Record<string, unknown> }
  | { type: 'turn_end'; data: { tokens_in?: number | null; tokens_out?: number | null } }
  | { type: 'usage'; data: { input_tokens: number; output_tokens: number; cached_tokens: number } }
  | { type: 'run_complete'; data: { run_id: string; status: string; total_tokens: number; duration_ms: number; error?: string | null } }
  | { type: string; data: Record<string, unknown> };   // catch-all for forward-compat
export interface TranscriptSummary { run_id: string; agent: string; provider: string; model: string; status: string; total_tokens: number; duration_ms: number; started_at: string; error?: string | null }
export interface TranscriptResponse { events: TranscriptEvent[]; summary: TranscriptSummary | null }
```
   (Verify field names against `reader.rs` RunSummary / `event.rs`.)

- [ ] **Step 2: api methods** (`api.ts`) — `getProjects(): ProjectRow[]`, `getProject(wsId): ProjectDetail`, `getProjectRuns/Sessions/Coverage(wsId)`, `getTranscript(path): Promise<TranscriptResponse>` → GET `/api/transcript?path=${encodeURIComponent(path)}`, `subscribeTranscript(path, onEvent, onError?): () => void` → `new EventSource('/api/transcript/stream?path='+encodeURIComponent(path))`, `es.onmessage = m => onEvent(JSON.parse(m.data))`, returns `() => es.close()`. Add `ProjectRow`/`ProjectDetail` interfaces matching the Task 1/2 JSON.

- [ ] **Step 3: test** — extend `api.test.ts`: `getTranscript` 200 → typed; `getProject` 200 → typed; 404 → ApiError.

- [ ] **Step 4:** `npm run build` (strict) + `npm test -- --run`. Commit `feat(cp/web): api client for projects + transcript`.

---

### Task 6: `TranscriptPanel` (conversation render) + mapping test

**Files:** Create `crates/rupu-cp/web/src/components/TranscriptPanel.tsx`, `src/components/transcript/transcriptView.ts` (+ test).

- [ ] **Step 1: pure mapping + test** — a small pure helper `transcriptView.ts` that, given `TranscriptEvent[]`, produces a render model (groups assistant message + its tool calls into turns; separates thinking; flags findings). Unit-test (`transcriptView.test.ts`): a fixture of `[run_start, assistant_message{thinking}, tool_call, tool_result, run_complete]` → assert the view model has the run header, one assistant turn carrying the thinking + the tool call/result paired by `call_id`, and a completion footer. (Keep it pure + tested; the JSX component consumes it.)

- [ ] **Step 2: implement `transcriptView.ts`** to satisfy the test (pair tool_result to tool_call by `call_id`; attach `thinking`; collect `usage`/`run_complete` into a footer).

- [ ] **Step 3: `TranscriptPanel.tsx`** (contract — rendering validated by matt): props `{ path: string; live: boolean }`. On mount `getTranscript(path)` → set events; if `live`, `subscribeTranscript(path, e => append(e))` (cleanup on unmount; dedupe). Render via `transcriptView`: a header (agent · model · status · total tokens), the conversation (user/assistant bubbles, assistant content as light markdown or plain text, **thinking collapsed** behind a toggle, **tool calls as collapsible cards** — name + input, result/error/duration in a collapsed body, error in red), file-edit/command chips, findings highlighted, a usage footer; a small live/connection indicator when `live`. Match the approved mockup `.superpowers/brainstorm/3629-1781892946/content/transcript-content.html` (conversation style) + the existing palette/`STATE_STYLE`. STATIC Tailwind classes only.

- [ ] **Step 4:** `npm test -- --run transcriptView` green; `npm run build` strict. Commit `feat(cp/web): TranscriptPanel (conversation render) + view mapping`.

---

### Task 7: Projects nav + pages

**Files:** Modify `src/lib/sidebarNav.ts`, `src/App.tsx`; create `src/pages/Projects.tsx`, `src/pages/ProjectDetail.tsx`.

- [ ] **Step 1: nav** — add a top **Projects** leaf to `sidebarNav.ts` (`/projects`, lucide `FolderGit2`) directly under Dashboard, above the Runs group. Keep the existing groups (the firehose).
- [ ] **Step 2: `Projects.tsx`** — `getProjects()` list: each row = name, path (mono), repo/branch, last-run relative time; links to `/projects/:wsId`. Empty state.
- [ ] **Step 3: `ProjectDetail.tsx`** — overview dashboard (mockup `.superpowers/brainstorm/3629-1781892946/content/project-detail.html`, layout A): `getProject(wsId)` → identity header (name, path, repo_remote, branch, last_run_at, ws_id); rollup tiles (Runs + running, Sessions + active, **Coverage %** + bar (from `coverage.assessed_pct`, show "—" when null), Findings); sections — Recent runs (→ `/runs/:id`), Coverage (→ `/projects/:wsId/coverage`), Sessions (→ session detail) — each "see all" to the scoped route. Reuse `ListCard`/`SectionHeader`/`StatusPill`/`lib/time`.
- [ ] **Step 4: routes** — `App.tsx`: `/projects`→Projects, `/projects/:wsId`→ProjectDetail, plus `/projects/:wsId/{runs,sessions,coverage}` (can reuse the firehose list pages with a `wsId` param filter, or simple scoped pages). Build strict + tests. Commit `feat(cp/web): Projects nav + project pages`.

---

### Task 8: split-pane run detail + click-node→transcript

**Files:** Modify `src/pages/RunDetail.tsx`, `src/components/RunGraph.tsx` (node-click → select).

- [ ] **Step 1: select plumbing** — `RunGraph` already has `onOpenUnit(stepId, index)`; add/confirm an `onSelectNode(node)` (or reuse onOpenUnit + a step-node click) that yields the clicked node's **transcript path + live flag**. For a step: the path is the step's `transcript_path` (from `step_results`); for a unit: `unit.transcriptPath`; live = the node state is `running`. (The graph model's `GraphNode`/`UnitView` already carry these — thread `transcript_path` onto the step node from `step_results` in `runGraphModel` if not already present; add it there with a test if missing.)
- [ ] **Step 2: split-pane** — `RunDetail.tsx`: render `RunGraph` (top) + `<TranscriptPanel path={selectedPath} live={selectedLive} />` (bottom) in a vertical split. Default `selectedPath` = the active/most-recent node's transcript. Clicking a node updates `{selectedPath, selectedLive}`. Keep the Events tab/toggle available. Preserve the existing single run-log SSE subscription (the transcript stream is separate, inside TranscriptPanel).
- [ ] **Step 3:** build strict + tests. Commit `feat(cp/web): split-pane run detail (graph + transcript)`.

---

### Task 9: click-to-transcript from lists + RunTranscript + sessions-as-containers

**Files:** Create `src/pages/RunTranscript.tsx`; modify `src/pages/runs/AgentRuns.tsx`, `src/pages/SessionDetail.tsx`, `src/App.tsx`.

- [ ] **Step 1: `RunTranscript.tsx`** — a transcript-only page for runs with no graph (agent/session/standalone). Reads a `?path=` (or a route param carrying the encoded path) + a `live` flag → header + `<TranscriptPanel>`. Route `/transcript?path=...` (or `/runs/agent/:run/transcript`).
- [ ] **Step 2: AgentRuns rows → transcript** — each `AgentRunRow` carries `transcript_path` → row links to `RunTranscript` with that path (encoded) + `live` from status. Workflow/autoflow run rows continue to `/runs/:id` (the split-pane).
- [ ] **Step 3: sessions-as-containers** — `SessionDetail.tsx`: show the session's turn-runs as a list (group `getAgentRuns()` by `session_id === :id`, OR if a scoped endpoint is cheaper, use it) — each row: prompt preview, status, tokens, time → links to `RunTranscript` for that turn's transcript. Header shows the session identity. ProjectDetail's Sessions section + Fleet ▸ Sessions link here.
- [ ] **Step 4:** build strict + tests. Commit `feat(cp/web): click-to-transcript + sessions-as-containers`.

---

## Self-review

**Spec coverage:** Projects-as-root nav ✓ (T7); `/api/projects` + rollup ✓ (T1/T2); scoped lists ✓ (T2/T7); transcript endpoint path-validated ✓ (T3) + SSE live-tail ✓ (T4); transcript types adjacently-tagged ✓ (T5); TranscriptPanel conversation render ✓ (T6); split-pane run detail ✓ (T8); click-to-transcript from nodes ✓ (T8) + list rows ✓ (T9); sessions-as-containers ✓ (T9); basic coverage rollup incl. audit % ✓ (T2/T7); read-adapter only (rupu-workspace/rupu-transcript path deps, no rupu-cli) ✓.

**Placeholder scan:** the backend "adapt to the real `iter()` item type" (T3) and "verify the minimal Workspace TOML" (T1) are flagged verification steps with the exact file to read, not hand-waves; the code is concrete. No TBD/await-later.

**Type consistency:** `ProjectRow`/`ProjectDetail` shared T1↔T2↔T5↔T7; `TranscriptEvent` adjacently-tagged in T5 consumed by `transcriptView` in T6; `validate_transcript_path(raw, &[PathBuf])` signature consistent T3↔T4; `getTranscript`/`subscribeTranscript(path)` consistent T5↔T6↔T8↔T9; the transcript path threaded from `step_results`/`UnitView.transcriptPath` (T8) matches the api types (T5).

**Notes for the executor:** rupu rules — workspace dep versions in root Cargo.toml (the two new deps are path deps, fine); `rupu-cp` stays a read adapter (no rupu-cli); `#![deny(clippy::all)]` incl `--all-targets`; never package-wide `cargo fmt`. The transcript path validator is SECURITY-CRITICAL — its traversal/out-of-root rejection test is the most important backend test. The web UI is matt-validated. Backend Tasks 1–4 and frontend 5–6 can interleave; 7–9 depend on 5–6. Stacks on #320/#321.
