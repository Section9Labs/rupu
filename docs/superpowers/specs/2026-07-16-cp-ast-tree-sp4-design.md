# CP AST visualization — SP4 (tree-sitter CST viewer) — design

**Date:** 2026-07-16
**Status:** approved-to-build (user chose the full tree-sitter approach)
**Part of:** "CP rich tool-call rendering" initiative, final sub-project. Depends on SP2 (ast_grep renderer / match ranges) and SP3 (source endpoint infra: run resolution + path safety + local-first). Stacked on `cp-source-preview`.

## Motivation

Give the CP a real, playground-style syntax-tree view: for a matched code location, render the actual tree-sitter CST (every node, field names, ranges), with the matched node highlighted. This is the "proper AST visualization" — beyond SP2's metavariable bindings table. The user explicitly chose the full tree-sitter path over lighter alternatives.

## New crate: `rupu-ast` (reusable parser)

A small, dependency-isolated crate wrapping tree-sitter so the capability is reusable (CLI, tools, CP) rather than CP-locked — honoring the "in-project file parser we can use" intent and the hexagonal discipline.

- **`Lang`** enum: `Rust, Python, TypeScript, Tsx, JavaScript, Go, Json`. `Lang::from_path(path) -> Option<Lang>` (by extension: `rs→Rust, py→Python, ts→TypeScript, tsx→Tsx, js/jsx/mjs/cjs→JavaScript, go→Go, json→Json`). `Lang::grammar() -> tree_sitter::Language`.
- **`AstNode`** (serializable, camelCase): `{ kind: String, named: bool, field: Option<String>, startLine, startCol, endLine, endCol (1-based, u32), matched: bool, children: Vec<AstNode> }`.
- **`parse_slice(source: &str, lang: Lang, target: Range) -> Result<AstSubtree, AstError>`** where `Range = { line, col }` (1-based) or a start/end pair:
  1. Parse `source` with tree-sitter.
  2. Find the deepest **named** node whose range contains `target` — the "matched" node.
  3. Root the returned subtree at an ancestor for context: walk up `CONTEXT_ANCESTORS = 3` named ancestors (or to the root if fewer). Return that subtree.
  4. Serialize to `AstNode`, marking the matched node `matched: true`, converting 0-based tree-sitter points to 1-based.
  5. **Bound size:** cap at `MAX_AST_NODES = 2000` nodes (depth-first; when the cap is hit, stop descending and set a top-level `truncated` flag). Prevents huge CST payloads.
- **`AstSubtree`**: `{ language: Lang, root: AstNode, truncated: bool, totalMatchedNodeDepth?: u32 }`.
- No `unsafe` in `rupu-ast` itself (tree-sitter FFI is inside the grammar crates, which are the only exception — the crate's own code stays safe).

### Dependencies (the crux — version compat)

Add to root `[workspace.dependencies]` a **mutually compatible** set (tree-sitter core + grammar crates whose declared `tree-sitter` range includes the chosen core). The implementer MUST verify they build together (grammar crates compile C, and version mismatch is the classic failure) — iterate versions until `cargo build -p rupu-ast` is clean:
- `tree-sitter` (core)
- `tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-typescript` (exposes `LANGUAGE_TYPESCRIPT` + `LANGUAGE_TSX`), `tree-sitter-javascript`, `tree-sitter-go`, `tree-sitter-json`
Pin exact versions in the root workspace table; `rupu-ast/Cargo.toml` references them via `.workspace = true`. This is a real, C-compiling dependency addition (build-time cost) — explicitly approved.

## Backend endpoint: `GET /api/runs/:id/ast`

New handler (extend `crates/rupu-cp/src/api/source.rs` or a sibling `ast.rs` module; register in `mod.rs` + `server.rs`).

- **Route:** `GET /api/runs/:id/ast?path=<rel>&line=<1-based>&col=<1-based>&host=`.
- **Flow (mirrors SP3's `get_source`):** resolve run → local/remote branch (remote → `{ available:false, reason:"…not available for remote-host runs yet." }`) → `resolve_under_workspace` path guard (reuse SP3's helper — traversal/symlink safe) → size guard (`MAX_SOURCE_BYTES`) → `Lang::from_path`; if `None` → `{ available:false, reason:"No syntax grammar for this file type." }` → read file → `rupu_ast::parse_slice(source, lang, {line,col})` → return `{ available:true, language, root, truncated }`. Unknown run → 404. Parse error → `{ available:false, reason:"Could not parse file." }`.
- **Response `AstResponse`:** `{ available: bool, language?: string, root?: AstNode, truncated?: bool, reason?: string }` (camelCase; `AstNode` as above).
- Reuse SP3's `resolve_under_workspace`, run-resolution, and size-guard code (extract shared helpers if cleaner, or call across modules) — do NOT duplicate the security guard.

## Frontend

- **`api.readAst(runId, path, line, col, opts?: { host? }): Promise<AstResponse>`** + `AstNode`/`AstResponse` TS types (`api.ts`, beside `readSource`).
- **`AstTree` component** (`components/transcript/AstTree.tsx`): recursive, collapsible tree. Each node row: expand/collapse chevron (for nodes with children), the `kind` (monospace), the `field:` prefix when present (dimmed), and the range; **anonymous nodes** (`named:false`, e.g. punctuation/keywords) rendered dimmer/smaller or hidden behind a "show anonymous" toggle (default: named-only for readability). The `matched:true` node is highlighted and auto-expanded/scrolled into view. `truncated` → a "tree truncated (large file)" note. Lazy-fetches on open (like `SourcePreview`).
- **Wiring:** add a second toggle to each ast_grep match row (next to SP3's source-preview toggle) — a "syntax tree" button that opens `<AstTree runId path line col />` for that match (using the match's `range.startLine`/`startCol`). Graceful non-clickable when `runId` absent (same rule as SP3). Optionally also on `FindingCard` (stretch; primary target is the ast_grep match rows).

## Testing

- **`rupu-ast` (unit):** parse a small rust snippet → assert the CST has expected kinds (`source_file` → `function_item` → `identifier`/`parameters`); `matched` marks the deepest named node at a target position; 1-based conversion; `MAX_AST_NODES` truncation sets the flag on a large input; `Lang::from_path` extension mapping; each supported language parses a trivial snippet without panic.
- **Backend:** endpoint returns a tree for a rust file at a line; unsupported extension → `available:false`; path traversal → 400 (reuses SP3 guard, add one assertion); remote → unavailable; unknown run → 404.
- **Frontend:** `api.readAst` URL; `AstTree` renders a mocked tree (named-only default, expand shows children, matched node highlighted, truncated note); toggle mounts it lazily. Build (`npm run build`) + `tsc` clean. Visual check flagged.

## Out of scope / follow-ups

- Whole-file tree (we return a bounded subtree around the match — the playground-style focus). A "view full file tree" expansion is a follow-up.
- Editing / interactive pattern testing against the tree (playground's write side).
- Remote-host parsing — same deferral as SP3 (needs `HostConnector` file read).

## Delivery

- Branch `cp-ast-tree`, stacked on `cp-source-preview`. Rebase base to `main` as the stack lands.
- Order: `rupu-ast` crate + deps (Task 1, the dependency-risk task) → backend endpoint (Task 2) → frontend `api.readAst` + `AstTree` (Task 3) → wiring into ast_grep rows (Task 4).
- Adds tree-sitter + grammar crates (approved). No new npm deps (the tree UI is hand-rolled React). CP embeds `web/dist` (release needs `make cp-web`).
