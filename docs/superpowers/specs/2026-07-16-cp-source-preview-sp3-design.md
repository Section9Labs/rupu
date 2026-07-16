# CP source preview — SP3 (in-project file read + clickable path:line) — design

**Date:** 2026-07-16
**Status:** approved-to-build (user directed SP3 then SP4)
**Part of:** "CP rich tool-call rendering" initiative. SP3 = source-file access. Depends on SP2 (the ast_grep renderer whose `file:line` headers become clickable). SP4 (AST visualization) builds on this.

## Motivation

The CP renders `path:line` references (ast_grep match headers from SP2, `FindingCard` location chips) but they are dead text — the CP has no source-file access. This sub-project adds a workspace-scoped backend endpoint that serves a slice of a source file around a line, and a `SourcePreview` UI that turns those references into clickable, syntax-highlighted previews.

## Scope decision: local-first

Run workspaces can live on the CP's own machine (`RunLocation::Global` / `ProjectLocal`) or on a remote host (`RunLocation::Host`). `HostConnector` has **no file-read method today**. SP3 ships **local runs fully** and returns a graceful "source preview not available for remote runs yet" for `Host` runs — matching the existing remote-transcript-gap deferral. Adding a `HostConnector::read_source` (or proxying to a remote source endpoint) is an explicit follow-up (tracked in TODO.md). This keeps SP3 a clean, shippable slice without solving remote file transport.

## Backend — `GET /api/runs/:id/source`

New module `crates/rupu-cp/src/api/source.rs`, registered in `api/mod.rs` (`pub mod source;`) and `server.rs` (`.merge(crate::api::source::routes())`).

- **Route:** `GET /api/runs/:id/source?path=<workspace-relative>&line=<1-based>&context=<n>` (also accept `&host=` for parity, but a non-`local` host short-circuits to the remote-unavailable response).
- **Handler flow:**
  1. Load `RunRecord` via `state.run_store.load(id)` (or the `RunLocation`-resolved store for `ProjectLocal`, mirroring `runs.rs:958-981`). 404 if the run is unknown.
  2. `resolve_run_location(&state, id)`: `Global` / `ProjectLocal { path }` → local read; `Host { .. }` (or `host` query != `local`) → return `{ available: false, reason: "Source preview is not available for remote-host runs yet." }` (HTTP 200 with an unavailable flag, so the UI degrades gracefully rather than erroring); `NotFound`/`Unpersisted` → 404.
  3. **Path safety:** resolve `path` under `record.workspace_path` using the `validate_transcript_path` approach (`transcript.rs:39`) — reject `..` components, canonicalize the deepest existing ancestor to defeat symlink escape, require `resolved.starts_with(canonicalize(workspace_path))`. (Drop the `.jsonl` extension requirement.) On failure → 400.
  4. Read the file. **Guard size:** refuse files larger than `MAX_SOURCE_BYTES = 2 MiB` with an `{ available: false, reason: "File too large to preview" }`. Read as UTF-8 lossy.
  5. Extract a window: `context` (default 20, clamped to `[0, 200]`) lines on each side of `line`, clamped to file bounds. 1-based line numbers.
- **Response JSON (`SourceSlice`):**
  ```jsonc
  {
    "available": true,
    "path": "crates/rupu-tools/src/ast_grep.rs",  // echo, workspace-relative
    "language": "rust",            // from extension; null if unknown
    "startLine": 26,               // 1-based, first line in `lines`
    "endLine": 66,
    "targetLine": 46,              // the requested line (clamped)
    "totalLines": 219,
    "lines": [ { "n": 26, "text": "..." }, ... ]
  }
  ```
  Unavailable case: `{ "available": false, "reason": "<message>" }`.
- **Tests** (`crates/rupu-cp/tests` or inline): path-traversal (`../../etc/passwd`) rejected (400); a symlink escaping the workspace rejected; window extraction correct near start/end of file and mid-file; unknown run → 404; oversize file → `available:false`; language detection for a couple extensions. Remote-run path is covered by a unit test on the location branch if feasible, else noted.

## Frontend — `api.readSource` + `SourcePreview`

- **API client** (`crates/rupu-cp/web/src/lib/api.ts`): add
  `readSource(runId: string, path: string, line: number, opts?: { context?: number; host?: string }): Promise<SourceSlice>` next to `getTranscript`, plus the `SourceSlice` TS type.
- **`SourcePreview` component** (`crates/rupu-cp/web/src/components/transcript/SourcePreview.tsx`):
  - Given `{ runId, path, line }`, lazy-fetches the slice on first open (collapsed by default), shows a small spinner/error state, and renders a line-numbered code block with the `targetLine` emphasized (highlighted background) and syntax highlighting via the existing highlight.js path (`CodeHighlight.tsx`) — **register the additional grammars** needed (rust, python, typescript, javascript, go, json; extend `CodeHighlight`'s registration). Unknown language → plain mono.
  - `available: false` → render the `reason` text (e.g. "not available for remote-host runs yet"), not an error.
- **Clickable wiring (two hook points):**
  1. **ast_grep renderer** (`ToolCard.tsx` `AstGrepBody`, from SP2): the per-match `file:line:col` header becomes a button that toggles an inline `<SourcePreview runId path line />` beneath the match. `runId` must be threaded into `ToolCard`/`AstGrepBody` — trace how the transcript view knows its run id (the `RunTranscript` page has it; thread it down as a prop or context).
  2. **`FindingCard`** (`FindingCard.tsx:74-79`): the location chip becomes clickable, opening the same `SourcePreview`. `runId` threaded in similarly.
- **Tests:** `api.readSource` builds the right URL; `SourcePreview` renders slice / loading / unavailable states; clicking a `file:line` header toggles the preview (react-testing-library, mocking `api.readSource`). Build clean (`npm run build`, `tsc --noEmit`). Visual check flagged (as SP2).

## The "in-project file parser"

SP3's "parser" is a **source reader + slicer + language-detector**, not a syntax-tree parser — that (tree-sitter) is SP4. Highlighting is client-side via highlight.js (already a CP dependency). No new crate dependency; possibly new highlight.js language registrations (already bundled with the `highlight.js` package — no new npm dep).

## Out of scope (tracked in TODO.md)

- Remote-host source reads (`HostConnector::read_source`) — deferred.
- Full-file view / open-in-editor — SP3 serves windows only.
- AST/tree rendering — SP4.

## Delivery

- Branch `cp-source-preview`, stacked on `cp-rich-tool-rendering` (PR base = that branch, so SP3's PR shows only SP3's diff). Rebase base to `main` if #479 merges first.
- Backend tasks (Rust) land before frontend tasks (the endpoint the UI calls).
- No new crate deps; no new npm deps. CP embeds `web/dist` at build time (release needs `make cp-web`).
