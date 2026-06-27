# rupu-cp Phase 3b — Edit workflow `.yaml` in the browser — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Edit / create / delete workflow `.yaml` definitions from the web CP, validated by `rupu_orchestrator::Workflow::parse` before save. Second slice of CP Phase 3 (Authoring); mirrors 3a (agent `.md` editor, merged in #377) almost exactly.

**Design source:** the Phase-3 surface analysis. CP roadmap: `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md`. **Reference implementation to mirror:** the agent editor merged in #377 — `crates/rupu-cp/src/api/agents.rs` (PUT/POST/DELETE + `validate_name`/`write_atomic`), `crates/rupu-cp/web/src/pages/AgentDetail.tsx` (Edit/Save/Delete), `crates/rupu-cp/web/src/pages/Agents.tsx` (New-agent modal), `crates/rupu-cp/web/src/lib/api.ts` (`saveAgent`/`createAgent`/`deleteAgent`), `crates/rupu-cp/web/src/components/CodeEditor.tsx` (lazy CodeMirror — already supports `language="yaml"`).

**Constraints:** no `any` (TS); static Tailwind; recharts + CodeMirror stay OUT of the main chunk; stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. rupu-cp/web clean on worktree (Homebrew 1.95).

## Key facts (from analysis)
- `GET /api/workflows/:name` already returns `{ workflow, yaml, usage }` — **`yaml` is the full raw file. Read exists.**
- `rupu_orchestrator::Workflow::parse(&str) -> Result<Workflow, WorkflowParseError>` validates (non-empty, unique step ids, max_parallel, triggers) with a clean `Display`. Already imported in `workflows.rs`.
- Workflows are keyed by **file stem**; the file lives at `global_dir/workflows/<name>.yaml`. `Workflow` HAS a `name: String` field → enforce `workflow.name == url_name` on PUT (mirrors agents) so the file stem and declared name stay coherent. On POST derive the name from `workflow.name`.
- The agent endpoints already define private `validate_name` (start ASCII letter; `[A-Za-z0-9_-]` only — rejects `/ . ..`) and `write_atomic` (tmp+rename) in `agents.rs`. **Lift both into a shared `crate::api::fs_safety` module** so workflows reuses them (DRY); refactor `agents.rs` to import them (behavior identical — its tests must stay green).
- `ApiError::bad_request / not_found / conflict / internal` all exist (agents.rs uses them).
- `WorkflowDetail.tsx` renders `detail.yaml` via `<CodeHighlight language="yaml">` (read-only) — add an Edit mode. `Workflows.tsx` lists workflows — add a New-workflow modal.

---

### Task 1: Backend — shared fs-safety helpers + workflow write/create/delete endpoints

**Files:** Create `crates/rupu-cp/src/api/fs_safety.rs`; Modify `crates/rupu-cp/src/api/mod.rs` (declare module), `crates/rupu-cp/src/api/agents.rs` (use the shared helpers), `crates/rupu-cp/src/api/workflows.rs` (new endpoints + tests).

- [ ] **Step 1: Shared module.** Create `crates/rupu-cp/src/api/fs_safety.rs` with `pub(crate) fn validate_name(name: &str) -> Result<(), crate::error::ApiError>` and `pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()>`, lifted verbatim from `agents.rs`. Declare `mod fs_safety;` in `crates/rupu-cp/src/api/mod.rs` (match the existing `mod` style there). In `agents.rs`, delete the private `validate_name`/`write_atomic` and replace call sites with `crate::api::fs_safety::{validate_name, write_atomic}` (or a `use`). Keep `agents_dir` private in agents.rs.
- [ ] **Step 2: Run `cargo test -p rupu-cp agents` — confirm STILL GREEN** (the refactor is behavior-preserving; the existing `validate_name_rejects_traversal_and_accepts_plain` test moves with the fn — relocate it into `fs_safety.rs`'s test module or keep an equivalent there).
- [ ] **Step 3: Write failing tests in `workflows.rs`** (mirror the agents.rs test idiom — tempdir `AppState` with `global_dir`; a minimal valid workflow yaml is e.g. `name: demo\nsteps:\n  - id: one\n    agent: x\n`):
  - `PUT /api/workflows/:name` with valid yaml whose `name:` == `:name` → writes `global_dir/workflows/<name>.yaml`, 200; re-reading via `get_workflow` returns the new `yaml`.
  - PUT with UNPARSEABLE yaml (e.g. empty, or duplicate step ids) → 400 with the parse-error message; file NOT written (assert the path is unchanged / absent).
  - PUT where yaml `name:` ≠ url `:name` → 400 (mismatch message).
  - `POST /api/workflows` with valid yaml → derives the file from parsed `name`, writes it, 200; a second POST of the same name → 409.
  - `DELETE /api/workflows/:name` → removes the file (200); deleting an absent workflow → 404.
- [ ] **Step 4: Run `cargo test -p rupu-cp workflows` — confirm failure.**
- [ ] **Step 5: Implement** in `workflows.rs`:
  - `fn workflows_dir(s: &AppState) -> PathBuf { s.global_dir.join("workflows") }`.
  - `#[derive(Deserialize)] struct WorkflowWriteBody { raw: String }`.
  - `PUT /api/workflows/:name` (`write_workflow`): `fs_safety::validate_name(&name)?`; `Workflow::parse(&body.raw).map_err(|e| ApiError::bad_request(e.to_string()))?`; if `wf.name != name` → `ApiError::bad_request("workflow name must equal the workflow file name")`; `fs::create_dir_all(workflows_dir)`; `fs_safety::write_atomic(workflows_dir/<name>.yaml, raw.as_bytes())`; return the reloaded detail (reuse `get_workflow`'s body shape — factor a small `fn load_detail(&s, &name) -> ApiResult<Json<Value>>` if convenient, or just re-read + re-parse + json! the same `{workflow, yaml, usage}`).
  - `POST /api/workflows` (`create_workflow`): body `{ raw }`; `Workflow::parse` (400 on err); `name = wf.name`; `validate_name(&name)?`; target = `workflows_dir/<name>.yaml`; if `target.exists()` → `ApiError::conflict("workflow already exists")`; `create_dir_all` + `write_atomic`; return the created detail.
  - `DELETE /api/workflows/:name` (`delete_workflow`): `validate_name`; target = `workflows_dir/<name>.yaml`; if `!target.exists()` → `ApiError::not_found`; `fs::remove_file`; `Json(json!({ "deleted": true }))`.
  - Routes: change to `.route("/api/workflows", get(list_workflows).post(create_workflow))` and `.route("/api/workflows/:name", get(get_workflow).put(write_workflow).delete(delete_workflow))` (import `axum::routing::{put, delete}`; `post` already imported). Leave `/:name/run` as-is.
- [ ] **Step 6:** `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.
- [ ] **Step 7: Commit.** `git add crates/rupu-cp/src/api/fs_safety.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/api/agents.rs crates/rupu-cp/src/api/workflows.rs` → `feat(cp): workflow write/create/delete endpoints (validated .yaml)`.

---

### Task 2: Frontend — workflow editor + create/delete

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx`, `crates/rupu-cp/web/src/pages/Workflows.tsx`; Test (`WorkflowDetail.test.tsx`, optionally `Workflows` test).

- [ ] **Step 1: `api.ts`.** Mirror the agent fns (right after `getWorkflow`): `saveWorkflow(name: string, raw: string): Promise<WorkflowDetail>` → `PUT /api/workflows/:name` body `{ raw }`; `createWorkflow(raw: string): Promise<WorkflowDetail>` → `POST /api/workflows` body `{ raw }`; `deleteWorkflow(name: string): Promise<void>` → `DELETE /api/workflows/:name`. Use the existing `request` wrapper. No `any`.
- [ ] **Step 2: `WorkflowDetail.tsx` edit mode.** Reuse the existing `CodeEditor` component (`import CodeEditor from '../components/CodeEditor'`; it supports `language="yaml"`). The "YAML" section currently shows `<CodeHighlight code={detail.yaml} language="yaml" />`. Add an **Edit** button (in that section's header, or beside Run) → swaps to `<CodeEditor value={draft} onChange={setDraft} language="yaml" ariaLabel="Workflow YAML editor" />` + **Save** / **Cancel**. Save → `api.saveWorkflow(name, draft)`; on success exit edit mode + replace `detail` with the returned value (or re-fetch); on failure show an inline `role="alert"` error (the `ApiError` message — the parse error). Disable Save while in-flight or when `draft === detail.yaml`. Add a **Delete** action (confirm → `api.deleteWorkflow(name)` → `navigate('/workflows')` — import `useNavigate`). Mirror AgentDetail.tsx state/handlers exactly.
- [ ] **Step 3: `Workflows.tsx` create.** A **New workflow** button → a small modal/inline editor (mirror the Agents.tsx New-agent modal) seeded with a minimal template:
  ```yaml
  name: my-workflow
  description: ...
  steps:
    - id: step-one
      agent: my-agent
  ```
  → **Create** → `api.createWorkflow(raw)` → on success `navigate('/workflows/' + name)` (derive the new name from the returned `detail.workflow.name`, narrowed defensively like the rest of the page, falling back to re-list); inline error on 400/409.
- [ ] **Step 4: Test** (`WorkflowDetail.test.tsx`; mock `../lib/api` and mock `../components/CodeEditor` to a plain `<textarea>` so tests don't need a real CodeMirror — mirror `AgentDetail.test.tsx`): clicking Edit shows the editor seeded with `detail.yaml`; Save calls `saveWorkflow(name, draft)`; a 400 surfaces the error; Save disabled when unchanged; Delete (confirmed) calls `deleteWorkflow`. If feasible, a Workflows New→Create test.
- [ ] **Step 5:** `npm test -- --run` + `npm run build` green/exit 0; `grep -c recharts dist/assets/index-*.js` → 0; confirm `@codemirror`/`codemirror` does NOT appear in `dist/assets/index-*.js` (own chunk); report the main chunk size.
- [ ] **Step 6: Commit.** `git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/WorkflowDetail.tsx crates/rupu-cp/web/src/pages/Workflows.tsx <tests>` → `feat(cp/web): workflow editor — edit/create/delete with validation`.

---

### Final verification
- `cargo test -p rupu-cp` green; clippy clean (incl. the agents.rs refactor stays green). `npm test -- --run` green; `npm run build` strict; recharts + CodeMirror out of the main chunk.
- Final review (validate-before-write; name-safety rejects traversal on BOTH url name and POST-derived name; mismatch handling; create-overwrite 409; delete 404; lazy editor chunk; the agents.rs helper refactor is behavior-preserving).
- TODO note: 3b is GLOBAL workflows only (project-workflow editing + 3c visual DAG editor are follow-ups).
