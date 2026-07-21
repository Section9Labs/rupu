# Findings on Code — Project Code Tab, Viewer with Inline Findings, File Navigator — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show rupu security findings *on the actual source code* — the vulnerable line highlighted, the finding as an inline GitHub-PR-style comment — inside a new project "Code" tab with a lazy file navigator, plus "View on GitHub/GitLab" deep-links.

**Architecture:** Two new read-only, workspace-scoped rupu-cp endpoints (`GET /api/projects/:ws_id/tree` and `.../source`) that resolve every path through the existing `resolve_under_workspace` safety primitive. A shared React `CodeViewer` renders a whole file with theme-aware syntax highlighting, per-finding severity line-bands, and expandable inline finding cards; a `FileTree` navigator lazily lists directories with finding badges. Drift (code changed since a finding was made) is detected client-side by comparing the finding's stored `code_excerpt`. A pure Rust permalink builder in `rupu-scm` turns the git remote + branch + file + line range into a github/gitlab web URL.

**Tech Stack:** Rust (axum, serde, `rupu-cp`/`rupu-scm`/`rupu-workspace`/`rupu-coverage` crates); React + Vite + TypeScript, react-router-dom v6, Tailwind (CSS-custom-prop theming), highlight.js, vitest + @testing-library/react.

## Global Constraints

- **rupu-cp stays READ-ONLY.** No endpoint writes to the workspace. New handlers only list directories and read files.
- **Path safety is load-bearing.** Every project file read/list MUST go through `crate::api::source::resolve_under_workspace(workspace, rel)` (rejects absolute paths, any `..` component, and symlink escapes via canonicalize + `starts_with`). Never touch a caller-supplied path any other way.
- **Workspace deps only** — versions pinned in root `Cargo.toml`; never add a version to a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden (rupu-scm and rupu-cp are NOT exempt).
- Errors: `thiserror` in libraries; handlers return `crate::error::ApiResult<Json<T>>` where `ApiResult<T> = Result<T, crate::error::ApiError>`. `ApiError` constructors: `bad_request`, `not_found`, `internal`.
- **No new npm dependencies.** No new Rust dependencies.
- **No color literals in the web UI** — colors come from `--c-*` CSS tokens / Tailwind classes keyed off them (`text-sev-high`, `bg-panel`, `border-border`, etc.). **Both light and dark themes must read correctly.**
- **Runtime validation before merge:** per CLAUDE.md, `cargo build`/`cargo test` cleanliness ≠ rendering cleanliness. The operator (matt) browser-validates the viewer + navigator in **both** light and dark before any merge. Subagents cannot validate GPUI/DOM rendering.
- **Toolchain note:** this worktree runs Homebrew Rust 1.95 while the repo pins 1.88; `rupu-cli` may show pre-existing red unrelated to this work. `rupu-cp`, `rupu-scm`, `rupu-workspace`, `rupu-coverage` are the crates this plan touches — keep those green. Never run workspace-wide `cargo fmt`; format only changed files with `rustfmt --edition 2021 <file>`.
- **Commit style:** every commit message ends with a trailing line `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Work on branch `findings-code-viewer`. No `gpg` signing.

---

## File Structure

**Backend (Rust)**
- Create `crates/rupu-cp/src/api/code.rs` — the two new workspace-scoped endpoints (`tree`, `source`) + their DTOs + `routes()`. One module, one responsibility (project-scoped file access for the Code tab).
- Modify `crates/rupu-cp/src/server.rs` — merge `crate::api::code::routes()`.
- Modify `crates/rupu-cp/src/api/mod.rs` — declare `pub mod code;`.
- Create `crates/rupu-scm/src/weburl.rs` — pure git-remote-URL parser + blob/home permalink builder.
- Modify `crates/rupu-scm/src/lib.rs` — declare `pub mod weburl;` and re-export.
- Modify `crates/rupu-cp/src/api/findings.rs` — add `permalink: Option<String>` to `FindingOut`, computed via the `rupu-scm` builder.
- Modify `crates/rupu-cp/src/api/projects.rs` — add `repo_home_url: Option<String>` to `ProjectDetail`.

**Frontend (TypeScript/React)**
- Modify `crates/rupu-cp/web/src/lib/api.ts` — add `getProjectTree` / `getProjectSource` calls + `TreeResult`/`TreeEntry`/`FileContent` types; extend `FindingRecord` with `permalink?` and `ProjectDetail` with `repo_home_url?`.
- Modify `crates/rupu-cp/web/src/components/CodeHighlight.tsx` — theme-aware (light+dark) highlight.js styling.
- Create `crates/rupu-cp/web/src/components/code/drift.ts` — pure `isFindingStale()` function.
- Create `crates/rupu-cp/web/src/components/code/InlineFindingCard.tsx` — the PR-style collapsible finding comment.
- Create `crates/rupu-cp/web/src/components/code/CodeViewer.tsx` — whole-file viewer with severity bands + inline cards.
- Create `crates/rupu-cp/web/src/components/code/FileTree.tsx` — lazy navigator with finding badges.
- Create `crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx` — two-pane shell (tree + viewer).
- Modify `crates/rupu-cp/web/src/pages/ProjectDetail.tsx` — add `'code'` tab.
- Modify `crates/rupu-cp/web/src/App.tsx` — add `/projects/:wsId/code` route (before the `:wsId` wildcard).
- Modify `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` — deep-link `file:line` into the Code tab.

Co-located `*.test.ts(x)` files accompany each new/changed unit.

---

## Phase 1 — Foundational endpoints + the CodeViewer

### Task 1: Project file-tree endpoint — `GET /api/projects/:ws_id/tree`

**Files:**
- Create: `crates/rupu-cp/src/api/code.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs` (add `pub mod code;`)
- Modify: `crates/rupu-cp/src/server.rs` (merge `crate::api::code::routes()`)
- Test: inline `#[cfg(test)] mod tests` in `crates/rupu-cp/src/api/code.rs`

**Interfaces:**
- Consumes (existing, verbatim): `crate::api::source::resolve_under_workspace(workspace: &std::path::Path, rel: &str) -> Result<PathBuf, ApiError>`; `crate::state::AppState` (fields used: `global_dir: PathBuf`); `crate::error::{ApiError, ApiResult}` (`ApiError::{bad_request,not_found,internal}`); `rupu_workspace::store::WorkspaceStore { root: PathBuf }` with `fn load(&self, id: &str) -> Result<Option<rupu_workspace::Workspace>, _>`; `rupu_workspace::Workspace { path: String, .. }`.
- Produces (later tasks / frontend rely on these EXACT shapes): `GET /api/projects/:ws_id/tree?path=<rel>` → JSON `TreeResult { path: String, parent: Option<String>, entries: Vec<TreeEntry> }`, `TreeEntry { name: String, path: String, kind: String /* "dir" | "file" */ }`. Both serialize field-name-as-is (no rename). `pub fn routes() -> axum::Router<AppState>`. Helper `pub(crate) fn load_workspace(s: &AppState, ws_id: &str) -> Result<rupu_workspace::Workspace, ApiError>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cp/src/api/code.rs` with only the test module first (the functions it calls come in Step 3). Add at top of file:

```rust
//! Project-scoped, read-only file access for the CP "Code" tab: a lazy
//! per-directory tree and whole-file source. Every caller-supplied path is
//! resolved through `crate::api::source::resolve_under_workspace`, the same
//! containment primitive the run-scoped source endpoint uses — no path here
//! ever escapes the workspace root.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_ws() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(d.path().join("README.md"), "# hi\n").unwrap();
        fs::create_dir_all(d.path().join(".git")).unwrap();
        fs::write(d.path().join(".git/HEAD"), "ref: x\n").unwrap();
        d
    }

    #[test]
    fn lists_root_dirs_first_then_files_and_hides_git() {
        let d = tmp_ws();
        let res = list_tree(d.path(), "").unwrap();
        let names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        // dirs before files, alphabetical within group, .git hidden
        assert_eq!(names, vec!["src", "README.md"]);
        assert_eq!(res.entries[0].kind, "dir");
        assert_eq!(res.entries[0].path, "src");
        assert_eq!(res.entries[1].kind, "file");
        assert_eq!(res.entries[1].path, "README.md");
        assert_eq!(res.parent, None);
    }

    #[test]
    fn lists_subdir_and_sets_parent() {
        let d = tmp_ws();
        let res = list_tree(d.path(), "src").unwrap();
        assert_eq!(res.path, "src");
        assert_eq!(res.parent.as_deref(), Some(""));
        let names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["main.rs"]);
        assert_eq!(res.entries[0].path, "src/main.rs");
    }

    #[test]
    fn refuses_parent_dir_escape() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "../etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn refuses_absolute_path() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "/etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn errors_when_path_is_a_file_not_dir() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "README.md").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp --lib api::code::tests -- --nocapture`
Expected: FAIL — `cannot find function list_tree in this scope` (and `TreeResult`/`TreeEntry` unresolved).

- [ ] **Step 3: Write the endpoint + the `list_tree` core**

Add above the test module in `crates/rupu-cp/src/api/code.rs`:

```rust
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::Path as FsPath;

/// A directory noise-list hidden from the tree. Dotfiles are otherwise shown.
const HIDDEN_DIRS: &[&str] = &[".git", ".rupu", "node_modules", "target"];

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeEntry {
    pub name: String,
    /// Workspace-relative POSIX-style path (matches `FindingRecord.file_path`).
    pub path: String,
    /// `"dir"` or `"file"`.
    pub kind: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeResult {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub path: String,
}

/// Resolve a `ws_id` to its `Workspace`, or a 404. Shared by both handlers.
pub(crate) fn load_workspace(
    s: &AppState,
    ws_id: &str,
) -> Result<rupu_workspace::Workspace, ApiError> {
    let store = rupu_workspace::store::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    match store.load(ws_id) {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(ApiError::not_found(format!("project {ws_id} not found"))),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

/// List the immediate children of a workspace-relative directory.
/// `rel == ""` means the workspace root. Dirs first, then files, each group
/// sorted by name; entries in `HIDDEN_DIRS` are omitted.
fn list_tree(workspace: &FsPath, rel: &str) -> Result<TreeResult, ApiError> {
    let dir = crate::api::source::resolve_under_workspace(workspace, rel)?;
    if !dir.is_dir() {
        return Err(ApiError::bad_request("path is not a directory"));
    }
    let mut dirs: Vec<TreeEntry> = Vec::new();
    let mut files: Vec<TreeEntry> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| ApiError::internal(e.to_string()))? {
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && HIDDEN_DIRS.contains(&name.as_str()) {
            continue;
        }
        // Build the workspace-relative child path with forward slashes.
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel.trim_end_matches('/'), name)
        };
        let te = TreeEntry {
            name,
            path: child,
            kind: if is_dir { "dir".into() } else { "file".into() },
        };
        if is_dir {
            dirs.push(te);
        } else {
            files.push(te);
        }
    }
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.extend(files);

    let parent = if rel.is_empty() {
        None
    } else {
        Some(
            rel.trim_end_matches('/')
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default(),
        )
    };
    Ok(TreeResult {
        path: rel.to_string(),
        parent,
        entries: dirs,
    })
}

async fn get_tree(
    Path(ws_id): Path<String>,
    Query(q): Query<TreeQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<TreeResult>> {
    let w = load_workspace(&s, &ws_id)?;
    let res = list_tree(FsPath::new(&w.path), &q.path)?;
    Ok(Json(res))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/projects/:ws_id/tree", get(get_tree))
}
```

Then declare the module and wire the route.

In `crates/rupu-cp/src/api/mod.rs`, add alongside the other `pub mod` lines:

```rust
pub mod code;
```

In `crates/rupu-cp/src/server.rs`, inside the `let api = Router::new()` chain (next to `.merge(crate::api::source::routes())`), add:

```rust
        .merge(crate::api::code::routes())
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp --lib api::code::tests -- --nocapture`
Expected: PASS (5 tests). If `tempfile` is not already a dev-dependency of rupu-cp, it is — confirm with `cargo test -p rupu-cp` building; the run-scoped `source.rs` tests already use it.

- [ ] **Step 5: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/code.rs
git add crates/rupu-cp/src/api/code.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs
git commit -m "feat(cp): project-scoped file-tree endpoint for the Code tab

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Project whole-file source endpoint — `GET /api/projects/:ws_id/source`

**Files:**
- Modify: `crates/rupu-cp/src/api/code.rs` (add `FileContent`, `read_whole_file`, `get_source`, extend `routes()`)
- Test: inline `#[cfg(test)] mod tests` in `crates/rupu-cp/src/api/code.rs`

**Interfaces:**
- Consumes: `crate::api::source::{resolve_under_workspace, detect_language, SourceLine}` — `SourceLine { pub n: usize, pub text: String }` (serializes as `n`/`text`, no rename); `detect_language(&FsPath) -> Option<&'static str>`.
- Produces (frontend relies on this EXACT shape): `GET /api/projects/:ws_id/source?path=<rel>` → JSON `FileContent` with `#[serde(rename_all = "camelCase")]` → `{ available, path?, language?, totalLines?, lines?: [{n,text}], reason? }`. Size cap constant `MAX_FILE_BYTES: u64 = 2 * 1024 * 1024`.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `mod tests` in `code.rs`:

```rust
    #[test]
    fn reads_whole_file_with_language_and_line_numbers() {
        let d = tmp_ws();
        let fc = read_whole_file(d.path(), "src/main.rs").unwrap();
        assert!(fc.available);
        assert_eq!(fc.path.as_deref(), Some("src/main.rs"));
        assert_eq!(fc.language, Some("rust"));
        assert_eq!(fc.total_lines, Some(1));
        let lines = fc.lines.unwrap();
        assert_eq!(lines[0].n, 1);
        assert_eq!(lines[0].text, "fn main() {}");
    }

    #[test]
    fn source_refuses_escape() {
        let d = tmp_ws();
        let err = read_whole_file(d.path(), "../secret").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn source_soft_fails_on_missing_file() {
        let d = tmp_ws();
        let fc = read_whole_file(d.path(), "src/nope.rs").unwrap();
        assert!(!fc.available);
        assert!(fc.reason.is_some());
        assert!(fc.lines.is_none());
    }

    #[test]
    fn source_soft_fails_on_oversized_file() {
        let d = tmp_ws();
        let big = vec![b'a'; (MAX_FILE_BYTES + 1) as usize];
        std::fs::write(d.path().join("big.bin"), &big).unwrap();
        let fc = read_whole_file(d.path(), "big.bin").unwrap();
        assert!(!fc.available);
        assert!(fc.reason.as_deref().unwrap().contains("too large"));
    }

    #[test]
    fn source_soft_fails_on_non_utf8() {
        let d = tmp_ws();
        std::fs::write(d.path().join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        let fc = read_whole_file(d.path(), "bin.dat").unwrap();
        assert!(!fc.available);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp --lib api::code::tests -- --nocapture`
Expected: FAIL — `cannot find function read_whole_file` / `MAX_FILE_BYTES`.

- [ ] **Step 3: Write the source reader + handler**

Add to `code.rs` (below the tree code, above `routes()`):

```rust
use crate::api::source::{detect_language, SourceLine};

/// Whole-file cap. Larger files soft-fail (the viewer shows a placeholder).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<SourceLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl FileContent {
    fn unavailable(reason: impl Into<String>) -> Self {
        FileContent {
            available: false,
            reason: Some(reason.into()),
            ..Default::default()
        }
    }
}

/// Read a workspace-relative file whole (up to `MAX_FILE_BYTES`). Missing,
/// oversized, non-file, or non-UTF-8 targets return `available:false` with a
/// reason (HTTP 200) — only path-safety violations are hard errors.
fn read_whole_file(workspace: &FsPath, rel: &str) -> Result<FileContent, ApiError> {
    let file = crate::api::source::resolve_under_workspace(workspace, rel)?;
    if !file.is_file() {
        return Ok(FileContent::unavailable("file not found"));
    }
    let meta = match std::fs::metadata(&file) {
        Ok(m) => m,
        Err(_) => return Ok(FileContent::unavailable("file not found")),
    };
    if meta.len() > MAX_FILE_BYTES {
        return Ok(FileContent::unavailable("file too large to display"));
    }
    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) => return Ok(FileContent::unavailable(e.to_string())),
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Ok(FileContent::unavailable("binary or non-UTF-8 file")),
    };
    let lines: Vec<SourceLine> = text
        .lines()
        .enumerate()
        .map(|(i, l)| SourceLine {
            n: i + 1,
            text: l.to_string(),
        })
        .collect();
    Ok(FileContent {
        available: true,
        path: Some(rel.to_string()),
        language: detect_language(&file),
        total_lines: Some(lines.len()),
        lines: Some(lines),
        reason: None,
    })
}

async fn get_source(
    Path(ws_id): Path<String>,
    Query(q): Query<TreeQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<FileContent>> {
    let w = load_workspace(&s, &ws_id)?;
    let fc = read_whole_file(FsPath::new(&w.path), &q.path)?;
    Ok(Json(fc))
}
```

Extend `routes()` to register the second endpoint (it reuses `TreeQuery` for the `?path=` param):

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/:ws_id/tree", get(get_tree))
        .route("/api/projects/:ws_id/source", get(get_source))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp --lib api::code::tests -- --nocapture`
Expected: PASS (10 tests total).

- [ ] **Step 5: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/code.rs
git add crates/rupu-cp/src/api/code.rs
git commit -m "feat(cp): project-scoped whole-file source endpoint

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: API client calls + types

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`
- Test: `crates/rupu-cp/web/src/lib/api.code.test.ts` (new)

**Interfaces:**
- Consumes: the `request<T>(path, init?)` base helper and the `api` object (both existing in `api.ts`).
- Produces (components rely on these EXACT names): types `TreeEntry { name: string; path: string; kind: 'dir' | 'file' }`, `TreeResult { path: string; parent: string | null; entries: TreeEntry[] }`, `FileContent { available: boolean; path?: string; language?: string | null; totalLines?: number; lines?: { n: number; text: string }[]; reason?: string }`; methods `api.getProjectTree(wsId: string, path?: string): Promise<TreeResult>` and `api.getProjectSource(wsId: string, path: string): Promise<FileContent>`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/api.code.test.ts`:

```ts
import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('project code API', () => {
  it('getProjectTree encodes ws_id in the path and path as a query param', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ path: '', parent: null, entries: [] })));
    await api.getProjectTree('ws 1', 'src/a');
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('/api/projects/ws%201/tree?path=src%2Fa');
  });

  it('getProjectTree omits path when at root', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ path: '', parent: null, entries: [] })));
    await api.getProjectTree('ws1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/ws1/tree');
  });

  it('getProjectSource builds the source URL', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ available: false })));
    await api.getProjectSource('ws1', 'src/main.rs');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/ws1/source?path=src%2Fmain.rs');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/api.code.test.ts`
Expected: FAIL — `api.getProjectTree is not a function`.

- [ ] **Step 3: Add the types and methods**

In `crates/rupu-cp/web/src/lib/api.ts`, add these type exports near the other DTO interfaces (e.g. just above `SourceSlice`):

```ts
export interface TreeEntry {
  name: string;
  path: string;
  kind: 'dir' | 'file';
}
export interface TreeResult {
  path: string;
  parent: string | null;
  entries: TreeEntry[];
}
export interface FileContent {
  available: boolean;
  path?: string;
  language?: string | null;
  totalLines?: number;
  lines?: { n: number; text: string }[];
  reason?: string;
}
```

Inside the `export const api = { ... }` object, next to `getProjectRuns`, add:

```ts
  getProjectTree(wsId: string, path?: string): Promise<TreeResult> {
    const qs = path ? `?path=${encodeURIComponent(path)}` : '';
    return request<TreeResult>(`/api/projects/${encodeURIComponent(wsId)}/tree${qs}`);
  },
  getProjectSource(wsId: string, path: string): Promise<FileContent> {
    return request<FileContent>(
      `/api/projects/${encodeURIComponent(wsId)}/source?path=${encodeURIComponent(path)}`,
    );
  },
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/api.code.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/lib/api.code.test.ts
git commit -m "feat(web): getProjectTree/getProjectSource API client calls

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Theme-aware CodeHighlight (add dark)

**Files:**
- Modify: `crates/rupu-cp/web/src/components/CodeHighlight.tsx`
- Create: `crates/rupu-cp/web/src/components/codeHighlight.css` (both-theme hljs token colors)
- Test: `crates/rupu-cp/web/src/components/CodeHighlight.theme.test.tsx` (new)

**Interfaces:**
- Consumes: `useTheme()` from `./theme/ThemeProvider` returning `{ mode: 'light' | 'dark' }`.
- Produces: `CodeHighlight` renders identical HTML structure but its `<code>`/`<pre>` carries `data-hl-theme={mode}` so CSS selects the palette. No props change (existing `CodeHighlightProps` unchanged), so `SourcePreview` and the new `CodeViewer` need no edits to consume it.

**Why not the static `github.css` import:** it hardcodes light hexes and cannot switch. We replace it with a scoped stylesheet defining `.hljs` token colors under `[data-hl-theme="light"]` and `[data-hl-theme="dark"]`, using the GitHub-dark palette already documented in `codeHighlightTheme.ts` (`#ff7b72` keyword, `#a5d6ff` string, `#8b949e` comment, `#79c0ff` number, `#d2a8ff` title, `#7ee787` name, `#e6edf3` base) so the editor and the viewer match.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/CodeHighlight.theme.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import CodeHighlight from './CodeHighlight';
import { ThemeProvider } from './theme/ThemeProvider';

afterEach(cleanup);

function renderWithTheme(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('CodeHighlight theming', () => {
  it('stamps the resolved theme mode on the rendered code element', () => {
    document.documentElement.dataset.theme = 'dark';
    const { container } = renderWithTheme(
      <CodeHighlight code={'fn main() {}'} language="rust" inline />,
    );
    const code = container.querySelector('code.hljs');
    expect(code).not.toBeNull();
    expect(code!.getAttribute('data-hl-theme')).toBe('dark');
  });

  it('uses light when the document theme is light', () => {
    document.documentElement.dataset.theme = 'light';
    const { container } = renderWithTheme(
      <CodeHighlight code={'fn main() {}'} language="rust" inline />,
    );
    expect(container.querySelector('code.hljs')!.getAttribute('data-hl-theme')).toBe('light');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/CodeHighlight.theme.test.tsx`
Expected: FAIL — `data-hl-theme` attribute is null (not yet emitted).

- [ ] **Step 3: Create the two-theme stylesheet**

Create `crates/rupu-cp/web/src/components/codeHighlight.css`:

```css
/* Two-theme highlight.js token palette, selected at runtime via
   [data-hl-theme]. Light mirrors GitHub-light; dark mirrors the GitHub-dark
   hexes documented in codeHighlightTheme.ts so the editor and the source
   viewer render identically. Base text/background come from --c-* tokens so
   the block sits correctly in either theme. */
.hljs {
  color: rgb(var(--c-ink));
  background: transparent;
}
[data-hl-theme='light'].hljs .hljs-keyword,
[data-hl-theme='light'] .hljs-keyword,
[data-hl-theme='light'] .hljs-type { color: #cf222e; }
[data-hl-theme='light'] .hljs-string { color: #0a3069; }
[data-hl-theme='light'] .hljs-comment { color: #6e7781; }
[data-hl-theme='light'] .hljs-number { color: #0550ae; }
[data-hl-theme='light'] .hljs-title,
[data-hl-theme='light'] .hljs-title.function_ { color: #8250df; }
[data-hl-theme='light'] .hljs-name,
[data-hl-theme='light'] .hljs-tag { color: #116329; }

[data-hl-theme='dark'].hljs,
[data-hl-theme='dark'] { color: #e6edf3; }
[data-hl-theme='dark'] .hljs-keyword,
[data-hl-theme='dark'] .hljs-type { color: #ff7b72; }
[data-hl-theme='dark'] .hljs-string { color: #a5d6ff; }
[data-hl-theme='dark'] .hljs-comment { color: #8b949e; }
[data-hl-theme='dark'] .hljs-number { color: #79c0ff; }
[data-hl-theme='dark'] .hljs-title,
[data-hl-theme='dark'] .hljs-title.function_ { color: #d2a8ff; }
[data-hl-theme='dark'] .hljs-name,
[data-hl-theme='dark'] .hljs-tag { color: #7ee787; }
```

- [ ] **Step 4: Rewire `CodeHighlight.tsx`**

In `crates/rupu-cp/web/src/components/CodeHighlight.tsx`:

Replace the static light import
```ts
import 'highlight.js/styles/github.css';
```
with our scoped stylesheet:
```ts
import './codeHighlight.css';
```

Add the theme hook import near the top (with the other imports):
```ts
import { useTheme } from './theme/ThemeProvider';
```

In the component body, read the mode and stamp it on the rendered element. Find the `inline` return (currently `return <code className="hljs whitespace-pre font-mono" dangerouslySetInnerHTML={{ __html: html }} />;`) and the block return (`<pre className={PRE_CLASS}><code className="hljs" .../></pre>`), and add `data-hl-theme={mode}`:

```tsx
  const { mode } = useTheme();
  // ...existing html computation...
  if (inline) {
    return (
      <code
        className="hljs whitespace-pre font-mono"
        data-hl-theme={mode}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    );
  }
  return (
    <pre className={PRE_CLASS}>
      <code className="hljs" data-hl-theme={mode} dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
```

(Keep the existing `frontmatter` branch as-is but add `data-hl-theme={mode}` to its rendered `<code>`/`<pre>` too, so agent `.md` previews theme correctly.)

- [ ] **Step 5: Run the theme test + the existing CodeHighlight test**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/CodeHighlight.theme.test.tsx src/components/CodeHighlight.test.ts`
Expected: PASS (new theme tests + all pre-existing CodeHighlight tests still green).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/components/CodeHighlight.tsx crates/rupu-cp/web/src/components/codeHighlight.css crates/rupu-cp/web/src/components/CodeHighlight.theme.test.tsx
git commit -m "feat(web): theme-aware syntax highlighting (add dark palette)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

> **Runtime check flagged for operator:** SourcePreview (transcript) now renders dark in dark mode — matt should eyeball a transcript source preview in both themes when validating Phase 1.

---

### Task 5: Drift-detection pure function

**Files:**
- Create: `crates/rupu-cp/web/src/components/code/drift.ts`
- Test: `crates/rupu-cp/web/src/components/code/drift.test.ts` (new)

**Interfaces:**
- Produces: `isFindingStale(excerpt: string | null | undefined, fileLines: { n: number; text: string }[], lineRange: [number, number] | null | undefined): boolean`. Returns `false` when `excerpt` is absent/empty or `lineRange` is absent (drift unknown → not stale, no note). Otherwise compares the excerpt against the current file slice `lineRange[0]..=lineRange[1]` with whitespace-normalized line comparison.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/code/drift.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { isFindingStale } from './drift';

const lines = (arr: string[]) => arr.map((text, i) => ({ n: i + 1, text }));

describe('isFindingStale', () => {
  const file = lines(['fn a() {}', '  let x = 1;', '  let y = 2;', '}']);

  it('is not stale when the excerpt still matches the range', () => {
    expect(isFindingStale('let x = 1;\nlet y = 2;', file, [2, 3])).toBe(false);
  });

  it('tolerates leading/trailing whitespace differences', () => {
    expect(isFindingStale('   let x = 1;\n\tlet y = 2;  ', file, [2, 3])).toBe(false);
  });

  it('is stale when the code at the range changed', () => {
    expect(isFindingStale('let x = 1;\nlet y = 2;', lines(['fn a() {}', '  moved();', '}']), [2, 3])).toBe(
      true,
    );
  });

  it('is not stale when the excerpt is missing (drift unknown)', () => {
    expect(isFindingStale(undefined, file, [2, 3])).toBe(false);
    expect(isFindingStale('', file, [2, 3])).toBe(false);
  });

  it('is not stale when lineRange is missing', () => {
    expect(isFindingStale('anything', file, null)).toBe(false);
  });

  it('is stale when the range runs past the end of the file', () => {
    expect(isFindingStale('let x = 1;', lines(['only one line']), [5, 5])).toBe(true);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/drift.test.ts`
Expected: FAIL — cannot resolve `./drift`.

- [ ] **Step 3: Write the function**

Create `crates/rupu-cp/web/src/components/code/drift.ts`:

```ts
/** Collapse a line to its comparable core: trim ends, collapse inner runs of
 *  whitespace to a single space. Drift detection should ignore reindentation
 *  and trailing-newline noise, not real edits. */
function norm(s: string): string {
  return s.replace(/\s+/g, ' ').trim();
}

/**
 * True when a finding's recorded `code_excerpt` no longer matches the current
 * file content at its `line_range`. Absent excerpt or range → drift is
 * unknown, reported as not-stale (no note shown).
 */
export function isFindingStale(
  excerpt: string | null | undefined,
  fileLines: { n: number; text: string }[],
  lineRange: [number, number] | null | undefined,
): boolean {
  if (!excerpt || !excerpt.trim() || !lineRange) return false;
  const [start, end] = lineRange;
  const current: string[] = [];
  for (let n = start; n <= end; n++) {
    const ln = fileLines[n - 1];
    if (!ln) return true; // range runs past EOF → definitely drifted
    current.push(ln.text);
  }
  const want = excerpt
    .split('\n')
    .map(norm)
    .filter((l) => l.length > 0);
  const have = current.map(norm).filter((l) => l.length > 0);
  if (want.length === 0) return false;
  // The excerpt must appear as a contiguous, in-order match of the range.
  return want.join('\n') !== have.join('\n');
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/drift.test.ts`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/code/drift.ts crates/rupu-cp/web/src/components/code/drift.test.ts
git commit -m "feat(web): client-side finding drift detection

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: CodeViewer — whole-file view with severity bands + inline finding cards

This is the centerpiece. Split into a presentational `InlineFindingCard` (its own test) then the `CodeViewer` that composes it. **Use the `frontend-design` skill for the visual crafting of both** (severity bands, squiggle, PR-style card) so the result matches the aikido reference and is not generic.

**Files:**
- Create: `crates/rupu-cp/web/src/components/code/InlineFindingCard.tsx`
- Create: `crates/rupu-cp/web/src/components/code/CodeViewer.tsx`
- Test: `crates/rupu-cp/web/src/components/code/InlineFindingCard.test.tsx`, `crates/rupu-cp/web/src/components/code/CodeViewer.test.tsx`

**Interfaces:**
- Consumes: `api.getProjectSource` + `FileContent` (Task 3); `isFindingStale` (Task 5); `CodeHighlight` (Task 4); `FindingRecord` type from `../../lib/api` (fields: `id`, `file_path?`, `line_range?: [number, number]`, `summary`, `severity`, `concern_id?`, `evidence: { code_excerpt?: string; rationale: string; references: string[] }`, `permalink?: string` — `permalink` added in Task 12; treat as optional here); `SEVERITY_STYLE` + `Severity` from `../../lib/severity`; `MarkdownView` if one exists for rationale (else render as `<pre>` — check `src/components/` for an existing markdown renderer, e.g. the transcript one, and reuse it).
- Produces: `CodeViewer` props `{ wsId: string; path: string; findings: FindingRecord[]; initialLine?: number }`. `InlineFindingCard` props `{ finding: FindingRecord; stale: boolean }`.

- [ ] **Step 1: Write the failing test for InlineFindingCard**

Create `crates/rupu-cp/web/src/components/code/InlineFindingCard.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ThemeProvider } from '../theme/ThemeProvider';
import InlineFindingCard from './InlineFindingCard';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [17, 17],
  summary: 'Missing tenant check on billing read',
  severity: 'high',
  evidence: {
    code_excerpt: 'let bill = db.get(org_id);',
    rationale: 'Line 17 checks orgId but **never** userId.',
    references: ['CWE-639'],
  },
} as unknown as FindingRecord;

function view(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('InlineFindingCard', () => {
  it('shows the collapsed summary and expands on click', () => {
    view(<InlineFindingCard finding={FINDING} stale={false} />);
    expect(screen.getByText('Missing tenant check on billing read')).toBeInTheDocument();
    // rationale hidden until expanded
    expect(screen.queryByText(/never/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(screen.getByText(/never/)).toBeInTheDocument();
  });

  it('renders the stale note when stale', () => {
    view(<InlineFindingCard finding={FINDING} stale={true} />);
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(screen.getByText(/code may have changed/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/InlineFindingCard.test.tsx`
Expected: FAIL — cannot resolve `./InlineFindingCard`.

- [ ] **Step 3: Write InlineFindingCard**

Create `crates/rupu-cp/web/src/components/code/InlineFindingCard.tsx`:

```tsx
import { useState } from 'react';
import type { FindingRecord } from '../../lib/api';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';

export interface InlineFindingCardProps {
  finding: FindingRecord;
  stale: boolean;
}

/** A PR-style inline comment sitting under a finding's line range: a collapsed
 *  one-line marker (severity dot + summary) that expands to rationale, refs,
 *  a repo permalink, and a drift note. */
export default function InlineFindingCard({ finding, stale }: InlineFindingCardProps) {
  const [open, setOpen] = useState(false);
  const sev = (finding.severity as Severity) ?? 'info';
  const style = SEVERITY_STYLE[sev] ?? SEVERITY_STYLE.info;

  return (
    <div className={`my-1 rounded-md border ${style.ring} bg-surface`}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px]"
      >
        <span className={`h-2 w-2 shrink-0 rounded-full ${style.bar}`} aria-hidden />
        <span className="font-medium text-ink">{finding.summary}</span>
        <span className={`ml-auto shrink-0 rounded px-1.5 py-0.5 text-[10px] ${style.pill}`}>
          {style.label}
        </span>
      </button>
      {open && (
        <div className="border-t border-border px-3 py-2 text-[12px] text-ink-dim">
          {stale && (
            <div className="mb-2 rounded bg-warn-bg px-2 py-1 text-[11px] text-ink">
              ⚠ The code may have changed since this finding was recorded — the line below is
              where it was found.
            </div>
          )}
          <p className="whitespace-pre-wrap">{finding.evidence?.rationale}</p>
          {finding.concern_id && (
            <div className="mt-2 text-[11px] text-ink-mute">Concern: {finding.concern_id}</div>
          )}
          {finding.evidence?.references?.length > 0 && (
            <div className="mt-1 flex flex-wrap gap-1">
              {finding.evidence.references.map((r) => (
                <span
                  key={r}
                  className="rounded bg-panel px-1.5 py-0.5 text-[10.5px] font-mono text-ink-dim"
                >
                  {r}
                </span>
              ))}
            </div>
          )}
          {finding.permalink && (
            <a
              href={finding.permalink}
              target="_blank"
              rel="noreferrer"
              className="mt-2 inline-block text-[11px] text-brand-700 hover:underline"
            >
              View on repository ↗
            </a>
          )}
        </div>
      )}
    </div>
  );
}
```

> **Note on `MarkdownView`:** the rationale is markdown. If a shared markdown renderer exists (search `src/components/` — the transcript renderer at `src/components/transcript/` likely has one), swap the `<p className="whitespace-pre-wrap">` for it. If none is trivially reusable, the `<pre>`/whitespace form ships for v1 and a follow-up richens it; do NOT add a markdown dependency.

- [ ] **Step 4: Run to verify InlineFindingCard passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/InlineFindingCard.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Write the failing test for CodeViewer**

Create `crates/rupu-cp/web/src/components/code/CodeViewer.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import { ThemeProvider } from '../theme/ThemeProvider';
import CodeViewer from './CodeViewer';
import { api } from '../../lib/api';
import type { FindingRecord, FileContent } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const FILE: FileContent = {
  available: true,
  path: 'src/billing.rs',
  language: 'rust',
  totalLines: 3,
  lines: [
    { n: 1, text: 'fn read(org_id: u64) {' },
    { n: 2, text: '  let bill = db.get(org_id);' },
    { n: 3, text: '}' },
  ],
};

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [2, 2],
  summary: 'Missing tenant check',
  severity: 'high',
  evidence: { code_excerpt: 'let bill = db.get(org_id);', rationale: 'no userId check', references: [] },
} as unknown as FindingRecord;

function view(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('CodeViewer', () => {
  it('renders the file and an inline finding marker at its line', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(FILE);
    view(<CodeViewer wsId="ws1" path="src/billing.rs" findings={[FINDING]} />);
    await waitFor(() => expect(screen.getByText('Missing tenant check')).toBeInTheDocument());
    // the anchored line carries a finding marker (data-finding-line=2)
    expect(document.querySelector('[data-finding-line="2"]')).not.toBeNull();
  });

  it('stacks multiple findings on the same line', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue(FILE);
    const second = { ...FINDING, id: 'f2', summary: 'Also logs PII' } as FindingRecord;
    view(<CodeViewer wsId="ws1" path="src/billing.rs" findings={[FINDING, second]} />);
    await waitFor(() => expect(screen.getByText('Missing tenant check')).toBeInTheDocument());
    expect(screen.getByText('Also logs PII')).toBeInTheDocument();
  });

  it('shows a placeholder when the file is unavailable', async () => {
    vi.spyOn(api, 'getProjectSource').mockResolvedValue({ available: false, reason: 'file too large to display' });
    view(<CodeViewer wsId="ws1" path="x" findings={[]} />);
    await waitFor(() => expect(screen.getByText(/too large/)).toBeInTheDocument());
  });
});
```

- [ ] **Step 6: Run to verify CodeViewer test fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/CodeViewer.test.tsx`
Expected: FAIL — cannot resolve `./CodeViewer`.

- [ ] **Step 7: Write CodeViewer**

Create `crates/rupu-cp/web/src/components/code/CodeViewer.tsx`:

```tsx
import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { api, type FileContent, type FindingRecord } from '../../lib/api';
import { SOURCE_PREVIEW_LANGUAGES } from '../CodeHighlight';
import CodeHighlight from '../CodeHighlight';
import { SEVERITY_STYLE, type Severity, severityRank } from '../../lib/severity';
import { isFindingStale } from './drift';
import InlineFindingCard from './InlineFindingCard';

export interface CodeViewerProps {
  wsId: string;
  path: string;
  findings: FindingRecord[];
  initialLine?: number;
}

type Load = { state: 'loading' } | { state: 'error'; msg: string } | { state: 'loaded'; file: FileContent };

/** Group findings by their anchor line (line_range[0]); worst-severity first
 *  within a line so the band color reflects the most severe. */
function byLine(findings: FindingRecord[]): Map<number, FindingRecord[]> {
  const m = new Map<number, FindingRecord[]>();
  for (const f of findings) {
    if (!f.line_range) continue;
    const anchor = f.line_range[0];
    const arr = m.get(anchor) ?? [];
    arr.push(f);
    m.set(anchor, arr);
  }
  for (const arr of m.values()) {
    arr.sort((a, b) => severityRank(b.severity as Severity) - severityRank(a.severity as Severity));
  }
  return m;
}

export default function CodeViewer({ wsId, path, findings, initialLine }: CodeViewerProps) {
  const [load, setLoad] = useState<Load>({ state: 'loading' });
  const anchorRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let live = true;
    setLoad({ state: 'loading' });
    api
      .getProjectSource(wsId, path)
      .then((file) => live && setLoad({ state: 'loaded', file }))
      .catch((e) => live && setLoad({ state: 'error', msg: String(e?.message ?? e) }));
    return () => {
      live = false;
    };
  }, [wsId, path]);

  const findingsForFile = useMemo(
    () => findings.filter((f) => f.file_path === path),
    [findings, path],
  );
  const grouped = useMemo(() => byLine(findingsForFile), [findingsForFile]);

  // Scroll the anchor line into view once loaded.
  useEffect(() => {
    if (load.state === 'loaded' && initialLine && anchorRef.current) {
      anchorRef.current.scrollIntoView({ block: 'center' });
    }
  }, [load.state, initialLine]);

  if (load.state === 'loading') {
    return (
      <div className="flex h-40 items-center justify-center gap-2 text-ink-dim text-sm">
        <Loader2 size={14} className="animate-spin" /> Loading source…
      </div>
    );
  }
  if (load.state === 'error') {
    return <div className="p-4 text-sm text-err">Could not load file: {load.msg}</div>;
  }
  const { file } = load;
  if (!file.available || !file.lines) {
    return (
      <div className="p-6 text-sm text-ink-dim">
        {file.reason ?? 'This file cannot be displayed.'}
      </div>
    );
  }

  const lang =
    file.language && SOURCE_PREVIEW_LANGUAGES.has(file.language) ? file.language : null;

  return (
    <div className="overflow-x-auto rounded-md border border-border bg-panel text-[12px]">
      <pre className="m-0 font-mono leading-5">
        {file.lines.map((ln) => {
          const here = grouped.get(ln.n);
          const worst = here?.[0];
          const sev = worst ? ((worst.severity as Severity) ?? 'info') : null;
          const style = sev ? SEVERITY_STYLE[sev] : null;
          const isAnchor = initialLine === ln.n;
          return (
            <div key={ln.n}>
              <div
                ref={isAnchor ? anchorRef : undefined}
                data-finding-line={here ? ln.n : undefined}
                className={`flex ${style ? `${style.bg} border-l-2 ${style.barBorder}` : 'border-l-2 border-transparent'}`}
              >
                <span
                  className="select-none pr-3 pl-3 text-right text-ink-mute"
                  style={{ minWidth: '4ch' }}
                >
                  {ln.n}
                </span>
                {lang ? (
                  <CodeHighlight code={ln.text} language={lang as 'rust'} inline />
                ) : (
                  <code className="whitespace-pre font-mono text-ink">{ln.text}</code>
                )}
              </div>
              {here && (
                <div className="pl-[5ch] pr-3">
                  {here.map((f) => (
                    <InlineFindingCard
                      key={f.id}
                      finding={f}
                      stale={isFindingStale(f.evidence?.code_excerpt, file.lines!, f.line_range)}
                    />
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </pre>
    </div>
  );
}
```

This references two helpers that must exist in `src/lib/severity.ts`: `severityRank(sev): number` and a `barBorder` field on each `SEVERITY_STYLE` entry. Add them.

In `crates/rupu-cp/web/src/lib/severity.ts`:
- Add a `barBorder` Tailwind class to each severity entry (border-color counterpart of `bar`), e.g. critical `barBorder: 'border-sev-critical'`, high `border-sev-high`, medium `border-sev-medium`, low `border-sev-low`, info `border-sev-info`.
- Add and export:
```ts
export function severityRank(sev: Severity): number {
  return { critical: 4, high: 3, medium: 2, low: 1, info: 0 }[sev] ?? 0;
}
```

- [ ] **Step 8: Run to verify CodeViewer passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/CodeViewer.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 9: Squiggle underline on the anchor line (visual polish)**

Add a squiggle under the finding's anchor line text (a reference cue). In `codeHighlight.css` (or a new `code/codeViewer.css` imported by `CodeViewer`), add a `.finding-squiggle` class using a repeating linear-gradient underline colored via `currentColor`, and apply it on the `<code>` of a finding line. Keep it token-driven (no literal colors beyond the wavy gradient geometry). Manual/visual — no unit assertion; the operator validates it. Commit with the viewer.

- [ ] **Step 10: Format-check and commit**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run src/components/code/`
Expected: type-clean; all `code/` tests pass.

```bash
git add crates/rupu-cp/web/src/components/code/ crates/rupu-cp/web/src/lib/severity.ts
git commit -m "feat(web): CodeViewer — whole-file view with inline findings + severity bands

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Phase 1 operator gate:** matt runs the CP, opens a file with findings via a temporary harness (or waits for Phase 2's tab), and confirms the viewer renders correctly in **light and dark**. Do not proceed to merge Phase 1 alone without this; Phase 2 makes it reachable in the UI.

---

## Phase 2 — Code tab + file navigator

### Task 7: FileTree navigator (lazy, finding badges, folder rollup)

**Files:**
- Create: `crates/rupu-cp/web/src/components/code/FileTree.tsx`
- Test: `crates/rupu-cp/web/src/components/code/FileTree.test.tsx`

**Interfaces:**
- Consumes: `api.getProjectTree` + `TreeResult`/`TreeEntry` (Task 3); `FindingRecord[]` for badge data (fetched by the parent tab and passed down); `SEVERITY_STYLE`/`severityRank` (Task 6).
- Produces: `FileTree` props `{ wsId: string; findings: FindingRecord[]; selectedPath: string | null; onSelect: (path: string) => void }`. Emits `onSelect(path)` when a file node is clicked.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/code/FileTree.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import FileTree from './FileTree';
import { api } from '../../lib/api';
import type { FindingRecord, TreeResult } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ROOT: TreeResult = {
  path: '',
  parent: null,
  entries: [
    { name: 'src', path: 'src', kind: 'dir' },
    { name: 'README.md', path: 'README.md', kind: 'file' },
  ],
};
const SRC: TreeResult = {
  path: 'src',
  parent: '',
  entries: [{ name: 'billing.rs', path: 'src/billing.rs', kind: 'file' }],
};

const FINDINGS = [
  { id: 'f1', file_path: 'src/billing.rs', line_range: [2, 2], severity: 'high', summary: 's', evidence: { rationale: '', references: [] } },
] as unknown as FindingRecord[];

describe('FileTree', () => {
  it('renders root entries and a folder rollup badge', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue(ROOT);
    render(<FileTree wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={() => {}} />);
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    // src rolls up billing.rs's high finding
    expect(screen.getByTestId('badge-src')).toHaveTextContent('1');
  });

  it('lazy-loads a folder on expand and selects a file', async () => {
    const spy = vi.spyOn(api, 'getProjectTree');
    spy.mockResolvedValueOnce(ROOT).mockResolvedValueOnce(SRC);
    const onSelect = vi.fn();
    render(<FileTree wsId="ws1" findings={FINDINGS} selectedPath={null} onSelect={onSelect} />);
    await waitFor(() => expect(screen.getByText('src')).toBeInTheDocument());
    fireEvent.click(screen.getByText('src'));
    await waitFor(() => expect(screen.getByText('billing.rs')).toBeInTheDocument());
    fireEvent.click(screen.getByText('billing.rs'));
    expect(onSelect).toHaveBeenCalledWith('src/billing.rs');
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/FileTree.test.tsx`
Expected: FAIL — cannot resolve `./FileTree`.

- [ ] **Step 3: Write FileTree**

Create `crates/rupu-cp/web/src/components/code/FileTree.tsx`:

```tsx
import { useEffect, useMemo, useState } from 'react';
import { ChevronRight, ChevronDown, File as FileIcon, Loader2 } from 'lucide-react';
import { api, type FindingRecord, type TreeEntry, type TreeResult } from '../../lib/api';
import { SEVERITY_STYLE, severityRank, type Severity } from '../../lib/severity';

export interface FileTreeProps {
  wsId: string;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

/** Worst severity among findings whose file_path is at-or-under `prefix`
 *  (folders) or exactly `path` (files), plus a count. Null when none. */
function rollup(findings: FindingRecord[], prefix: string, isDir: boolean) {
  const match = findings.filter((f) => {
    if (!f.file_path) return false;
    return isDir ? f.file_path === prefix || f.file_path.startsWith(prefix + '/') : f.file_path === prefix;
  });
  if (match.length === 0) return null;
  const worst = match.reduce<Severity>((acc, f) => {
    const s = (f.severity as Severity) ?? 'info';
    return severityRank(s) > severityRank(acc) ? s : acc;
  }, 'info');
  return { worst, count: match.length };
}

function Badge({ node, findings }: { node: TreeEntry; findings: FindingRecord[] }) {
  const r = rollup(findings, node.path, node.kind === 'dir');
  if (!r) return null;
  const style = SEVERITY_STYLE[r.worst];
  return (
    <span
      data-testid={`badge-${node.path}`}
      className={`ml-auto shrink-0 rounded-full px-1.5 text-[10px] ${style.pill}`}
    >
      {r.count}
    </span>
  );
}

function Dir({
  node,
  depth,
  wsId,
  findings,
  selectedPath,
  onSelect,
}: {
  node: TreeEntry;
  depth: number;
  wsId: string;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (p: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<TreeEntry[] | null>(null);
  const [loading, setLoading] = useState(false);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && children === null) {
      setLoading(true);
      api
        .getProjectTree(wsId, node.path)
        .then((r) => setChildren(r.entries))
        .finally(() => setLoading(false));
    }
  };

  return (
    <div>
      <button
        type="button"
        onClick={toggle}
        className="flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[12px] text-ink-dim hover:bg-surface-hover"
        style={{ paddingLeft: `${depth * 12 + 4}px` }}
      >
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className="text-ink">{node.name}</span>
        <Badge node={node} findings={findings} />
      </button>
      {open &&
        (loading ? (
          <div className="pl-6 py-0.5 text-ink-mute">
            <Loader2 size={12} className="animate-spin" />
          </div>
        ) : (
          children?.map((c) =>
            c.kind === 'dir' ? (
              <Dir
                key={c.path}
                node={c}
                depth={depth + 1}
                wsId={wsId}
                findings={findings}
                selectedPath={selectedPath}
                onSelect={onSelect}
              />
            ) : (
              <FileNode
                key={c.path}
                node={c}
                depth={depth + 1}
                findings={findings}
                selectedPath={selectedPath}
                onSelect={onSelect}
              />
            ),
          )
        ))}
    </div>
  );
}

function FileNode({
  node,
  depth,
  findings,
  selectedPath,
  onSelect,
}: {
  node: TreeEntry;
  depth: number;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (p: string) => void;
}) {
  const active = selectedPath === node.path;
  return (
    <button
      type="button"
      onClick={() => onSelect(node.path)}
      className={`flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[12px] ${active ? 'bg-panel text-ink ring-1 ring-border' : 'text-ink-dim hover:bg-surface-hover'}`}
      style={{ paddingLeft: `${depth * 12 + 18}px` }}
    >
      <FileIcon size={12} className="shrink-0 text-ink-mute" />
      <span className="truncate">{node.name}</span>
      <Badge node={node} findings={findings} />
    </button>
  );
}

export default function FileTree({ wsId, findings, selectedPath, onSelect }: FileTreeProps) {
  const [root, setRoot] = useState<TreeResult | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .getProjectTree(wsId)
      .then((r) => live && setRoot(r))
      .catch((e) => live && setErr(String(e?.message ?? e)));
    return () => {
      live = false;
    };
  }, [wsId]);

  const entries = useMemo(() => root?.entries ?? [], [root]);
  if (err) return <div className="p-2 text-[12px] text-err">Tree error: {err}</div>;
  if (!root)
    return (
      <div className="flex items-center gap-2 p-2 text-[12px] text-ink-dim">
        <Loader2 size={12} className="animate-spin" /> Loading files…
      </div>
    );

  return (
    <div className="py-1">
      {entries.map((e) =>
        e.kind === 'dir' ? (
          <Dir key={e.path} node={e} depth={0} wsId={wsId} findings={findings} selectedPath={selectedPath} onSelect={onSelect} />
        ) : (
          <FileNode key={e.path} node={e} depth={0} findings={findings} selectedPath={selectedPath} onSelect={onSelect} />
        ),
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/FileTree.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/code/FileTree.tsx crates/rupu-cp/web/src/components/code/FileTree.test.tsx
git commit -m "feat(web): FileTree navigator with finding badges + folder rollup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Code tab shell + route + tab wiring

**Files:**
- Create: `crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx`
- Modify: `crates/rupu-cp/web/src/pages/ProjectDetail.tsx` (add `'code'` to `ProjectTab`, a `TabButton`, and the body render)
- Modify: `crates/rupu-cp/web/src/App.tsx` (route before `:wsId` wildcard)
- Test: `crates/rupu-cp/web/src/components/project/ProjectCodeTab.test.tsx`

**Interfaces:**
- Consumes: `FileTree` (Task 7), `CodeViewer` (Task 6), `api.getFindings` (existing — `{ wsId }` → `FindingsResponse { findings: FindingOut[] }`; `FindingOut` flattens `FindingRecord`, so each item has `file_path`, `line_range`, `evidence`, `severity`, `permalink?`), `useSearchParams` from react-router-dom.
- Produces: `ProjectCodeTab` props `{ wsId: string }`. Reads `?path=` and `?line=` from the URL for finding-first deep-links (Task 9 writes them).

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/project/ProjectCodeTab.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { ThemeProvider } from '../theme/ThemeProvider';
import ProjectCodeTab from './ProjectCodeTab';
import { api } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ProjectCodeTab', () => {
  it('loads the tree and findings and shows an empty-state prompt before a file is picked', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue({ path: '', parent: null, entries: [] });
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [], summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 } } as never);
    render(
      <MemoryRouter initialEntries={['/projects/ws1/code']}>
        <ThemeProvider>
          <ProjectCodeTab wsId="ws1" />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/select a file/i)).toBeInTheDocument());
  });

  it('opens the file named by the ?path= deep-link', async () => {
    vi.spyOn(api, 'getProjectTree').mockResolvedValue({ path: '', parent: null, entries: [] });
    vi.spyOn(api, 'getFindings').mockResolvedValue({ findings: [], summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 } } as never);
    const src = vi.spyOn(api, 'getProjectSource').mockResolvedValue({ available: true, path: 'src/a.rs', language: 'rust', totalLines: 1, lines: [{ n: 1, text: 'fn a() {}' }] });
    render(
      <MemoryRouter initialEntries={['/projects/ws1/code?path=src%2Fa.rs&line=1']}>
        <ThemeProvider>
          <ProjectCodeTab wsId="ws1" />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => expect(src).toHaveBeenCalledWith('ws1', 'src/a.rs'));
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/project/ProjectCodeTab.test.tsx`
Expected: FAIL — cannot resolve `./ProjectCodeTab`.

- [ ] **Step 3: Write ProjectCodeTab**

Create `crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx`:

```tsx
import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api, type FindingRecord } from '../../lib/api';
import FileTree from '../code/FileTree';
import CodeViewer from '../code/CodeViewer';

export interface ProjectCodeTabProps {
  wsId: string;
}

export default function ProjectCodeTab({ wsId }: ProjectCodeTabProps) {
  const [params, setParams] = useSearchParams();
  const deepPath = params.get('path');
  const deepLine = params.get('line');
  const [selected, setSelected] = useState<string | null>(deepPath);
  const [findings, setFindings] = useState<FindingRecord[]>([]);

  // Findings for the whole project (badges + inline cards).
  useEffect(() => {
    let live = true;
    api
      .getFindings({ wsId })
      .then((r) => live && setFindings(r.findings as unknown as FindingRecord[]))
      .catch(() => live && setFindings([]));
    return () => {
      live = false;
    };
  }, [wsId]);

  // Keep the selection in sync with the URL deep-link.
  useEffect(() => {
    if (deepPath) setSelected(deepPath);
  }, [deepPath]);

  const initialLine = useMemo(() => (deepLine ? Number(deepLine) : undefined), [deepLine]);

  const onSelect = (path: string) => {
    setSelected(path);
    // Reflect selection in the URL (drop the line anchor on manual browse).
    setParams({ path }, { replace: true });
  };

  return (
    <div className="grid grid-cols-[minmax(200px,280px)_1fr] gap-3 max-md:grid-cols-1">
      <aside className="max-h-[70vh] overflow-y-auto rounded-md border border-border bg-surface">
        <FileTree wsId={wsId} findings={findings} selectedPath={selected} onSelect={onSelect} />
      </aside>
      <section className="min-w-0">
        {selected ? (
          <CodeViewer
            wsId={wsId}
            path={selected}
            findings={findings}
            initialLine={selected === deepPath ? initialLine : undefined}
          />
        ) : (
          <div className="flex h-40 items-center justify-center text-sm text-ink-dim">
            Select a file to view its source and findings.
          </div>
        )}
      </section>
    </div>
  );
}
```

- [ ] **Step 4: Wire the tab into ProjectDetail**

In `crates/rupu-cp/web/src/pages/ProjectDetail.tsx`:

Add `'code'` to the tab union (line ~35):
```ts
export type ProjectTab = 'overview' | 'runs' | 'findings' | 'code' | 'sessions' | 'coverage' | 'config';
```

Import the icon and the tab body near the other imports:
```ts
import { Code2 } from 'lucide-react';
import ProjectCodeTab from '../components/project/ProjectCodeTab';
```

Add a `<TabButton>` in the `<TabBar>` (place it after Findings, before Sessions):
```tsx
          <TabButton
            active={tab === 'code'}
            onClick={() => navigate(`/projects/${encodedId}/code`)}
            icon={Code2}
            label="Code"
          />
```

Add the body render alongside the other `{tab === ... && ...}` lines:
```tsx
        {tab === 'code' && <ProjectCodeTab wsId={p.ws_id} />}
```

- [ ] **Step 5: Add the route (before the `:wsId` wildcard)**

In `crates/rupu-cp/web/src/App.tsx`, add **above** the bare `/projects/:wsId` route line:
```tsx
            <Route path="/projects/:wsId/code" element={<Suspense fallback={<PageFallback />}><ProjectDetail tab="code" /></Suspense>} />
```

- [ ] **Step 6: Run the tab test + typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/project/ProjectCodeTab.test.tsx && npx tsc --noEmit`
Expected: PASS (2 tests); type-clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx crates/rupu-cp/web/src/components/project/ProjectCodeTab.test.tsx crates/rupu-cp/web/src/pages/ProjectDetail.tsx crates/rupu-cp/web/src/App.tsx
git commit -m "feat(web): Code tab shell — two-pane navigator + viewer

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Phase 2 operator gate:** matt browses the Code tab on a real project, expands folders, clicks files, sees findings as inline cards — in **light and dark**.

---

## Phase 3 — Finding-first deep-linking

### Task 9: Deep-link the findings lists into the Code tab

**Files:**
- Modify: `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` (make `file:line` a link into the Code tab)
- Test: `crates/rupu-cp/web/src/components/findings/FindingRow.deeplink.test.tsx` (new)

**Interfaces:**
- Consumes: `useNavigate` from react-router-dom; `FindingRecord`/`FindingOut` shape — the row already receives `finding` with `file_path` + `line_range`; the ws id is available on `FindingOut` as `ws_id` (the Findings page passes it; if `FindingRow` currently lacks `ws_id`, add an optional `wsId?: string` prop and thread it from the parent list).
- Produces: clicking the location navigates to `/projects/<wsId>/code?path=<file_path>&line=<line_range[0]>`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/components/findings/FindingRow.deeplink.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import FindingRow from './FindingRow';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [17, 19],
  severity: 'high',
  summary: 's',
  evidence: { rationale: '', references: [] },
} as unknown as FindingRecord;

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

describe('FindingRow deep-link', () => {
  it('navigates to the Code tab at the finding file:line', () => {
    render(
      <MemoryRouter initialEntries={['/findings']}>
        <FindingRow finding={FINDING} wsId="ws1" />
        <LocationProbe />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: /src\/billing\.rs/ }));
    expect(screen.getByTestId('loc')).toHaveTextContent(
      '/projects/ws1/code?path=src%2Fbilling.rs&line=17',
    );
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/FindingRow.deeplink.test.tsx`
Expected: FAIL — the location text is unchanged (currently a plain `<span>`, not a navigating button), or `wsId` prop is unknown.

- [ ] **Step 3: Make the location a deep-link**

In `crates/rupu-cp/web/src/components/findings/FindingRow.tsx`:

Add the router hook import:
```ts
import { useNavigate } from 'react-router-dom';
```

Add an optional `wsId` to the props interface (props are around lines 13-20):
```ts
  wsId?: string;
```

In the component body add:
```ts
  const navigate = useNavigate();
```

Replace the plain location span (around lines 70-71, `{location && <span className="font-mono break-all">{location}</span>}`) with a button that deep-links when `wsId`, `file_path`, and `line_range` are all present; otherwise keep the plain span:
```tsx
        {location &&
          (finding.file_path && finding.line_range && wsId ? (
            <button
              type="button"
              onClick={() =>
                navigate(
                  `/projects/${encodeURIComponent(wsId)}/code?path=${encodeURIComponent(finding.file_path!)}&line=${finding.line_range![0]}`,
                )
              }
              className="font-mono break-all text-brand-700 hover:underline"
            >
              {location}
            </button>
          ) : (
            <span className="font-mono break-all">{location}</span>
          ))}
```

- [ ] **Step 4: Thread `wsId` from the parents**

Wherever `FindingRow` is rendered (Findings page, `ProjectFindingsTab`, CoverageDetail), pass `wsId={finding.ws_id}` (each `FindingOut` carries `ws_id`). Grep for `<FindingRow` and add the prop. In `ProjectFindingsTab` the ws id is already in scope (`wsId`); pass that.

Run: `cd crates/rupu-cp/web && grep -rn "<FindingRow" src/`
Then add `wsId={...}` to each call site.

- [ ] **Step 5: Run the test + typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/FindingRow.deeplink.test.tsx && npx tsc --noEmit`
Expected: PASS; type-clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/components/findings/
git commit -m "feat(web): deep-link findings file:line into the Code tab

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Phase 3 operator gate:** from the Findings page and a project's Findings tab, matt clicks a `file:line` and lands on the Code tab with that file open, scrolled to the line, the inline card in place.

---

## Phase 4 — SCM deep-links (permalink builder)

### Task 10: Git-remote-URL parser in rupu-scm

**Files:**
- Create: `crates/rupu-scm/src/weburl.rs`
- Modify: `crates/rupu-scm/src/lib.rs` (add `pub mod weburl;`)
- Test: inline `#[cfg(test)] mod tests` in `weburl.rs`

**Interfaces:**
- Consumes: `crate::platform::Platform` (`Github`/`Gitlab`), `crate::types::RepoRef { platform, owner, repo }`.
- Produces: `pub struct RepoWeb { pub platform: Platform, pub host: String, pub owner: String, pub repo: String }` and `pub fn parse_repo_remote(remote: &str) -> Option<RepoWeb>` — handles `git@github.com:owner/repo.git`, `https://github.com/owner/repo.git`, `ssh://git@gitlab.com/group/sub/repo.git`; strips a trailing `.git`; maps host `github.com`→Github, `gitlab.com`→Gitlab; unknown host → `None`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-scm/src/weburl.rs`:

```rust
//! Pure git-remote-URL parsing and web permalink construction. No IO, no
//! async. Turns a `Workspace.repo_remote` (raw `git remote get-url origin`
//! output) plus a branch + file + line range into a github/gitlab blob URL.

use crate::platform::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWeb {
    pub platform: Platform,
    pub host: String,
    pub owner: String,
    pub repo: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scp_style_github() {
        let r = parse_repo_remote("git@github.com:section9labs/rupu.git").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.host, "github.com");
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parses_https_github_without_dot_git() {
        let r = parse_repo_remote("https://github.com/section9labs/rupu").unwrap();
        assert_eq!(r.platform, Platform::Github);
        assert_eq!(r.owner, "section9labs");
        assert_eq!(r.repo, "rupu");
    }

    #[test]
    fn parses_gitlab_nested_group() {
        let r = parse_repo_remote("https://gitlab.com/group/sub/proj.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "group/sub");
        assert_eq!(r.repo, "proj");
    }

    #[test]
    fn parses_ssh_scheme() {
        let r = parse_repo_remote("ssh://git@gitlab.com/group/proj.git").unwrap();
        assert_eq!(r.platform, Platform::Gitlab);
        assert_eq!(r.owner, "group");
        assert_eq!(r.repo, "proj");
    }

    #[test]
    fn unknown_host_is_none() {
        assert!(parse_repo_remote("git@bitbucket.org:x/y.git").is_none());
        assert!(parse_repo_remote("not a url").is_none());
        assert!(parse_repo_remote("").is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-scm --lib weburl -- --nocapture`
Expected: FAIL — `cannot find function parse_repo_remote`.

- [ ] **Step 3: Write the parser**

Add above the test module in `weburl.rs`:

```rust
fn platform_for_host(host: &str) -> Option<Platform> {
    match host {
        "github.com" => Some(Platform::Github),
        "gitlab.com" => Some(Platform::Gitlab),
        _ => None,
    }
}

/// Parse a raw git remote URL into a `RepoWeb`. Supports scp-style
/// (`git@host:owner/repo.git`), `https://host/owner/repo(.git)`, and
/// `ssh://git@host/owner/repo.git`. Owner may contain `/` (GitLab groups);
/// the last path segment is the repo. Unknown hosts → `None`.
pub fn parse_repo_remote(remote: &str) -> Option<RepoWeb> {
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }

    // Split into (host, path) across the three URL shapes.
    let (host, path) = if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
        .or_else(|| remote.strip_prefix("ssh://"))
    {
        // scheme URL: [user@]host/owner/.../repo
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        (host.to_string(), path.to_string())
    } else if let Some(rest) = remote.strip_prefix("git@") {
        // scp style: git@host:owner/.../repo
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    } else {
        return None;
    };

    let platform = platform_for_host(&host)?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.rsplit_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(RepoWeb {
        platform,
        host,
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}
```

Register the module in `crates/rupu-scm/src/lib.rs` (alongside the other `pub mod` lines):
```rust
pub mod weburl;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-scm --lib weburl -- --nocapture`
Expected: PASS (5 tests).

- [ ] **Step 5: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-scm/src/weburl.rs
git add crates/rupu-scm/src/weburl.rs crates/rupu-scm/src/lib.rs
git commit -m "feat(scm): parse git remote URLs into platform/owner/repo

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Blob + home permalink builder

**Files:**
- Modify: `crates/rupu-scm/src/weburl.rs` (add `home_url`, `blob_url`)
- Test: inline tests in `weburl.rs`

**Interfaces:**
- Produces: `impl RepoWeb { pub fn home_url(&self) -> String; pub fn blob_url(&self, branch: &str, path: &str, line_range: Option<[u32; 2]>) -> String }`. Plus a top-level convenience `pub fn repo_permalink(remote: &str, branch: Option<&str>, path: &str, line_range: Option<[u32; 2]>) -> Option<String>` used by rupu-cp.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `weburl.rs`:

```rust
    #[test]
    fn github_blob_url_with_range() {
        let r = parse_repo_remote("git@github.com:o/r.git").unwrap();
        assert_eq!(
            r.blob_url("main", "src/a.rs", Some([17, 19])),
            "https://github.com/o/r/blob/main/src/a.rs#L17-L19"
        );
    }

    #[test]
    fn github_blob_url_single_line() {
        let r = parse_repo_remote("git@github.com:o/r.git").unwrap();
        assert_eq!(
            r.blob_url("main", "src/a.rs", Some([17, 17])),
            "https://github.com/o/r/blob/main/src/a.rs#L17"
        );
    }

    #[test]
    fn gitlab_blob_url_uses_dash_blob_and_dash_range() {
        let r = parse_repo_remote("https://gitlab.com/g/s/p.git").unwrap();
        assert_eq!(
            r.blob_url("dev", "a/b.rs", Some([3, 8])),
            "https://gitlab.com/g/s/p/-/blob/dev/a/b.rs#L3-8"
        );
    }

    #[test]
    fn home_urls() {
        assert_eq!(
            parse_repo_remote("git@github.com:o/r.git").unwrap().home_url(),
            "https://github.com/o/r"
        );
        assert_eq!(
            parse_repo_remote("https://gitlab.com/g/p.git").unwrap().home_url(),
            "https://gitlab.com/g/p"
        );
    }

    #[test]
    fn convenience_permalink_defaults_branch_to_main_and_none_on_unknown() {
        assert_eq!(
            repo_permalink("git@github.com:o/r.git", Some("dev"), "x.rs", Some([1, 1])),
            Some("https://github.com/o/r/blob/dev/x.rs#L1".to_string())
        );
        assert_eq!(
            repo_permalink("git@github.com:o/r.git", None, "x.rs", None),
            Some("https://github.com/o/r/blob/main/x.rs".to_string())
        );
        assert_eq!(repo_permalink("bad", Some("main"), "x.rs", None), None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-scm --lib weburl -- --nocapture`
Expected: FAIL — no method `blob_url` / no fn `repo_permalink`.

- [ ] **Step 3: Write the builders**

Add to `weburl.rs` (after the `RepoWeb` struct / `parse_repo_remote`):

```rust
impl RepoWeb {
    /// Repository landing page.
    pub fn home_url(&self) -> String {
        match self.platform {
            Platform::Github => format!("https://{}/{}/{}", self.host, self.owner, self.repo),
            Platform::Gitlab => format!("https://{}/{}/{}", self.host, self.owner, self.repo),
        }
    }

    /// Web blob URL to a file (optionally a line range). Platform-specific
    /// path prefix and line-fragment syntax:
    ///   GitHub: `/blob/<branch>/<path>#L<a>-L<b>`
    ///   GitLab: `/-/blob/<branch>/<path>#L<a>-<b>`
    pub fn blob_url(&self, branch: &str, path: &str, line_range: Option<[u32; 2]>) -> String {
        let base = match self.platform {
            Platform::Github => format!(
                "https://{}/{}/{}/blob/{}/{}",
                self.host, self.owner, self.repo, branch, path
            ),
            Platform::Gitlab => format!(
                "https://{}/{}/{}/-/blob/{}/{}",
                self.host, self.owner, self.repo, branch, path
            ),
        };
        match line_range {
            None => base,
            Some([a, b]) if a == b => format!("{base}#L{a}"),
            Some([a, b]) => match self.platform {
                Platform::Github => format!("{base}#L{a}-L{b}"),
                Platform::Gitlab => format!("{base}#L{a}-{b}"),
            },
        }
    }
}

/// Convenience: parse a remote and build a blob permalink in one call.
/// `branch` defaults to `"main"` when `None`. Returns `None` for unknown hosts.
pub fn repo_permalink(
    remote: &str,
    branch: Option<&str>,
    path: &str,
    line_range: Option<[u32; 2]>,
) -> Option<String> {
    let rw = parse_repo_remote(remote)?;
    Some(rw.blob_url(branch.unwrap_or("main"), path, line_range))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-scm --lib weburl -- --nocapture`
Expected: PASS (10 tests total in the module).

- [ ] **Step 5: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-scm/src/weburl.rs
git add crates/rupu-scm/src/weburl.rs
git commit -m "feat(scm): github/gitlab blob + home permalink builders

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: Surface permalinks — findings + Code tab header

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs` (add `permalink: Option<String>` to `FindingOut`, compute via `rupu_scm::weburl::repo_permalink`)
- Modify: `crates/rupu-cp/src/api/projects.rs` (add `repo_home_url: Option<String>` to `ProjectDetail`)
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (add `permalink?: string` to `FindingRecord`, `repo_home_url?: string | null` to `ProjectDetail`)
- Modify: `crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx` (header chip with repo link)
- Test: inline Rust tests in `findings.rs`; the InlineFindingCard "View on repository" link already renders `finding.permalink` (Task 6) — extend its test.

**Interfaces:**
- Consumes: `rupu_scm::weburl::{parse_repo_remote, repo_permalink}` (Tasks 10-11); `Workspace.{repo_remote, initial_branch}`; existing `load_workspace`.
- Produces: each `FindingOut` gains `permalink: Option<String>`; `ProjectDetail` gains `repo_home_url: Option<String>`.

- [ ] **Step 1: Write the failing Rust test**

In `crates/rupu-cp/src/api/findings.rs`, add a test that the permalink is built for a finding whose workspace has a github remote. Add to (or create) the `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn permalink_built_from_github_remote() {
        // Given a workspace remote + branch and a finding with file+lines,
        // the FindingOut permalink is the github blob URL.
        let url = rupu_scm::weburl::repo_permalink(
            "git@github.com:o/r.git",
            Some("main"),
            "src/a.rs",
            Some([17, 19]),
        );
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/o/r/blob/main/src/a.rs#L17-L19")
        );
    }
```

(This locks the integration contract; the wiring below makes `FindingOut.permalink` use exactly this call.)

- [ ] **Step 2: Run to verify it fails/compiles**

Run: `cargo test -p rupu-cp --lib api::findings -- --nocapture`
Expected: FAIL to compile if `rupu-scm` isn't a dependency of `rupu-cp` — check `crates/rupu-cp/Cargo.toml`. If `rupu-scm` is not listed, add `rupu-scm = { workspace = true }` (workspace dep only; the version lives in root `Cargo.toml`). Then the test should compile and PASS (it only calls the pure fn); proceed to wire the DTO.

- [ ] **Step 3: Add `permalink` to `FindingOut` and compute it**

In `crates/rupu-cp/src/api/findings.rs`:

Add the field to the DTO (after `workflow_name`):
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
```

Where `FindingOut` values are built (in `list_findings`, per workspace), compute the permalink from the workspace's remote + branch. The handler already iterates workspaces via `collect_all_findings`; load each finding's workspace once (cache by `ws_id`) to get `repo_remote`/`initial_branch`, then:
```rust
let permalink = match (ws.repo_remote.as_deref(), rec.file_path.as_deref()) {
    (Some(remote), Some(path)) => rupu_scm::weburl::repo_permalink(
        remote,
        ws.initial_branch.as_deref(),
        path,
        rec.line_range,
    ),
    _ => None,
};
```
Set `permalink` on each constructed `FindingOut`. (If `collect_all_findings` doesn't currently hand back the owning `Workspace`, load it via the existing `WorkspaceStore` by `ws_id` — reuse `code::load_workspace` or the local `store(&s)` helper, memoizing in a `HashMap<String, Workspace>` to avoid re-reading the TOML per finding.)

- [ ] **Step 4: Add `repo_home_url` to `ProjectDetail`**

In `crates/rupu-cp/src/api/projects.rs`, add to the `ProjectDetail` struct:
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_home_url: Option<String>,
```
Populate it where `ProjectDetail` is built:
```rust
    repo_home_url: w
        .repo_remote
        .as_deref()
        .and_then(rupu_scm::weburl::parse_repo_remote)
        .map(|r| r.home_url()),
```

- [ ] **Step 5: Run the Rust tests**

Run: `cargo test -p rupu-cp --lib api::findings api::projects -- --nocapture && cargo build -p rupu-cp`
Expected: PASS; clean build.

- [ ] **Step 6: Extend the frontend types + render the links**

In `crates/rupu-cp/web/src/lib/api.ts`:
- Add `permalink?: string;` to the `FindingRecord` interface.
- Add `repo_home_url?: string | null;` to the `ProjectDetail` interface.

`InlineFindingCard` already renders `finding.permalink` (Task 6) — update its label from the generic "View on repository ↗" to platform-aware text if desired (optional; keep generic to avoid re-parsing host on the client).

Add the repo chip to the Code tab header. In `ProjectCodeTab.tsx`, fetch the project detail (or accept `repoHomeUrl` as a prop from `ProjectDetail.tsx`, which already loads `p`). Simplest: pass it down. In `ProjectDetail.tsx` change the body render to:
```tsx
        {tab === 'code' && <ProjectCodeTab wsId={p.ws_id} repoHomeUrl={p.repo_home_url ?? null} repoRemote={p.repo_remote ?? null} branch={p.branch ?? null} />}
```
And extend `ProjectCodeTabProps`:
```ts
export interface ProjectCodeTabProps {
  wsId: string;
  repoHomeUrl?: string | null;
  repoRemote?: string | null;
  branch?: string | null;
}
```
Render a header above the two-pane grid:
```tsx
      {(repoHomeUrl || repoRemote) && (
        <div className="mb-2 flex items-center gap-2 text-[12px] text-ink-dim">
          <span className="rounded bg-surface px-2 py-0.5 font-mono">
            {repoRemote?.replace(/^.*[:/]([^/]+\/[^/]+?)(?:\.git)?$/, '$1') ?? 'repo'}
            {branch ? ` · ${branch}` : ''}
          </span>
          {repoHomeUrl && (
            <a href={repoHomeUrl} target="_blank" rel="noreferrer" className="text-brand-700 hover:underline">
              View on repository ↗
            </a>
          )}
        </div>
      )}
```

- [ ] **Step 7: Update the InlineFindingCard test for the permalink link**

Add to `InlineFindingCard.test.tsx`:
```tsx
  it('renders a repository permalink when present', () => {
    const f = { ...FINDING, permalink: 'https://github.com/o/r/blob/main/src/billing.rs#L17' } as unknown as FindingRecord;
    view(<InlineFindingCard finding={f} stale={false} />);
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    const link = screen.getByRole('link', { name: /view on repository/i });
    expect(link).toHaveAttribute('href', 'https://github.com/o/r/blob/main/src/billing.rs#L17');
  });
```

- [ ] **Step 8: Run frontend tests + typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/ src/components/project/ && npx tsc --noEmit`
Expected: PASS; type-clean.

- [ ] **Step 9: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/findings.rs crates/rupu-cp/src/api/projects.rs
git add crates/rupu-cp/src/api/findings.rs crates/rupu-cp/src/api/projects.rs crates/rupu-cp/Cargo.toml crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/components/code/InlineFindingCard.tsx crates/rupu-cp/web/src/components/code/InlineFindingCard.test.tsx crates/rupu-cp/web/src/components/project/ProjectCodeTab.tsx crates/rupu-cp/web/src/pages/ProjectDetail.tsx
git commit -m "feat: SCM permalinks on findings + Code tab header (github/gitlab)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Phase 4 operator gate:** matt confirms "View on repository" opens the correct github/gitlab blob URL at the right line, on a finding card and the tab header, for both a github and (if available) a gitlab project.

---

## Final integration & handoff

- [ ] **Full backend test sweep:** `cargo test -p rupu-cp -p rupu-scm` — all green.
- [ ] **Full frontend sweep:** `cd crates/rupu-cp/web && npx vitest run && npx tsc --noEmit && npm run build` — all green, production build succeeds (this bundles the embedded UI).
- [ ] **Rebuild embedded UI before any release:** per project memory, `make cp-web` regenerates `web/dist` that the binary embeds; a release without it ships a stale UI. (Not needed for PR review, only for cutting a build.)
- [ ] **Operator runtime validation (REQUIRED before merge):** matt runs `rupu cp serve`, opens a project's **Code** tab, and in **both light and dark** verifies: file tree lazy-expands with badges + folder rollup; a file opens with syntax highlighting; findings render as severity-banded lines with squiggle + inline PR-style cards that expand/collapse and stack; the drift note appears on a deliberately edited file; finding-first deep-links land on the right file:line; "View on repository" links resolve correctly.
- [ ] **Open the PR** (`gh pr create --draft` over HTTPS; SSH remote is down) with a summary of the four phases and the validation checklist.

---

## Self-Review notes (author, against the spec)

- **§3 source-of-truth (project-current + drift):** Task 2 serves `Workspace.path` live; Task 5 + Task 6 compute/show drift client-side from `code_excerpt`. ✔
- **§5.1 tree / §5.2 whole-file / path-safety:** Tasks 1-2, both through `resolve_under_workspace`; malicious-path tests included. ✔
- **§5.3 drift (client, no endpoint):** Task 5 pure fn, unit-tested. ✔
- **§5.4 permalink builder (github vs gitlab, ssh vs https, unknown→None):** Tasks 10-11 in `rupu-scm`, unit-tested; Task 12 wires it. Branch-relative via `initial_branch`; `commit_sha` persistence explicitly deferred (spec Non-Goal). ✔
- **§6.1 CodeViewer (dark theme, severity band, squiggle, PR-style stacking cards):** Tasks 4 + 6. ✔
- **§6.2 FileTree (lazy, badges, folder rollup):** Task 7. ✔
- **§6.3 Code tab shell (two-pane, responsive, repo chip):** Tasks 8 + 12. ✔
- **§6.4 finding-first wiring (ws_id + file_path, no runId):** Task 9. ✔
- **§7 polish / frontend-design skill:** called out in Task 6 (and applies to Tasks 7-8 UI). ✔
- **§9 read-only, tokens-only, both themes, path-safety tests, runtime validation:** Global Constraints + operator gates. ✔
- **Open risk flagged:** `collect_all_findings` may not currently expose the owning `Workspace` to `list_findings`; Task 12 Step 3 notes the fallback (load by `ws_id` via `WorkspaceStore`, memoized). Confirm the exact shape during Task 12 and adjust the loop without changing the DTO contract.
