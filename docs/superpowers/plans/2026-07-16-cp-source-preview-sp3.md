# CP source preview SP3 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Serve a workspace-scoped slice of a source file from the CP backend and turn CP `path:line` references (ast_grep match headers, finding locations) into clickable, syntax-highlighted source previews. Local runs only; remote deferred.

**Architecture:** New axum `GET /api/runs/:id/source` endpoint reads a line-window under the run's `RunRecord.workspace_path` using the existing `transcript.rs` path-safety guard; returns a `SourceSlice` JSON. The web app gains `api.readSource` + a lazy `SourcePreview` component, wired into `AstGrepBody` (SP2) and `FindingCard`.

**Tech Stack:** Rust + axum (`crates/rupu-cp`); React + TS + highlight.js (`crates/rupu-cp/web`). No new crate or npm deps.

**Spec:** `docs/superpowers/specs/2026-07-16-cp-source-preview-sp3-design.md`.

## Global Constraints

- Rust 2021; do NOT run workspace-wide `cargo fmt` — `rustfmt --edition 2021` only touched files. Ignore pre-existing warnings/clippy failures in untouched crates (scope clippy to `-p rupu-cp`).
- No new crate deps; no new npm deps (highlight.js is already a CP dependency — new language grammars are bundled with it).
- **Local-first:** `RunLocation::Global` / `ProjectLocal` → read from disk; `RunLocation::Host` (or `host` query != `local`) → return `{ available: false, reason: "Source preview is not available for remote-host runs yet." }` (HTTP 200). `NotFound`/`Unpersisted` → 404.
- **Path safety is mandatory:** resolve the requested `path` under `record.workspace_path` by copying `crates/rupu-cp/src/api/transcript.rs`'s `validate_transcript_path` approach (reject `..`, canonicalize deepest existing ancestor, require `starts_with` the canonicalized workspace root), MINUS the `.jsonl` extension requirement. A traversal/symlink escape must yield 400, never read outside the workspace.
- `MAX_SOURCE_BYTES = 2 * 1024 * 1024`; oversize → `{ available: false, reason: "File too large to preview" }`.
- `context` default 20, clamp to `[0, 200]`. Line numbers 1-based.
- `SourceSlice` response shape (backend emits, frontend consumes — keys must match exactly): `{ available: bool, path?, language?: string|null, startLine?, endLine?, targetLine?, totalLines?, lines?: [{n, text}], reason?: string }`.

---

### Task 1: Backend `GET /api/runs/:id/source` endpoint

**Files:**
- Create: `crates/rupu-cp/src/api/source.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs` (add `pub mod source;` alphabetically)
- Modify: `crates/rupu-cp/src/server.rs` (add `.merge(crate::api::source::routes())` in `router()`)
- Test: inline `#[cfg(test)]` in `source.rs` (+ an integration test under `crates/rupu-cp/tests/` if that is the crate's convention)

**Interfaces:**
- Produces: `GET /api/runs/:id/source?path=&line=&context=&host=` → `Json<SourceSlice>`; the `SourceSlice` shape above. Consumed by Task 2's `api.readSource`.

- [ ] **Step 1: Read the patterns to copy**

Read these first (do not guess their signatures):
- `crates/rupu-cp/src/api/transcript.rs` — copy `validate_transcript_path` (path-safety) and the handler/`routes()`/`ApiResult<Json<T>>`/`State(s)`/`Query(q)` shape, and the local-vs-remote `resolve_run_location` match (also shown in `crates/rupu-cp/src/api/runs.rs:958-981`).
- `crates/rupu-cp/src/api/run_resolve.rs` — `resolve_run_location` + `RunLocation` variants.
- `crates/rupu-orchestrator/src/runs.rs` (~line 103-123) — `RunRecord` with `workspace_path: PathBuf`.
- `crates/rupu-cp/src/state.rs` — `AppState` (has `run_store: Arc<RunStore>`).
- `crates/rupu-cp/src/error.rs` — `ApiError`/`ApiResult` constructors (`not_found`, bad-request).

- [ ] **Step 2: Write the failing tests**

In `source.rs` `#[cfg(test)] mod tests`, add tests for the PURE helpers (so they don't need a running server):

```rust
// window extraction: given total lines, a target line, and context, returns
// (start, end) 1-based inclusive, clamped to [1, total].
#[test]
fn window_clamps_at_bounds() {
    assert_eq!(source_window(219, 46, 20), (26, 66));
    assert_eq!(source_window(219, 1, 20), (1, 21));   // clamp at start
    assert_eq!(source_window(219, 219, 20), (199, 219)); // clamp at end
    assert_eq!(source_window(5, 3, 20), (1, 5));       // window > file
    assert_eq!(source_window(0, 1, 20), (1, 1));       // empty file guard
}

#[test]
fn rejects_path_traversal() {
    let root = std::path::Path::new("/tmp/ws-does-not-matter");
    assert!(resolve_under_workspace(root, "../etc/passwd").is_err());
    assert!(resolve_under_workspace(root, "/etc/passwd").is_err());
}
```

(Adapt `source_window`/`resolve_under_workspace` names to what you implement; keep them pure/free functions so they're unit-testable without axum.)

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-cp source::tests`
Expected: FAIL to compile (functions absent).

- [ ] **Step 4: Implement the module**

Create `crates/rupu-cp/src/api/source.rs` with:
- `SourceSlice` + `SourceLine` serde structs matching the Global-Constraints shape (`#[serde(rename_all = "camelCase")]`, `skip_serializing_if = "Option::is_none"` on the optional fields; `available: bool` always present).
- Pure helpers: `fn source_window(total: usize, target: usize, context: usize) -> (usize, usize)` (1-based inclusive, clamped, empty-file → `(1,1)`); `fn resolve_under_workspace(workspace: &Path, rel: &str) -> Result<PathBuf, ApiError>` (copy `validate_transcript_path`'s traversal+symlink+`starts_with` logic, no `.jsonl` check); `fn detect_language(path: &Path) -> Option<&'static str>` (map extensions `rs→rust, py→python, ts/tsx→typescript, js/jsx→javascript, go→go, json→json, toml→toml, md→markdown, yaml/yml→yaml`, else None).
- `async fn get_source(Path(id): Path<String>, Query(q): Query<SourceQuery>, State(s): State<AppState>) -> ApiResult<Json<SourceSlice>>` implementing the flow in the spec (§Backend): resolve run → local/remote branch → validate path → size guard → read → window → build slice. Remote/oversize/other soft-fails return `Json(SourceSlice { available: false, reason: Some(..), ..Default })` (derive `Default`); unknown run → `ApiError::not_found`.
- `pub fn routes() -> Router<AppState> { Router::new().route("/api/runs/:id/source", get(get_source)) }`.

Register: add `pub mod source;` to `crates/rupu-cp/src/api/mod.rs` (alphabetical), and `.merge(crate::api::source::routes())` in `crates/rupu-cp/src/server.rs`'s `router()` beside the other merges.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-cp source`
Expected: PASS (pure-helper tests green). Then `cargo build -p rupu-cp` clean.

- [ ] **Step 6: Add an endpoint-level test (if the crate has an axum test harness)**

If `crates/rupu-cp/tests/` shows a pattern for exercising a route (e.g. building the `router()` with a test `AppState` and calling via `tower::ServiceExt::oneshot`), add one test: a run whose `workspace_path` is a tempdir with a known file → `GET .../source?path=x.rs&line=2&context=1` returns `available:true` with the right window; a `path=../escape` → 400. If no such harness exists, note it and rely on the pure-helper tests + manual verification.

- [ ] **Step 7: Lint, format, commit**

`cargo clippy -p rupu-cp` (clean on `source.rs`); `rustfmt --edition 2021` the 3 touched files.

```bash
git add crates/rupu-cp/src/api/source.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs
git commit -m "feat(cp): workspace-scoped source-slice endpoint (GET /api/runs/:id/source)"
```

---

### Task 2: Web `api.readSource` + `SourcePreview` component

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (add `readSource` + `SourceSlice` type)
- Create: `crates/rupu-cp/web/src/components/transcript/SourcePreview.tsx`
- Modify: `crates/rupu-cp/web/src/components/CodeHighlight.tsx` (register rust/python/typescript/javascript/go/json grammars)
- Test: `crates/rupu-cp/web/src/components/transcript/SourcePreview.test.tsx`

**Interfaces:**
- Consumes: `GET /api/runs/:id/source` (Task 1).
- Produces: `api.readSource(runId, path, line, opts?)` and `<SourcePreview runId path line />`. Consumed by Task 3.

- [ ] **Step 1: Read patterns**

Read `crates/rupu-cp/web/src/lib/api.ts` (`getTranscript` ~1835, the `request<T>` helper ~34) and `crates/rupu-cp/web/src/components/CodeHighlight.tsx` (how it registers hljs languages + renders). Match their idioms.

- [ ] **Step 2: Add `readSource` + type + a URL test**

In `api.ts` add the type and method:

```ts
export interface SourceSlice {
  available: boolean;
  path?: string;
  language?: string | null;
  startLine?: number;
  endLine?: number;
  targetLine?: number;
  totalLines?: number;
  lines?: { n: number; text: string }[];
  reason?: string;
}
```
```ts
  readSource(runId: string, path: string, line: number, opts?: { context?: number; host?: string }): Promise<SourceSlice> {
    let url = `/api/runs/${encodeURIComponent(runId)}/source?path=${encodeURIComponent(path)}&line=${line}`;
    if (opts?.context != null) url += `&context=${opts.context}`;
    if (opts?.host) url += `&host=${encodeURIComponent(opts.host)}`;
    return request<SourceSlice>(url);
  },
```

Add a unit test (mirror existing api tests if any) asserting the built URL encodes path and includes line.

- [ ] **Step 3: Register highlight grammars**

In `CodeHighlight.tsx`, register the additional hljs languages (import from `highlight.js/lib/languages/<lang>` and `hljs.registerLanguage(...)`) for rust, python, typescript, javascript, go, json — following the existing yaml/markdown/ini registrations. (No new dependency — these ship with `highlight.js`.)

- [ ] **Step 4: Implement `SourcePreview` + tests (TDD)**

Write `SourcePreview.test.tsx` first (mock `api.readSource`): (a) shows loading then a line-numbered slice with the target line emphasized; (b) `available:false` renders the `reason` text; (c) fetches lazily (only when mounted/opened). Then implement `SourcePreview.tsx`:

```tsx
export function SourcePreview({ runId, path, line, host }: { runId: string; path: string; line: number; host?: string }) {
  const [state, setState] = React.useState<{ loading: boolean; slice?: SourceSlice; error?: string }>({ loading: true });
  React.useEffect(() => {
    let alive = true;
    api.readSource(runId, path, line, { host })
      .then((slice) => alive && setState({ loading: false, slice }))
      .catch((e) => alive && setState({ loading: false, error: String(e) }));
    return () => { alive = false; };
  }, [runId, path, line, host]);

  if (state.loading) return <div className="text-xs text-slate-400">Loading source…</div>;
  if (state.error) return <div className="text-xs text-rose-500">Could not load source: {state.error}</div>;
  const s = state.slice!;
  if (!s.available) return <div className="text-xs text-slate-500">{s.reason ?? "Source not available."}</div>;
  return (
    <div className="mt-1 overflow-x-auto rounded border border-slate-200 bg-slate-50 text-xs">
      {(s.lines ?? []).map((ln) => (
        <div key={ln.n} className={`flex ${ln.n === s.targetLine ? "bg-amber-100" : ""}`}>
          <span className="select-none pr-2 text-right text-slate-400" style={{ minWidth: "3ch" }}>{ln.n}</span>
          <code className="whitespace-pre">{s.language ? <CodeHighlight code={ln.text} language={s.language} inline /> : ln.text}</code>
        </div>
      ))}
    </div>
  );
}
```

(Adapt `CodeHighlight`'s actual prop API — if it highlights whole blocks rather than single lines, highlight the joined slice once and split, or render the block; match the real component. Keep the target-line emphasis.)

- [ ] **Step 5: Verify**

`cd crates/rupu-cp/web && npx vitest run SourcePreview` (green); `npx tsc --noEmit` (clean).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/components/transcript/SourcePreview.tsx crates/rupu-cp/web/src/components/transcript/SourcePreview.test.tsx crates/rupu-cp/web/src/components/CodeHighlight.tsx
git commit -m "feat(cp): readSource API + SourcePreview component"
```

---

### Task 3: Wire clickable `path:line` into `AstGrepBody` + `FindingCard`

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/ToolCard.tsx` (`AstGrepBody`: clickable header → toggle inline `SourcePreview`)
- Modify: `crates/rupu-cp/web/src/components/transcript/FindingCard.tsx` (location chip → clickable)
- Modify: whatever threads context into these (the transcript view / page) to supply `runId`
- Test: extend `ToolCard.test.tsx` / `FindingCard.test.tsx`

**Interfaces:**
- Consumes: `<SourcePreview />` (Task 2), `ToolView` (SP2).

- [ ] **Step 1: Trace how `runId` reaches the cards**

Read `crates/rupu-cp/web/src/pages/RunTranscript.tsx` and `components/TranscriptPanel.tsx` and `Turn.tsx` to find how a run id / transcript path is known at the page level and how props flow down to `ToolCard`/`FindingCard`. Decide the smallest thread: a `runId` prop passed `TranscriptPanel → Turn → ToolCard → AstGrepBody`, and to `FindingCard`. (If a run id isn't directly available but the transcript `path` is, thread whatever the `source` endpoint needs — the endpoint keys on run id, so prefer the run id; if only a path is present, note it as a concern and thread the run id from the page route param.)

- [ ] **Step 2: Wire `AstGrepBody` (TDD)**

Add a test to `ToolCard.test.tsx`: rendering an ast_grep tool with `runId` and clicking a match's `file:line` header mounts a `SourcePreview` (mock `api.readSource`, assert it's called with the file + line). Then implement: make each match header a `<button>` that toggles a per-match `useState` boolean; when open, render `<SourcePreview runId={runId} path={m.file} line={m.range.startLine} />` beneath the match. Thread `runId` into `AstGrepBody`'s props (and through `ToolCard`).

- [ ] **Step 3: Wire `FindingCard` (TDD)**

Add a test to `FindingCard.test.tsx`: the location chip is now a button; clicking it (with `runId` provided) mounts `SourcePreview` with the finding's `filePath` + first line. Implement: turn the chip `<span>` (FindingCard.tsx:74-79) into a `<button>` toggling a `SourcePreview`. Thread `runId` in. If `runId` is absent (chip rendered outside a run context), keep it a non-clickable span (graceful degradation).

- [ ] **Step 4: Verify build + tests**

`cd crates/rupu-cp/web && npx vitest run` (all green); `npx tsc --noEmit` (clean); `npm run build` (succeeds — the CP embeds the dist).

- [ ] **Step 5: Visual verification (required before merge)**

Cannot be unit-verified. Before merge, in a browser: open a run's `/transcript` with an ast_grep call, click a `file:line` header → confirm the source slice loads, is line-numbered, the target line is emphasized, and syntax highlighting renders; click a finding location chip → same. Confirm a remote-host run shows the graceful "not available" message. Report `DONE_WITH_CONCERNS` noting "visual check pending" if you cannot drive a browser — the controller routes it.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/components/transcript/ToolCard.tsx crates/rupu-cp/web/src/components/transcript/FindingCard.tsx crates/rupu-cp/web/src/
git commit -m "feat(cp): clickable path:line -> source preview in ast_grep + findings"
```

---

## Self-Review

**1. Spec coverage:** endpoint + path-safety + local/remote branch + size guard + window (Task 1); `api.readSource` + `SourcePreview` + grammar registration (Task 2); clickable wiring in both hook points + runId threading (Task 3); local-first with remote graceful-unavailable (Tasks 1+3); tests + build + visual gate (all). ✓
**2. Placeholder scan:** concrete schema, constants (`MAX_SOURCE_BYTES`, context clamp), and code/tests in each step; boilerplate deferred to named reference files to copy (transcript.rs, CodeHighlight.tsx) rather than invented — appropriate for an existing-codebase task. ✓
**3. Type consistency:** `SourceSlice` keys identical across backend serde (camelCase) and frontend TS type and `SourcePreview` consumption; `readSource(runId, path, line, opts)` signature matches its callers in Task 3. ✓
