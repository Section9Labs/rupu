# Findings on Code — Project Code Tab, Viewer with Inline Findings, File Navigator

**Date:** 2026-07-21
**Status:** Design
**Scope:** `crates/rupu-cp` (Rust API + `web/` SPA), reusing the source-preview/highlight infra from #479/#480/#483.

## 1. Problem

rupu produces security findings that already carry file, line range, a plain-English explanation, and the offending snippet — but the UI shows them as flat table rows with a non-clickable `file:line`. You cannot see the vulnerable code in context, cannot browse the project's files, and cannot jump to the file/line in GitHub/GitLab. The reference product (aikido code-audit) shows each finding *on the actual source* — the vulnerable line highlighted, the finding as an inline PR-style comment ("Line 17 checks orgId but never userId → any tenant can read another's billing"), with links to the repo — and a file navigator where findings appear as comments while you browse. This design brings that to rupu-cp.

## 2. Goals / Non-Goals

**Goals**
- Show a finding **on its source code**: the file rendered with syntax highlighting, the finding's `line_range` highlighted, and the finding as an expandable inline comment (severity, summary, rationale, CWE, repo link).
- A **project "Code" tab**: a two-pane file navigator (tree + viewer) where you browse the project's live files and see findings overlaid as inline comments, with finding badges in the tree.
- **One viewer, reached two ways**: finding-first (click a finding → viewer at that file:line) and file-first (browse the tree → open a file). No duplicate viewers.
- **SCM deep-links**: "View on GitHub/GitLab" from `repo_remote` + branch, on findings and the tab header.
- **Honest about staleness**: when the code changed since a finding was made, say so (drift detection via the finding's stored `code_excerpt`).
- **High visual polish** matching the aikido design language (see §7).

**Non-Goals**
- No autofix / suggested-change blocks, attack-chain narratives, refinement-funnel metrics, or agentic-scan timelines (the adjacent aikido features). Future, out of scope.
- No editing files in CP (read-only, per rupu-cp's contract).
- No commit-stable permalinks in the first pass (branch-relative first; `commit_sha` persistence is an optional follow-up — §5.4).
- No run-scoped file navigator. The navigator is project-level; runs deep-link into it.
- No new frontend dependencies; no full-tree eager fetch (lazy per-directory).

## 3. Key decision: what code do we show (source of truth)

**Project-current files, with drift detection** (chosen over run-snapshot and hybrid).

- The browsable/viewable source is the project's **live working directory** (`Workspace.path`). For the common case — a local run executing in-place in the project — this is exactly the code the run saw, so a finding's `line_range` is accurate. It also gives a real file tree for the navigator, which a per-run snapshot cannot.
- When the code changed since a finding was made (edited, or the run used a throwaway clone), the finding's line could drift. We **detect** this: each `FindingRecord` stores `evidence.code_excerpt`; the backend compares it against the current file's `line_range` content and returns a `stale` flag. The viewer still points at the recorded line but shows a quiet "code may have changed since this finding" note.
- The existing run-scoped `/api/runs/:id/source` + `SourcePreview` stay as-is for the transcript context (looking at one run). The findings/navigator surfaces use the new **project-scoped** source (§5).

## 4. Architecture

```
Project page → "Code" tab
├─ left pane:  FileTree  (project live files, lazy per-dir, finding badges)
└─ right pane: CodeViewer (whole file, highlighted, inline finding cards)
                    ▲
   finding-first ───┘   Findings / Coverage / ProjectFindings lists deep-link here
   file-first: click a tree node → open in the same CodeViewer

Backend (rupu-cp, read-only):
  GET /api/projects/:ws_id/tree?path=      → lazy directory children
  GET /api/projects/:ws_id/source?path=    → whole file content (+ drift per finding)
  (findings already: GET /api/findings?ws_id= → FindingOut[] keyed by file_path)
  SCM permalink builder: repo_remote + branch + file + line_range → github/gitlab URL
```

**Reuse (confirmed present):** `FindingRecord` (file_path, line_range [start,end] 1-based inclusive, summary, `evidence.rationale`, `evidence.code_excerpt`, severity, concern_id, references); `CodeHighlight` (highlight.js, per-line inline mode); `SourcePreview`/`GET /api/runs/:id/source` (line-numbered slice, single-line highlight, `resolve_under_workspace` path-safety); `FindingCard`'s clickable-file:line→preview pattern; `Workspace.{path, repo_remote, initial_branch}` (surfaced on `ProjectRow`); `rupu-scm` `RepoRef`/`Platform` parsing.

**Build (the gaps):** project-scoped tree + whole-file endpoints; drift detection; dark-theme highlighting (current is light-only); a multi-line highlight band + inline-comment interleaving in the viewer; the FileTree component with badges; the SCM permalink builder (none exists in `rupu-scm`); wiring the standalone findings lists (they don't thread `runId` today, but `declared_by.run_id` is available — and with project-scoped source we key off `ws_id` + `file_path`, not `runId`).

## 5. Backend (rupu-cp)

### 5.1 Project file tree — `GET /api/projects/:ws_id/tree?path=<rel>`
Returns the immediate children of a workspace-relative directory (default: root). Lazy per-directory (a folder's children are fetched when expanded), so large projects don't force a full-tree fetch.

```rust
struct TreeEntry { name: String, path: String /* workspace-relative */, kind: "dir" | "file" }
struct TreeResult { path: String, parent: Option<String>, entries: Vec<TreeEntry> }
```
- Resolve `ws_id` → `Workspace.path`; join `path`; **reuse `source.rs`'s `resolve_under_workspace`** to reject `..`, absolute paths, and symlink escapes (canonicalize + `starts_with`). The `fs.rs` browse endpoint is NOT reused (dirs-only, unconfined, absolute-path picker).
- Sort dirs-first then name; hide `.git` and other noise dirs (configurable small denylist); include dotfiles otherwise.
- Paths are **workspace-relative**, matching `FindingRecord.file_path`, so the tree and findings align.

### 5.2 Project file content — `GET /api/projects/:ws_id/source?path=<rel>`
The project-scoped analogue of `/api/runs/:id/source`, returning the **whole file** (not a ±200 window) for the viewer.

```rust
struct FileContent {
  available: bool,
  path: Option<String>,
  language: Option<&'static str>,   // detect_language() by extension (reuse)
  total_lines: Option<usize>,
  lines: Option<Vec<SourceLine>>,   // reuse SourceLine { n, text }
  reason: Option<String>,           // when unavailable (too big, binary, not found)
}
```
- Same `resolve_under_workspace` safety. Size cap (~1–2 MiB or a max line count — reuse `source.rs`'s existing caps); non-UTF-8 / oversized → `available:false` with a reason (the viewer shows a graceful placeholder).
- Whole-file, because the navigator + inline comments need the full file, not a window. Cap protects against pathological files.

### 5.3 Drift detection — client-side, no new endpoint
Whether a finding's recorded code still matches the current file is computed **on the client**: the viewer already fetches the whole file (§5.2) and each finding already carries `evidence.code_excerpt`, so a pure function compares the excerpt against the current `line_range` slice (normalizing whitespace / trailing newlines) → `stale: bool`. No backend addition; the function is unit-tested. When `code_excerpt` is absent, drift is unknown (treat as not-stale, no note). The viewer surfaces `stale` as a quiet "code may have changed since this finding" note.

### 5.4 SCM permalink builder
Given `repo_remote` (git URL, ssh or https), `branch`, a workspace-relative `file_path`, and `line_range`, build a web URL:
- Parse `repo_remote` → `{ host: github|gitlab|other, owner, repo }` (normalize `git@github.com:owner/repo.git` and `https://github.com/owner/repo.git`). Reuse `rupu-scm` `RepoRef`/`Platform` where possible; add URL construction (none exists on `RepoConnector`).
- GitHub: `https://github.com/<owner>/<repo>/blob/<branch>/<path>#L<a>-L<b>`. GitLab: `https://gitlab.com/<owner>/<repo>/-/blob/<branch>/<path>#L<a>-<b>` (note the `-/` and hash format differ).
- **Branch-relative** using `initial_branch`. A `commit_sha` for stable permalinks is **not persisted today** — capturing `git rev-parse HEAD` at run start + threading it to findings is an optional follow-up; branch-relative ships first (links can drift if lines moved, acceptable for v1, and the drift note already warns of change).
- Location: a small pure builder (rupu-scm or rupu-cp), unit-tested (github vs gitlab, ssh vs https, missing/unknown host → no link).
- Where used: the Code tab header ("owner/repo · branch" chip + repo link) and each finding card ("View on GitHub/GitLab"). If a run's `issue_ref`/PR is present, also link the PR.

## 6. Frontend (web/)

### 6.1 CodeViewer — the shared file+findings view
Extends the `SourcePreview`/`CodeHighlight` foundation:
- Renders the **whole file** with a line-number gutter and per-line syntax highlighting (`CodeHighlight` inline mode).
- **Dark theme**: `CodeHighlight` is light-only today — add a dark variant (theme-aware via `useThemeColors()` / `--c-*`), since the app is themed and the reference is dark. Both themes must read correctly.
- For each finding overlapping the visible file, draw a **severity-colored left border + line-band highlight** over its `line_range`, plus a squiggle underline on the anchor line — both cues from the reference images.
- **Inline finding cards**, PR-style: a collapsed one-line marker sits under the finding's `line_range` (severity dot + summary); clicking expands the full card inline (rationale as markdown, CWE link, "View on GitHub/GitLab", the `stale` note when drifted), pushing code below down. Multiple findings on the same lines **stack**. Collapsing restores the code flow.
- Props roughly: `{ wsId, path, findings: FindingOut[], initialLine?, repoLink?(finding) }`. Fetches file content via the §5.2 endpoint; computes `stale` per finding client-side (§5.3).

### 6.2 FileTree — the navigator
- Lazy directory tree (expand a folder → fetch its children via §5.1). Dirs-first, syntax-appropriate file icons.
- **Finding badges**: fetch the project's findings once (`GET /api/findings?ws_id=`), index by `file_path`; a file node shows a severity dot + count; a folder node rolls up its descendants' **worst severity** (client-side aggregate over the findings' paths — no full-tree fetch needed). This is what makes the browser read as a security tool.
- Selecting a file opens it in the CodeViewer (right pane) with its findings overlaid.

### 6.3 Code tab shell
- New "Code" tab on the Project detail page (alongside Runs / Findings / Sessions / Coverage). Two-pane: FileTree left, CodeViewer right (responsive: stack on narrow).
- Header: repo/branch context chip ("owner/repo · main") + "View on GitHub/GitLab" (§5.4) when `repo_remote` is present.

### 6.4 Finding-first wiring
- The standalone `FindingRow` (Findings page, CoverageDetail, ProjectFindingsTab) makes `file:line` a **deep-link** into the project's Code tab at that `path` + `line_range[0]`, opening the CodeViewer scrolled/expanded to the finding. Uses `ws_id` + `file_path` (both on `FindingOut`) — no `runId` needed since source is project-scoped.

## 7. Design language / polish (a hard requirement, not a nice-to-have)
Match the aikido reference patterns the operator supplied:
- Clean severity-colored finding cards; severity as a colored dot/pill consistent with the app's existing `--c-sev-*` tokens.
- The vulnerable line: severity-tinted line band **and** a squiggle underline.
- Repo/branch context chips; "View PR"/"View on GitHub" affordances styled like the references.
- A genuinely crafted dark code viewer (not a bolted-on dark mode).
- **The UI build uses the `frontend-design` skill** so the result is polished, not generic. This is called out in the plan for the UI phases.

## 8. Delivery phases (stacked, each shippable)
1. **Foundational + viewer:** §5.1/§5.2 endpoints (tree + whole-file, path-safe) + the CodeViewer (§6.1) with inline findings + dark theme. Demonstrable by opening a file with findings.
2. **Code tab + navigator:** §6.2 FileTree with badges + §6.3 the two-pane tab; file-first browsing.
3. **Finding-first wiring:** §6.4 deep-link the findings lists into the viewer.
4. **SCM deep-links:** §5.4 permalink builder + "View on GitHub/GitLab" on findings + tab header; optional `commit_sha` persistence follow-up.

The viewer is front-loaded because everything depends on it (this reorders the operator's original 3.1-first, since choosing project-current + a richer viewer superseded reusing the run-scoped preview).

## 9. Constraints & Testing
- rupu-cp stays **READ-ONLY**; workspace deps only; `#![deny(clippy::all)]`; `unsafe_code` forbidden. No new npm deps; no color literals (tokens only); both themes must work.
- **Path safety is load-bearing** — every project file read/list goes through `resolve_under_workspace` (no `..`, no absolute, no symlink escape). Test with malicious paths.
- **Backend tests:** tree lists workspace-relative children and refuses escapes; whole-file endpoint returns content + caps oversized/binary; permalink builder (github vs gitlab, ssh vs https remote, unknown host → None).
- **Frontend tests:** viewer renders findings at the correct lines; marker expand/collapse; multiple findings stack; drift note shows when `code_excerpt` mismatches; FileTree lazy-loads + badges + folder rollup; finding-first deep-link lands on the right file:line.
- **Runtime validation:** per CLAUDE.md, build/test cleanliness ≠ rendering cleanliness — the operator browser-validates the viewer + navigator in light and dark before merge.
