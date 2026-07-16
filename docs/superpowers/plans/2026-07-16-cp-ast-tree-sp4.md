# CP AST visualization SP4 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A tree-sitter-backed CST viewer in the CP: for a matched code location, serve and render the real syntax tree (bounded subtree around the match), with the matched node highlighted.

**Architecture:** New `rupu-ast` crate wraps tree-sitter (parse + serialize to a bounded `AstNode` tree). A CP endpoint `GET /api/runs/:id/ast` reuses SP3's run-resolution + path-safety + local-first, calls `rupu_ast::parse_slice`, returns an `AstResponse`. The web app gains `api.readAst` + a recursive `AstTree` component, wired as a "syntax tree" toggle on ast_grep match rows.

**Tech Stack:** Rust + tree-sitter (core + rust/python/typescript/javascript/go/json grammar crates — NEW, approved); axum; React + TS (hand-rolled tree UI, no new npm dep).

**Spec:** `docs/superpowers/specs/2026-07-16-cp-ast-tree-sp4-design.md`.

## Global Constraints

- Rust 2021; do NOT run workspace-wide `cargo fmt` — `rustfmt --edition 2021` only touched files. `unsafe_code` forbidden in workspace crates — `rupu-ast`'s own code must be safe (the C FFI lives inside the grammar crates, which is fine). No new npm deps.
- **Workspace deps only:** tree-sitter core + grammar crate versions pinned in root `[workspace.dependencies]`; crate `Cargo.toml` uses `.workspace = true`. The versions MUST be mutually compatible (grammar crate's declared `tree-sitter` range must include the core version) — this is the primary risk; resolve it first.
- **Reuse, do not duplicate, SP3's security guard:** the `ast` endpoint uses SP3's `resolve_under_workspace` path-safety + run-resolution + `MAX_SOURCE_BYTES` size guard. Extract shared helpers if cleaner; never reimplement the traversal/symlink check.
- Local-first: remote (`Host`) runs → `{ available:false, reason:"…not available for remote-host runs yet." }`.
- Bounds: `MAX_AST_NODES = 2000` (truncate + flag); `CONTEXT_ANCESTORS = 3` (root the returned subtree this many named ancestors above the matched node).
- `AstNode` (camelCase): `{ kind, named, field?: string|null, startLine, startCol, endLine, endCol, matched, children:[] }`, 1-based line/col. `AstResponse`: `{ available, language?, root?, truncated?, reason? }`.

---

### Task 1: `rupu-ast` crate (tree-sitter wrapper) — DEPENDENCY-RISK GATE

**Files:**
- Create: `crates/rupu-ast/Cargo.toml`, `crates/rupu-ast/src/lib.rs`
- Modify: root `Cargo.toml` (`[workspace]` members + `[workspace.dependencies]` tree-sitter set)
- Test: `crates/rupu-ast/src/lib.rs` inline `#[cfg(test)]`

**Interfaces:**
- Produces: `rupu_ast::{Lang, AstNode, AstSubtree, parse_slice, AstError}`. Consumed by Task 2.

- [ ] **Step 1: Resolve a compatible dependency set FIRST (the risk)**

Before writing logic, get the deps compiling. Add `crates/rupu-ast` to the workspace `members`. In root `[workspace.dependencies]` add `tree-sitter` and the six grammar crates. Pick a **mutually compatible** set — the grammar crate's `tree-sitter` requirement must include the core version. Start from the latest tree-sitter core and the latest grammar crates that declare support for it; if `cargo build -p rupu-ast` fails with a `tree_sitter::Language` type-mismatch (two tree-sitter cores linked) or a version conflict, pin down/older until one core resolves for all. Note the exact chosen versions in the report.

Grammar language accessors differ by version: newer crates export a `LANGUAGE: LanguageFn` constant (use `tree_sitter::Language::from(tree_sitter_rust::LANGUAGE)`), older export `fn language() -> Language`. `tree-sitter-typescript` exposes two (`LANGUAGE_TYPESCRIPT`, `LANGUAGE_TSX`). Bind `Lang::grammar()` to whatever the resolved versions expose.

Verify the deps compile with a trivial parse before proceeding:
```rust
// scratch check inside Step 3's first test
let mut p = tree_sitter::Parser::new();
p.set_language(&Lang::Rust.grammar()).unwrap();
let tree = p.parse("fn main() {}", None).unwrap();
assert_eq!(tree.root_node().kind(), "source_file");
```
Run: `cargo build -p rupu-ast` — must be clean before Step 2.

- [ ] **Step 2: Write failing tests**

In `crates/rupu-ast/src/lib.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn parses_rust_and_marks_matched_node() {
    let src = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    // target the identifier `add` at line 1, col 4 (1-based)
    let sub = parse_slice(src, Lang::Rust, 1, 4).expect("parse");
    assert!(!sub.truncated);
    // somewhere in the tree there is exactly the matched node, named, kind identifier
    let matched = find_matched(&sub.root).expect("a matched node");
    assert!(matched.named);
    assert_eq!(matched.kind, "identifier");
    assert_eq!(matched.start_line, 1);
    // root is a named ancestor (context), not the identifier itself
    assert_ne!(sub.root.kind, "identifier");
}

#[test]
fn lang_from_path_maps_extensions() {
    assert_eq!(Lang::from_path(std::path::Path::new("a.rs")), Some(Lang::Rust));
    assert_eq!(Lang::from_path(std::path::Path::new("a.tsx")), Some(Lang::Tsx));
    assert_eq!(Lang::from_path(std::path::Path::new("a.unknown")), None);
}

#[test]
fn every_language_parses_a_trivial_snippet() {
    for (lang, src) in [
        (Lang::Rust, "fn a(){}"),
        (Lang::Python, "def a():\n    pass\n"),
        (Lang::TypeScript, "const a: number = 1;"),
        (Lang::Tsx, "const a = <div/>;"),
        (Lang::JavaScript, "const a = 1;"),
        (Lang::Go, "package main\nfunc a(){}"),
        (Lang::Json, "{\"a\":1}"),
    ] {
        assert!(parse_slice(src, lang, 1, 1).is_ok(), "{lang:?} failed to parse");
    }
}

// test helper
fn find_matched(n: &AstNode) -> Option<&AstNode> {
    if n.matched { return Some(n); }
    n.children.iter().find_map(find_matched)
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-ast`
Expected: FAIL to compile (types/functions absent).

- [ ] **Step 4: Implement `lib.rs`**

```rust
//! rupu-ast — tree-sitter CST wrapper: parse source and serialize a
//! bounded, JSON-friendly subtree around a target position.
#![forbid(unsafe_code)]

use serde::Serialize;

pub const MAX_AST_NODES: usize = 2000;
pub const CONTEXT_ANCESTORS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang { Rust, Python, TypeScript, Tsx, JavaScript, Go, Json }

impl Lang {
    pub fn from_path(p: &std::path::Path) -> Option<Lang> {
        match p.extension().and_then(|e| e.to_str())? {
            "rs" => Some(Lang::Rust),
            "py" => Some(Lang::Python),
            "ts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            "go" => Some(Lang::Go),
            "json" => Some(Lang::Json),
            _ => None,
        }
    }
    pub fn grammar(self) -> tree_sitter::Language {
        // Bind to the resolved grammar-crate accessors (LANGUAGE const or language()).
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AstNode {
    pub kind: String,
    pub named: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub matched: bool,
    pub children: Vec<AstNode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AstSubtree {
    pub language: String,      // lowercase name
    pub root: AstNode,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AstError {
    #[error("failed to set tree-sitter language")]
    Language,
    #[error("tree-sitter produced no tree")]
    NoTree,
}

/// Parse `source`, find the deepest NAMED node containing the 1-based
/// (line,col), root the returned subtree CONTEXT_ANCESTORS named
/// ancestors above it, serialize (1-based, matched node flagged),
/// capped at MAX_AST_NODES nodes.
pub fn parse_slice(source: &str, lang: Lang, line: u32, col: u32) -> Result<AstSubtree, AstError> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.grammar()).map_err(|_| AstError::Language)?;
    let tree = parser.parse(source, None).ok_or(AstError::NoTree)?;
    let root = tree.root_node();

    // tree-sitter Point is 0-based (row, column-in-bytes). Convert the
    // 1-based (line,col) target to a byte-ish point (col as char/byte is
    // approximate; use row + best-effort column).
    let target = tree_sitter::Point { row: line.saturating_sub(1) as usize, column: col.saturating_sub(1) as usize };

    // Descend to the smallest named node whose range contains `target`.
    let matched = deepest_named_at(root, target).unwrap_or(root);
    let matched_id = matched.id();

    // Walk up CONTEXT_ANCESTORS named ancestors for context.
    let mut ctx = matched;
    for _ in 0..CONTEXT_ANCESTORS {
        match ctx.parent() {
            Some(p) => ctx = p,
            None => break,
        }
    }

    let mut budget = MAX_AST_NODES;
    let mut truncated = false;
    let node = serialize(ctx, None, matched_id, &mut budget, &mut truncated);
    Ok(AstSubtree { language: lang_name(lang).to_string(), root: node, truncated })
}

fn deepest_named_at(node: tree_sitter::Node, pt: tree_sitter::Point) -> Option<tree_sitter::Node> {
    // named_descendant_for_point_range is the direct tree-sitter API for this.
    node.named_descendant_for_point_range(pt, pt)
}

fn serialize(
    node: tree_sitter::Node,
    field: Option<String>,
    matched_id: usize,
    budget: &mut usize,
    truncated: &mut bool,
) -> AstNode {
    let start = node.start_position();
    let end = node.end_position();
    let mut children = Vec::new();
    let mut cursor = node.walk();
    if node.child_count() > 0 && *budget > 0 {
        for (i, child) in node.children(&mut cursor).enumerate() {
            if *budget == 0 { *truncated = true; break; }
            *budget -= 1;
            let fname = node.field_name_for_child(i as u32).map(|s| s.to_string());
            children.push(serialize(child, fname, matched_id, budget, truncated));
        }
    }
    AstNode {
        kind: node.kind().to_string(),
        named: node.is_named(),
        field,
        start_line: start.row as u32 + 1,
        start_col: start.column as u32 + 1,
        end_line: end.row as u32 + 1,
        end_col: end.column as u32 + 1,
        matched: node.id() == matched_id,
        children,
    }
}

fn lang_name(l: Lang) -> &'static str {
    match l { Lang::Rust=>"rust", Lang::Python=>"python", Lang::TypeScript=>"typescript", Lang::Tsx=>"tsx", Lang::JavaScript=>"javascript", Lang::Go=>"go", Lang::Json=>"json" }
}
```

(Adapt `grammar()` accessors + `set_language`/`field_name_for_child` signatures to the resolved tree-sitter version — some versions use `set_language(lang)` by value, or `field_name_for_child` returns `Option<&str>` vs the cursor's `field_name()`. The ALGORITHM is fixed; bind the API.)

`Cargo.toml` for the crate: `serde` (workspace, derive), `thiserror` (workspace), `tree-sitter` + the six grammar crates (workspace). `[lints] workspace = true`.

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p rupu-ast`
Expected: PASS (adjust `col` in the first test if the char/byte column makes `add` resolve to an adjacent node — target a column squarely inside the identifier).

- [ ] **Step 6: Lint, format, commit**

`cargo clippy -p rupu-ast` clean; `rustfmt --edition 2021` the crate files + root Cargo.toml is TOML (leave formatting minimal, only your added lines).

```bash
git add crates/rupu-ast Cargo.toml Cargo.lock
git commit -m "feat(ast): rupu-ast crate — tree-sitter CST parse + bounded subtree"
```

---

### Task 2: Backend `GET /api/runs/:id/ast` endpoint

**Files:**
- Modify/Create: `crates/rupu-cp/src/api/source.rs` (add handler + route) OR new `crates/rupu-cp/src/api/ast.rs`; `api/mod.rs`; `server.rs`
- Modify: `crates/rupu-cp/Cargo.toml` (add `rupu-ast = { path = "../rupu-ast" }`)
- Test: alongside the SP3 endpoint tests

**Interfaces:**
- Consumes: `rupu_ast::parse_slice` (Task 1); SP3's `resolve_under_workspace`, run-resolution, `MAX_SOURCE_BYTES`.
- Produces: `GET /api/runs/:id/ast?path=&line=&col=&host=` → `Json<AstResponse>`.

- [ ] **Step 1: Read SP3's `source.rs`** to reuse `resolve_under_workspace`, the run-resolution/local-remote branch, and the size guard. If those are private, make them `pub(crate)` or `pub(super)` so the new handler can call them (do NOT copy the security guard).

- [ ] **Step 2: Write failing endpoint tests** (mirror SP3's harness): a rust file in a tempdir workspace → `GET /api/runs/:id/ast?path=x.rs&line=1&col=4` returns `available:true`, `language:"rust"`, `root.kind` non-empty, and some node has `matched:true`; `path=../escape` → 400; unsupported extension (`x.unknownext`) → `available:false`; remote host → `available:false`.

- [ ] **Step 3: Implement** `AstResponse { available: bool, #[serde(flatten over AstSubtree fields or) ] language?, root?, truncated?, reason? }` (define as a struct with optional fields; on success fill from `AstSubtree`). Handler flow per spec §Backend: resolve run → local/remote → path guard → size guard → `Lang::from_path` (None → unavailable) → read → `parse_slice` (Err → `available:false, reason:"Could not parse file."`) → success response. Register `rupu-ast` dep + route in `mod.rs`/`server.rs`.

- [ ] **Step 4: Verify** `cargo test -p rupu-cp` (SP3 + new tests green); `cargo build -p rupu-cp` clean; clippy clean on touched files; rustfmt touched files.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cp Cargo.toml Cargo.lock
git commit -m "feat(cp): GET /api/runs/:id/ast — tree-sitter CST endpoint"
```

---

### Task 3: Web `api.readAst` + `AstTree` component

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`readAst` + `AstNode`/`AstResponse` types)
- Create: `crates/rupu-cp/web/src/components/transcript/AstTree.tsx`
- Test: `crates/rupu-cp/web/src/components/transcript/AstTree.test.tsx`

**Interfaces:**
- Consumes: `GET /api/runs/:id/ast` (Task 2).
- Produces: `api.readAst(runId, path, line, col, opts?)`, `<AstTree runId path line col host? />`. Consumed by Task 4.

- [ ] **Step 1: Read** `api.ts` `readSource` and `SourcePreview.tsx` (Task-2/SP3 patterns to mirror: lazy fetch, states).

- [ ] **Step 2: Add `readAst` + types + URL test.** `AstNode` TS mirrors the Rust `AstNode` (camelCase, `children: AstNode[]`, `field?: string`). `AstResponse`: `{ available; language?; root?: AstNode; truncated?; reason? }`. `readAst(runId, path, line, col, opts?: {host?})` → `/api/runs/:id/ast?path=&line=&col=[&host=]`.

- [ ] **Step 3: Implement `AstTree` (TDD).** Lazy-fetch on mount (like `SourcePreview`); loading/error/`available:false`(reason)/tree states. Recursive `<TreeNode node depth />`: a row with an expand/collapse chevron when `children.length`, the `kind` (mono), a dimmed `field:` prefix when `node.field`, and the range; a `named:false` node rendered dimmed (and hidden by default behind a "show anonymous" checkbox at the tree root — default named-only). The `matched` node highlighted (amber bg) and its ancestor chain auto-expanded so it's visible on open. `truncated` → a small "tree truncated (large file)" note. Tests (mock `api.readAst`): renders a mocked tree named-only; expand reveals children; matched node highlighted; toggling "show anonymous" reveals unnamed nodes; `available:false` shows reason.

- [ ] **Step 4: Verify** `npx vitest run AstTree` + api test green; `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/components/transcript/AstTree.tsx crates/rupu-cp/web/src/components/transcript/AstTree.test.tsx
git commit -m "feat(cp): readAst API + AstTree CST viewer component"
```

---

### Task 4: Wire the "syntax tree" toggle into ast_grep match rows

**Files:**
- Modify: `crates/rupu-cp/web/src/components/transcript/ToolCard.tsx` (`AstGrepMatchRow`/`AstGrepTextMatchRow` — add a second toggle for the tree)
- Test: extend `ToolCard.test.tsx`

**Interfaces:**
- Consumes: `<AstTree />` (Task 3), the SP3 match-row structure (already has a `runId` prop + a source-preview toggle).

- [ ] **Step 1: Read** the current `AstGrepMatchRow` (from SP3) to see how the source-preview toggle is structured, and add a sibling.

- [ ] **Step 2: Add the toggle (TDD).** Beside the existing source-preview button, add a "tree" button (only when `runId` present) that toggles an inline `<AstTree runId path={m.file} line={m.range.startLine} col={m.range.startCol} host={host} />` below the row (independent `useState` from the source-preview toggle, so both can be open). Graceful non-clickable when `runId` absent. Test (mock `api.readAst`): clicking the tree button mounts `AstTree` / calls `api.readAst` with file+line+col; both toggles independent.

- [ ] **Step 3: Verify** `npx vitest run` (ALL green), `npx tsc --noEmit`, `npm run build` (succeeds).

- [ ] **Step 4: Visual verification (required before merge).** Cannot be unit-verified. Before merge, in a browser: click the "tree" button on an ast_grep match → the CST subtree renders, the matched node is highlighted, expand/collapse works, "show anonymous" toggles unnamed nodes, a large file shows the truncated note, an unsupported file type / remote run shows the graceful message. Report `DONE_WITH_CONCERNS` noting "visual check pending" if you cannot drive a browser.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cp/web/src/components/transcript/
git commit -m "feat(cp): syntax-tree toggle on ast_grep matches"
```

---

## Self-Review

**1. Spec coverage:** `rupu-ast` crate (Lang/AstNode/parse_slice, bounding, deps) → Task 1; `GET /api/runs/:id/ast` reusing SP3 guard → Task 2; `api.readAst` + `AstTree` → Task 3; wiring toggle → Task 4; tests + build + visual gate throughout. ✓
**2. Placeholder scan:** concrete struct/algorithm/tests; the one deliberately-deferred detail is the exact tree-sitter API binding (version-dependent) — the algorithm is fully specified and the version-resolution is the first explicit step. No TBD. ✓
**3. Type consistency:** `AstNode`/`AstResponse` keys identical across `rupu-ast` (Rust serde camelCase), the CP endpoint, and the TS types + `AstTree`; `parse_slice(source, lang, line, col)` signature consistent between Task 1 def and Task 2 call; `readAst(runId,path,line,col,opts)` consistent Task 3↔4. ✓
