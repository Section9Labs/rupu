# rupu-cp Phase 3a — Edit agent `.md` in the browser — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Edit / create / delete agent definitions from the web CP. Pure-state file writes validated by `AgentSpec::parse` before save (rupu-cp deps rupu-agent). First slice of CP Phase 3 (Authoring); 3b (workflow yaml) reuses the pattern.

**Design source:** the Phase-3a surface analysis (this session). CP roadmap: `docs/superpowers/specs/2026-06-18-rupu-control-plane-design.md`.

**Constraints:** no `any` (TS); static Tailwind; recharts out of main chunk; the code editor must be lazy-chunked (not in the main bundle); stage only specific files; never `-A`/`.rupu/*`; never package-wide `cargo fmt`. rupu-cp/web clean on worktree 1.95.

## Key facts (from analysis)
- `GET /api/agents/:name` already returns `AgentDetailDto { …, system_prompt, raw }` — `raw` is the full `.md`. **Read exists.**
- `rupu_agent::spec::AgentSpec::parse(&str) -> Result<AgentSpec, AgentSpecParseError>` validates (only `name:` required; `#[serde(deny_unknown_fields)]` rejects typos; clean `Display` errors). Callable from rupu-cp.
- Global agents at `global_dir/agents/<name>.md`; `loader::load_agent(&global_dir, None, name)`. Agents keyed by parsed `name:`, NOT file stem → pin filename to the declared/url name + reject mismatch (see Task 1).
- No CP sanitizer or atomic-write helper → port `validate_name` (start with ASCII letter; only `[A-Za-z0-9_-]`) + a tiny tmp+rename `write_atomic` into the agents module.
- No agent `delete` engine fn → CP `fs::remove_file` directly.
- AgentDetail.tsx renders `agent.raw` via `<CodeHighlight>` (read-only) — swap in an Edit mode. CodeMirror 6 (`@codemirror/*`, lang-markdown, lang-yaml) is **installed but unused** — use it lazy-chunked, or a textarea fallback.

---

### Task 1: Backend — agent write/create/delete endpoints

**Files:** Modify `crates/rupu-cp/src/api/agents.rs`.

- [ ] **Step 1: helpers.** Add a private `fn validate_name(name: &str) -> Result<(), ApiError>` (must start with an ASCII letter; only `[A-Za-z0-9_-]`; else `ApiError::bad_request("invalid agent name")` — rejects `/`, `.`, `..`). Add `fn write_atomic(path, bytes) -> io::Result<()>` (write `path.with_extension("tmp")` then `fs::rename`). `fn agents_dir(s) -> PathBuf { s.global_dir.join("agents") }`.
- [ ] **Step 2: Write failing tests** (over a tempdir `AppState`/`global_dir`):
  - `PUT /api/agents/:name` with a valid `.md` whose frontmatter `name:` == `:name` → writes `global_dir/agents/<name>.md`, 200; re-reading returns the new `raw`.
  - PUT with an UNPARSEABLE `.md` (e.g. no frontmatter) → 400 with the parse-error message; file NOT written.
  - PUT where frontmatter `name:` ≠ url `:name` → 400 (mismatch).
  - `POST /api/agents` with valid `.md` → derives the file from the parsed `name:`, writes it, 201/200; a second POST of the same name → 409.
  - `DELETE /api/agents/:name` → removes the file (200); deleting an absent agent → 404.
  - `validate_name` rejects `../evil`, `a/b`, `.`.
  - (Factor the parse+target logic into a pure helper if it helps testing; mirror the existing agents.rs test idiom.)
- [ ] **Step 3: Run `cargo test -p rupu-cp agents`, confirm failure.**
- [ ] **Step 4: Implement** in `agents::routes()` (already merged in server.rs):
  - `PUT /api/agents/:name` — `#[derive(Deserialize)] struct AgentWriteBody { raw: String }`. `validate_name(&name)?`; `AgentSpec::parse(&body.raw)` → `ApiError::bad_request(e.to_string())` on Err; if `spec.name != name` → `ApiError::bad_request("frontmatter name must equal the agent name")`; `create_dir_all(agents_dir)`; `write_atomic(agents_dir/<name>.md, raw)`; return `Json` of the reloaded `AgentDetailDto` (or 200 `{ "ok": true }`).
  - `POST /api/agents` — body `{ raw }`. `AgentSpec::parse` (400 on err); `name = spec.name`; `validate_name(&name)?`; target = `agents_dir/<name>.md`; if it exists → `ApiError::conflict("agent already exists")`; write_atomic; return the created agent.
  - `DELETE /api/agents/:name` — `validate_name`; `target = agents_dir/<name>.md`; if absent → `ApiError::not_found`; `fs::remove_file`; 200 `{ "deleted": true }`.
  - Routes: add `.route("/api/agents/:name", put(write_agent).delete(delete_agent))` + `.route("/api/agents", post(create_agent))` to the existing `routes()` (import `axum::routing::{put, post, delete}`; the existing `get` routes stay).
- [ ] **Step 5:** `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` green/clean.
- [ ] **Step 6: Commit.** `git add crates/rupu-cp/src/api/agents.rs` → `feat(cp): agent write/create/delete endpoints (validated .md)`.

---

### Task 2: Frontend — agent editor + create/delete

**Files:** Create `crates/rupu-cp/web/src/components/CodeEditor.tsx` (lazy CodeMirror, or textarea); Modify `crates/rupu-cp/web/src/lib/api.ts`, `crates/rupu-cp/web/src/pages/AgentDetail.tsx`, `crates/rupu-cp/web/src/pages/Agents.tsx`; Test.

- [ ] **Step 1: `api.ts`.** `saveAgent(name: string, raw: string): Promise<void>` → `PUT /api/agents/:name` body `{ raw }`; `createAgent(raw: string): Promise<{ name?: string }>` → `POST /api/agents` body `{ raw }`; `deleteAgent(name: string): Promise<void>` → `DELETE /api/agents/:name`. Use the existing `request` wrapper (it throws `ApiError` with `.body`/message). No `any`.
- [ ] **Step 2: `CodeEditor.tsx`** — a controlled code editor `{ value: string; onChange: (v: string) => void; language?: 'markdown'|'yaml' }`. Prefer **CodeMirror 6** (already installed: `@codemirror/{state,view,commands,language,lang-markdown,lang-yaml}`) built directly on `EditorView`/`EditorState` in a `useRef`+`useEffect` (create on mount, reconfigure on external value change, dispatch onChange from an updateListener). **Lazy-load it** (`React.lazy` + a `<Suspense>` fallback, or dynamic import) so CodeMirror lands in its OWN chunk, NOT the main bundle. If CodeMirror wiring proves problematic, fall back to a monospace `<textarea>` (note it) — correctness over polish. Static Tailwind for the container.
- [ ] **Step 3: `AgentDetail.tsx` edit mode.** The Definition section currently shows `<CodeHighlight code={agent.raw} frontmatter />`. Add an **Edit** button → swaps to `<CodeEditor value={draft} onChange={setDraft} language="markdown" />` + **Save** / **Cancel**. Save → `saveAgent(name, draft)`; on success exit edit mode + re-fetch (`getAgent`); on failure show the inline error (`ApiError` message — the parse error). Disable Save while in-flight / when unchanged. Also add a **Delete** action (confirm → `deleteAgent(name)` → navigate back to `/agents`).
- [ ] **Step 4: `Agents.tsx` create.** A **New agent** button → a small modal/inline editor seeded with a minimal template (`---\nname: my-agent\ndescription: ...\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou are ...`) using `<CodeEditor>` → **Create** → `createAgent(raw)` → on success navigate to the new agent (`/agents/:name`); inline error on 400/409.
- [ ] **Step 5: Test** (`AgentDetail.test.tsx` + an `Agents`/CodeEditor test as feasible): clicking Edit shows the editor seeded with `raw`; Save calls `saveAgent(name, draft)`; a 400 surfaces the error message; Delete (confirmed) calls `deleteAgent`. New-agent Create calls `createAgent`. Mock the api + `CodeEditor` (so tests don't need a real CodeMirror instance — e.g. mock it to a textarea).
- [ ] **Step 6:** `npm test -- --run` + `npm run build` green/exit 0; `grep -c recharts dist/assets/index-*.js` → 0; confirm CodeMirror is in its OWN chunk (not `index-*.js`) — report the main chunk size + that `@codemirror`/`codemirror` doesn't appear in `index-*.js`.
- [ ] **Step 7: Commit.** `git add crates/rupu-cp/web/src/components/CodeEditor.tsx crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/pages/AgentDetail.tsx crates/rupu-cp/web/src/pages/Agents.tsx <tests>` → `feat(cp/web): agent editor — edit/create/delete with validation`.

---

### Final verification
- `cargo test -p rupu-cp` green; clippy clean. `npm test -- --run` green; `npm run build` strict; recharts + CodeMirror out of the main chunk.
- Final review (validate-before-write; name-safety rejects traversal; mismatch handling; create-overwrite 409; delete 404; lazy editor chunk), then matt visual-validates editing an agent.
- TODO note: 3a is GLOBAL agents only (project-agent editing + the visual editor are follow-ups); CodeMirror-vs-textarea outcome noted.
