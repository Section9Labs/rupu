# CP Coverage Subfeatures — Plan 1 (core: templates / catalog / audit / gap)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the `rupu coverage` analytical subcommands (templates, catalog, audit, gap) in the CP web UI — a tabbed target-detail page plus a global Templates page.

**Architecture:** `rupu-coverage` is a clean reusable library the CP backend already calls. New backend handlers are thin pass-throughs to existing library functions (`builtin_names`/`resolve_builtin`, `read_snapshot`, `run_audit`); `AuditReport` and `FlatCatalog` derive `Serialize` so they're returned as JSON directly. The frontend refactors `CoverageDetail` into a tabbed shell (mirroring `ProjectDetail`) and adds per-tab components + a global Templates page. Gap is a client-side view of the audit response (no separate endpoint).

**Tech Stack:** Rust + axum (backend), React 18 + TypeScript + Vite + Vitest + Tailwind (frontend), `highlight.js` already present.

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-coverage-subfeatures-design.md`
Diff (run selectors + comparison UI) is deferred to Plan 2.

## Global Constraints

- Workspace deps only; never pin versions in crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-cli`/`rupu-cp` are thin: no business logic — delegate to `rupu-coverage`.
- Errors: `thiserror` in libraries, `ApiError`/`ApiResult` in CP handlers.
- Backend tests: factor pure logic into free functions and test those (the
  pattern in `crates/rupu-cp/src/api/findings.rs` and `autoflows.rs`); do not
  spin up the axum server.
- Run per-file `rustfmt`, never package-wide `cargo fmt` (main is fmt-dirty).
- Frontend component tests use `// @vitest-environment jsdom`; pure-logic tests
  run in the default node env.
- All new frontend routes are `React.lazy`-loaded (existing convention).

## Reference: exact library types (already `Serialize`)

From `rupu_coverage` (re-exported at crate root):
- `builtin_names() -> impl Iterator<Item = &'static str>`
- `resolve_builtin(name: &str) -> Option<Result<Template, ParseError>>`
- `read_snapshot(path: &Path) -> Result<FlatCatalog, SnapshotError>`
- `run_audit(paths: &CoveragePaths) -> std::io::Result<AuditReport>`
- `CoveragePaths::new(workspace: &Path, target_id: &str) -> CoveragePaths` with field `catalog: PathBuf`.
- `Severity` serializes lowercase: `info|low|medium|high|critical`.
- `Template { name: String, version: u32, description: String, references: Vec<String>, concerns: Vec<Concern>, includes: Vec<String> }`
- `Concern { id, name, description, severity: Severity, applicable_globs: Vec<String>, min_strength: TouchStrength, references: Vec<String>, tags: Vec<String> }`
- `FlatCatalog { concerns: Vec<Concern>, sources: BTreeMap<String,String>, render_modes: BTreeMap<String,CatalogMode> }`
- `AuditReport { target_id, concerns: Vec<ConcernCoverage>, files: Vec<FileCoverage>, cross_model: Vec<CrossModelEntry>, serendipitous: Vec<SerendipitousCluster>, total_concerns, complete_concerns, total_gap_files }`
- `ConcernCoverage { concern_id, name, severity, in_scope_files: Vec<String>, asserted_files: Vec<String>, gap_files: Vec<String>, clean: u32, findings: u32, examined: u32, not_applicable: u32 }`

---

## Task 1: Templates list endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/templates` → `Vec<TemplateSummary>` where
  `TemplateSummary { name: String, version: u32, description: String, concern_count: usize, severity_breakdown: BTreeMap<String, usize> }`.
- Produces (for Task 2): free fn `builtin_template_summaries() -> Vec<TemplateSummary>`.

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/rupu-cp/src/api/coverage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_summaries_include_known_builtins_with_counts() {
        let summaries = builtin_template_summaries();
        // stride is a known bundled template; it must be present with concerns.
        let stride = summaries
            .iter()
            .find(|t| t.name == "stride")
            .expect("stride template present");
        assert!(stride.concern_count > 0, "stride should have concerns");
        // severity_breakdown counts must sum to concern_count.
        let sum: usize = stride.severity_breakdown.values().copied().sum();
        assert_eq!(sum, stride.concern_count);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib template_summaries_include_known_builtins_with_counts`
Expected: FAIL — `builtin_template_summaries` not found.

- [ ] **Step 3: Write minimal implementation**

At the top of `coverage.rs`, extend the `rupu_coverage` import and add `std::collections::BTreeMap`:

```rust
use rupu_coverage::{
    builtin_names, coverage_status, discover_targets, file_views, read_file_events, read_findings,
    resolve_builtin, CoveragePaths, CoverageStatusInput,
};
use std::collections::BTreeMap;
```

Add the DTO + free fn + handler (place after the existing `CoverageSummary` block):

```rust
#[derive(Serialize)]
struct TemplateSummary {
    name: String,
    version: u32,
    description: String,
    concern_count: usize,
    /// Lowercase severity → count, e.g. {"high": 3, "medium": 5}.
    severity_breakdown: BTreeMap<String, usize>,
}

/// Resolve every bundled concern template into a list summary. Unparseable
/// builtins are skipped with a warning (should never happen for bundled YAML).
fn builtin_template_summaries() -> Vec<TemplateSummary> {
    let mut out = Vec::new();
    for name in builtin_names() {
        let tpl = match resolve_builtin(name) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                tracing::warn!(template = name, error = %e, "skipping unparseable builtin template");
                continue;
            }
            None => continue,
        };
        let mut severity_breakdown: BTreeMap<String, usize> = BTreeMap::new();
        for c in &tpl.concerns {
            // Severity serializes lowercase; reuse that vocabulary for the map key.
            let key = serde_json::to_value(c.severity)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "medium".to_string());
            *severity_breakdown.entry(key).or_default() += 1;
        }
        out.push(TemplateSummary {
            name: tpl.name,
            version: tpl.version,
            description: tpl.description,
            concern_count: tpl.concerns.len(),
            severity_breakdown,
        });
    }
    out
}

async fn list_templates() -> Json<Vec<TemplateSummary>> {
    Json(builtin_template_summaries())
}
```

Register the route in `routes()`:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/coverage", get(list_coverage))
        .route("/api/coverage/templates", get(list_templates))
        .route("/api/coverage/:target", get(get_coverage))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib template_summaries_include_known_builtins_with_counts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage templates list endpoint"
```

---

## Task 2: Template detail endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/templates/:name` → the full `Template`
  (serialized directly; `Template` already derives `Serialize`). 404 on unknown.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `coverage.rs`:

```rust
    #[test]
    fn resolve_template_returns_known_and_rejects_unknown() {
        assert!(resolve_template_by_name("stride").is_some());
        assert!(resolve_template_by_name("does-not-exist").is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib resolve_template_returns_known_and_rejects_unknown`
Expected: FAIL — `resolve_template_by_name` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `coverage.rs`:

```rust
/// Resolve one bundled template by name. `None` for unknown names; an
/// unparseable builtin is also treated as absent (logged).
fn resolve_template_by_name(name: &str) -> Option<rupu_coverage::Template> {
    match resolve_builtin(name) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            tracing::warn!(template = name, error = %e, "unparseable builtin template");
            None
        }
        None => None,
    }
}

async fn get_template(Path(name): Path<String>) -> ApiResult<Json<rupu_coverage::Template>> {
    resolve_template_by_name(&name)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("template {name} not found")))
}
```

Register the route (after `templates`, before `:target`):

```rust
        .route("/api/coverage/templates/:name", get(get_template))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib resolve_template_returns_known_and_rejects_unknown`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage template detail endpoint"
```

---

## Task 3: Catalog endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/:target/catalog?ws_id=…` → `FlatCatalog`
  (serialized directly). 404 when target/catalog absent.
- Consumes: the workspace-scan pattern already in `get_coverage`.

- [ ] **Step 1: Write the failing test**

The catalog logic is `read_snapshot` (library-tested). Our code is the
path wiring. Add a test that a tempfile target with a written catalog snapshot
reads back through `CoveragePaths`:

```rust
    #[test]
    fn catalog_reads_back_written_snapshot() {
        use rupu_coverage::{write_snapshot, FlatCatalog};
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        let paths = CoveragePaths::new(wp, "tgt-1");
        std::fs::create_dir_all(paths.catalog.parent().unwrap()).unwrap();
        let cat = FlatCatalog {
            concerns: vec![],
            sources: Default::default(),
            render_modes: Default::default(),
        };
        write_snapshot(&cat, &paths.catalog).expect("write snapshot");

        let got = read_target_catalog(wp, "tgt-1").expect("ok").expect("some");
        assert_eq!(got.concerns.len(), 0);
        // A target with no catalog file → None.
        assert!(read_target_catalog(wp, "missing").unwrap().is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib catalog_reads_back_written_snapshot`
Expected: FAIL — `read_target_catalog` not found.

- [ ] **Step 3: Write minimal implementation**

Extend the import with `read_snapshot`:

```rust
use rupu_coverage::{
    builtin_names, coverage_status, discover_targets, file_views, read_file_events, read_findings,
    read_snapshot, resolve_builtin, CoveragePaths, CoverageStatusInput,
};
```

Add the pure helper + handler:

```rust
/// Read the effective catalog snapshot for a target under one workspace path.
/// `Ok(None)` when the catalog file is absent; `Err` only on a corrupt file.
fn read_target_catalog(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<rupu_coverage::FlatCatalog>, String> {
    let paths = CoveragePaths::new(wp, target);
    if !paths.catalog.exists() {
        return Ok(None);
    }
    read_snapshot(&paths.catalog).map(Some).map_err(|e| e.to_string())
}

async fn get_catalog(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<rupu_coverage::FlatCatalog>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match read_target_catalog(wp, &target) {
            Ok(Some(cat)) => return Ok(Json(cat)),
            Ok(None) => continue,
            Err(e) => return Err(ApiError::internal(e)),
        }
    }
    Err(ApiError::not_found(format!(
        "coverage catalog for target {target} not found"
    )))
}
```

Factor the workspace-candidate selection (used by `get_coverage` too) into a
shared helper; add it and refactor `get_coverage` to call it:

```rust
/// The workspaces to search for a target: the single named one, or all.
fn scoped_workspaces(s: &AppState, ws_id: &Option<String>) -> Vec<rupu_workspace::Workspace> {
    let workspaces = store(s).list().unwrap_or_default();
    match ws_id {
        Some(id) => workspaces.into_iter().filter(|w| &w.id == id).collect(),
        None => workspaces,
    }
}
```

(In `get_coverage`, replace the inline `candidates` construction with
`for w in scoped_workspaces(&s, &q.ws_id)` and drop the now-unused
`let workspaces = …` line.)

Register the route (deeper than `:target`, so order-independent):

```rust
        .route("/api/coverage/:target/catalog", get(get_catalog))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib catalog_reads_back_written_snapshot`
Expected: PASS. Also run `cargo test -p rupu-cp --lib` — existing coverage
tests still green after the `get_coverage` refactor.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage catalog endpoint + shared workspace scoping"
```

---

## Task 4: Audit endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/:target/audit?ws_id=…` → `AuditReport`
  (serialized directly). 404 when target not found. The Gap tab consumes this.

- [ ] **Step 1: Write the failing test**

`run_audit` is library-tested; our code resolves the target then calls it.
Test the resolver returns the discovered paths only for existing targets:

```rust
    #[test]
    fn audit_runs_for_existing_target_only() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        // A target exists once it has any ledger dir/file under .rupu/coverage.
        let paths = CoveragePaths::new(wp, "tgt-9");
        std::fs::create_dir_all(paths.root.clone()).unwrap();
        let mut f = std::fs::File::create(&paths.concerns).unwrap();
        f.write_all(b"").unwrap();

        assert!(run_target_audit(wp, "tgt-9").expect("ok").is_some());
        assert!(run_target_audit(wp, "nope").expect("ok").is_none());
    }
```

(Confirm `CoveragePaths` exposes `root` and `concerns`; the exploration noted
both. If `root` is private, create `paths.concerns.parent()` instead.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib audit_runs_for_existing_target_only`
Expected: FAIL — `run_target_audit` not found.

- [ ] **Step 3: Write minimal implementation**

Extend the import with `run_audit`:

```rust
use rupu_coverage::{
    builtin_names, coverage_status, discover_targets, file_views, read_file_events, read_findings,
    read_snapshot, resolve_builtin, run_audit, CoveragePaths, CoverageStatusInput,
};
```

Add helper + handler:

```rust
/// Run the audit for a target under one workspace path. `Ok(None)` when the
/// target isn't present under this workspace.
fn run_target_audit(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<rupu_coverage::AuditReport>, String> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    run_audit(&paths).map(Some).map_err(|e| e.to_string())
}

async fn get_audit(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<rupu_coverage::AuditReport>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match run_target_audit(wp, &target) {
            Ok(Some(report)) => return Ok(Json(report)),
            Ok(None) => continue,
            Err(e) => return Err(ApiError::internal(e)),
        }
    }
    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}
```

Register the route:

```rust
        .route("/api/coverage/:target/audit", get(get_audit))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib audit_runs_for_existing_target_only`
Expected: PASS.

- [ ] **Step 5: Verify the whole backend is clean**

Run: `cargo test -p rupu-cp --lib` → all pass.
Run: `cargo clippy -p rupu-cp --all-targets` → no warnings.
Run: `rustfmt --edition 2021 --check crates/rupu-cp/src/api/coverage.rs` → exit 0
(if it reports diffs, run without `--check` to format just this file).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage audit endpoint"
```

---

## Task 5: Frontend api types + methods

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`

**Interfaces:**
- Produces (consumed by Tasks 6–10): the types and methods below.

- [ ] **Step 1: Add types** (place near the existing coverage block, ~line 643)

```ts
// ── Coverage: concerns / catalog / audit / templates ──────────────────────

export interface CoverageConcern {
  id: string;
  name: string;
  description: string;
  severity: string; // lowercase: info|low|medium|high|critical
  applicable_globs: string[];
  min_strength: string;
  references: string[];
  tags: string[];
}

export interface TemplateSummary {
  name: string;
  version: number;
  description: string;
  concern_count: number;
  severity_breakdown: Record<string, number>;
}

export interface TemplateDetail {
  name: string;
  version: number;
  description: string;
  references: string[];
  concerns: CoverageConcern[];
  includes: string[];
}

export interface FlatCatalog {
  concerns: CoverageConcern[];
  sources: Record<string, string>;     // concern_id → template name / "inline"
  render_modes: Record<string, string>; // concern_id → full|index|auto
}

export interface ConcernCoverage {
  concern_id: string;
  name: string;
  severity: string;
  in_scope_files: string[];
  asserted_files: string[];
  gap_files: string[];
  clean: number;
  findings: number;
  examined: number;
  not_applicable: number;
}

export interface FileCoverage {
  path: string;
  strongest_touch: string;
  asserted_concerns: string[];
  missing_concerns: string[];
}

export interface CrossModelEntry {
  concern_id: string;
  file_path: string;
  model_statuses: [string, string][];
  disagreement: boolean;
}

export interface SerendipitousCluster {
  theme: string;
  finding_ids: string[];
  count: number;
}

export interface AuditReport {
  target_id: string;
  concerns: ConcernCoverage[];
  files: FileCoverage[];
  cross_model: CrossModelEntry[];
  serendipitous: SerendipitousCluster[];
  total_concerns: number;
  complete_concerns: number;
  total_gap_files: number;
}
```

- [ ] **Step 2: Add API methods** (inside the `api` object, near `getCoverageDetail`)

```ts
  getCoverageTemplates(): Promise<TemplateSummary[]> {
    return request<TemplateSummary[]>('/api/coverage/templates');
  },
  getCoverageTemplate(name: string): Promise<TemplateDetail> {
    return request<TemplateDetail>(`/api/coverage/templates/${encodeURIComponent(name)}`);
  },
  getCoverageCatalog(target: string, wsId?: string): Promise<FlatCatalog> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<FlatCatalog>(`/api/coverage/${encodeURIComponent(target)}/catalog${qs}`);
  },
  getCoverageAudit(target: string, wsId?: string): Promise<AuditReport> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<AuditReport>(`/api/coverage/${encodeURIComponent(target)}/audit${qs}`);
  },
```

- [ ] **Step 3: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts
git commit -m "feat(cp/web): api types + methods for coverage templates/catalog/audit"
```

---

## Task 6: Refactor CoverageDetail into a tabbed shell

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/CoverageDetail.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx`

**Interfaces:**
- Produces: `CoverageDetail({ tab }: { tab?: CoverageTab })` where
  `type CoverageTab = 'overview' | 'catalog' | 'audit' | 'gap'`. The shell owns
  the header + TabBar and renders the active tab; `?ws_id=` is preserved.
- Consumes: existing `api.getCoverageDetail` (header + Overview).

- [ ] **Step 1: Extract the current body into an Overview tab**

In `CoverageDetail.tsx`, the existing findings/files/assertions sections become
the Overview. Keep the existing data-fetch (`getCoverageDetail`) for the shell
header and Overview. Add the tab type and accept a `tab` prop defaulting to
`'overview'`:

```tsx
export type CoverageTab = 'overview' | 'catalog' | 'audit' | 'gap';
```

Change the default export signature to:

```tsx
export default function CoverageDetail({ tab = 'overview' }: { tab?: CoverageTab }) {
```

After the header block (target id + badges), add a TabBar. Use `useNavigate`
and preserve `ws_id`:

```tsx
const navigate = useNavigate();
const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
const enc = encodeURIComponent(target);
const tabs: { id: CoverageTab; label: string; path: string }[] = [
  { id: 'overview', label: 'Overview', path: `/coverage/${enc}${qs}` },
  { id: 'catalog',  label: 'Catalog',  path: `/coverage/${enc}/catalog${qs}` },
  { id: 'audit',    label: 'Audit',    path: `/coverage/${enc}/audit${qs}` },
  { id: 'gap',      label: 'Gap',      path: `/coverage/${enc}/gap${qs}` },
];
```

```tsx
<nav className="mt-4 flex gap-1 border-b border-border">
  {tabs.map((t) => (
    <button
      key={t.id}
      onClick={() => navigate(t.path)}
      className={cn(
        'px-3 py-1.5 text-sm font-medium border-b-2 -mb-px',
        tab === t.id
          ? 'border-brand-500 text-ink'
          : 'border-transparent text-ink-dim hover:text-ink',
      )}
    >
      {t.label}
    </button>
  ))}
</nav>
```

Wrap the existing findings/files/assertions sections so they render only for the
overview tab, and mount the other tabs:

```tsx
{tab === 'overview' && (
  <>
    {/* existing Findings / Files touched / Assessed concerns sections */}
  </>
)}
{tab === 'catalog' && <CoverageCatalogTab target={target} wsId={wsId} />}
{tab === 'audit' && <CoverageAuditTab target={target} wsId={wsId} />}
{tab === 'gap' && <CoverageGapTab target={target} wsId={wsId} />}
```

Add imports (the tab components land in Tasks 7–9; create empty stubs now so
this compiles — each: `export default function X(_: {target:string; wsId?:string}) { return null; }`):

```tsx
import CoverageCatalogTab from '../components/coverage/CoverageCatalogTab';
import CoverageAuditTab from '../components/coverage/CoverageAuditTab';
import CoverageGapTab from '../components/coverage/CoverageGapTab';
```

Ensure `useNavigate` and `cn` are imported.

- [ ] **Step 2: Create the three stub tab files**

```bash
mkdir -p crates/rupu-cp/web/src/components/coverage
```

Create each of `CoverageCatalogTab.tsx`, `CoverageAuditTab.tsx`,
`CoverageGapTab.tsx` with:

```tsx
export default function Stub(_props: { target: string; wsId?: string }) {
  return null;
}
```

- [ ] **Step 3: Add routes in App.tsx**

Add lazy import alongside the existing `CoverageDetail`:

```tsx
// (CoverageDetail is already lazily imported)
```

Add the tab routes BEFORE the existing `/coverage/:target` route (static
segments first), each passing the matching `tab`:

```tsx
<Route path="/coverage/:target/catalog" element={<Suspense fallback={<PageFallback />}><CoverageDetail tab="catalog" /></Suspense>} />
<Route path="/coverage/:target/audit" element={<Suspense fallback={<PageFallback />}><CoverageDetail tab="audit" /></Suspense>} />
<Route path="/coverage/:target/gap" element={<Suspense fallback={<PageFallback />}><CoverageDetail tab="gap" /></Suspense>} />
<Route path="/coverage/:target" element={<Suspense fallback={<PageFallback />}><CoverageDetail /></Suspense>} />
```

- [ ] **Step 4: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success. Manually confirm the existing Overview content still renders
under `tab === 'overview'`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/CoverageDetail.tsx crates/rupu-cp/web/src/App.tsx crates/rupu-cp/web/src/components/coverage/
git commit -m "feat(cp/web): tabbed shell for coverage target detail"
```

---

## Task 7: Catalog tab

**Files:**
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageCatalogTab.tsx`

**Interfaces:**
- Consumes: `api.getCoverageCatalog`, `FlatCatalog`, `CoverageConcern`,
  `SectionHeader`, `ListCard`.

- [ ] **Step 1: Implement the tab**

```tsx
// Catalog tab — the effective concern catalog snapshot for a target.
import { useEffect, useState } from 'react';
import { api, type FlatCatalog } from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageCatalogTab({ target, wsId }: { target: string; wsId?: string }) {
  const [cat, setCat] = useState<FlatCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setCat(null);
    setError(null);
    api
      .getCoverageCatalog(target, wsId)
      .then((d) => !cancelled && setCat(d))
      .catch((e: unknown) =>
        !cancelled && setError(e instanceof Error ? e.message : 'Failed to load catalog'),
      );
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!cat) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (cat.concerns.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No catalog snapshot for this target.</p>;

  return (
    <section className="mt-6">
      <SectionHeader tone="muted" label="Catalog concerns" count={cat.concerns.length} />
      <ListCard>
        {cat.concerns.map((c) => (
          <div key={c.id} className="px-4 py-3">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium text-ink">{c.name}</span>
              <span className="text-[11px] font-mono text-ink-mute">{c.id}</span>
              <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
                {c.severity}
              </span>
              <span className="text-[10px] text-ink-mute">
                {cat.sources[c.id] ?? 'inline'}
              </span>
            </div>
            {c.description && (
              <p className="mt-1 text-xs text-ink-dim leading-snug">{c.description}</p>
            )}
            <p className="mt-1 text-[11px] text-ink-mute font-mono break-all">
              {c.applicable_globs.join(', ')}
            </p>
          </div>
        ))}
      </ListCard>
    </section>
  );
}
```

- [ ] **Step 2: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageCatalogTab.tsx
git commit -m "feat(cp/web): coverage catalog tab"
```

---

## Task 8: Audit tab

**Files:**
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageAuditTab.tsx`

**Interfaces:**
- Consumes: `api.getCoverageAudit`, `AuditReport`, `ConcernCoverage`,
  `sevRank`, `normFindingSeverity`, `SectionHeader`, `ListCard`.

- [ ] **Step 1: Implement the tab**

```tsx
// Audit tab — per-concern coverage matrix + cross-model + serendipitous.
import { useEffect, useMemo, useState } from 'react';
import {
  api,
  normFindingSeverity,
  sevRank,
  type AuditReport,
  type ConcernCoverage,
} from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageAuditTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setReport(null);
    setError(null);
    api
      .getCoverageAudit(target, wsId)
      .then((d) => !cancelled && setReport(d))
      .catch((e: unknown) =>
        !cancelled && setError(e instanceof Error ? e.message : 'Failed to load audit'),
      );
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const concerns = useMemo(
    () =>
      [...(report?.concerns ?? [])].sort(
        (a, b) =>
          sevRank(normFindingSeverity(b.severity)) - sevRank(normFindingSeverity(a.severity)),
      ),
    [report],
  );

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!report) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;

  return (
    <div className="mt-6 space-y-6">
      <div className="flex gap-4 text-sm">
        <Stat label="Concerns complete" value={`${report.complete_concerns}/${report.total_concerns}`} />
        <Stat label="Gap files" value={report.total_gap_files} />
      </div>

      <section>
        <SectionHeader tone="progress" label="Per-concern coverage" count={concerns.length} />
        {concerns.length === 0 ? (
          <p className="text-sm text-ink-dim pl-1 mt-1">No catalog → no audit matrix.</p>
        ) : (
          <ListCard>
            {concerns.map((c) => (
              <ConcernRow key={c.concern_id} c={c} />
            ))}
          </ListCard>
        )}
      </section>

      {report.cross_model.length > 0 && (
        <section>
          <SectionHeader tone="muted" label="Cross-model" count={report.cross_model.length} hint="multi-model cells" />
          <ListCard>
            {report.cross_model.map((x, i) => (
              <div key={`${x.concern_id}:${x.file_path}:${i}`} className="px-4 py-2 text-xs">
                <span className="font-mono text-ink">{x.concern_id}</span>
                <span className="text-ink-mute"> · {x.file_path}</span>
                {x.disagreement && (
                  <span className="ml-2 text-amber-700 font-medium">disagreement</span>
                )}
              </div>
            ))}
          </ListCard>
        </section>
      )}

      {report.serendipitous.length > 0 && (
        <section>
          <SectionHeader tone="bad" label="Serendipitous" count={report.serendipitous.length} hint="unscoped findings" />
          <ListCard>
            {report.serendipitous.map((s) => (
              <div key={s.theme} className="px-4 py-2 text-xs">
                <span className="text-ink">{s.theme}</span>
                <span className="ml-2 text-ink-mute tabular-nums">{s.count}</span>
              </div>
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg border border-border bg-panel px-3 py-2">
      <div className="text-[11px] text-ink-mute">{label}</div>
      <div className="text-sm font-semibold text-ink tabular-nums">{value}</div>
    </div>
  );
}

function ConcernRow({ c }: { c: ConcernCoverage }) {
  const assessed = c.asserted_files.length;
  const inScope = c.in_scope_files.length;
  const pct = inScope === 0 ? 0 : Math.round((assessed / inScope) * 100);
  return (
    <div className="px-4 py-3">
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-medium text-ink">{c.name}</span>
        <span className="text-[11px] font-mono text-ink-mute">{c.concern_id}</span>
        <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
          {c.severity}
        </span>
        {c.gap_files.length > 0 && (
          <span className="text-[10px] text-amber-700 font-medium">{c.gap_files.length} gap</span>
        )}
      </div>
      <div className="mt-1.5 flex items-center gap-2">
        <div className="h-1.5 flex-1 rounded bg-slate-100 overflow-hidden">
          <div className="h-full bg-brand-500" style={{ width: `${pct}%` }} />
        </div>
        <span className="text-[11px] text-ink-mute tabular-nums w-24 text-right">
          {assessed}/{inScope} files
        </span>
      </div>
      <div className="mt-1 text-[11px] text-ink-mute tabular-nums">
        clean {c.clean} · finding {c.findings} · examined {c.examined} · n/a {c.not_applicable}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageAuditTab.tsx
git commit -m "feat(cp/web): coverage audit tab"
```

---

## Task 9: Gap tab (derived from audit) — with a tested helper

**Files:**
- Create: `crates/rupu-cp/web/src/lib/coverageGap.ts`
- Create: `crates/rupu-cp/web/src/lib/coverageGap.test.ts`
- Modify: `crates/rupu-cp/web/src/components/coverage/CoverageGapTab.tsx`

**Interfaces:**
- Produces: `gapRows(report: AuditReport): GapRow[]` where
  `GapRow { concern_id: string; name: string; severity: string; gap_files: string[] }`
  — only concerns with non-empty `gap_files`, severity-sorted (critical→info).

- [ ] **Step 1: Write the failing test**

`crates/rupu-cp/web/src/lib/coverageGap.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { gapRows } from './coverageGap';
import type { AuditReport } from './api';

function report(concerns: AuditReport['concerns']): AuditReport {
  return {
    target_id: 't',
    concerns,
    files: [],
    cross_model: [],
    serendipitous: [],
    total_concerns: concerns.length,
    complete_concerns: 0,
    total_gap_files: 0,
  };
}

const base = {
  in_scope_files: [],
  asserted_files: [],
  clean: 0,
  findings: 0,
  examined: 0,
  not_applicable: 0,
};

describe('gapRows', () => {
  it('keeps only concerns with gap files, severity-sorted', () => {
    const r = report([
      { concern_id: 'a', name: 'A', severity: 'low', gap_files: ['x.rs'], ...base },
      { concern_id: 'b', name: 'B', severity: 'critical', gap_files: ['y.rs', 'z.rs'], ...base },
      { concern_id: 'c', name: 'C', severity: 'high', gap_files: [], ...base },
    ]);
    const rows = gapRows(r);
    expect(rows.map((x) => x.concern_id)).toEqual(['b', 'a']);
    expect(rows[0].gap_files).toEqual(['y.rs', 'z.rs']);
  });

  it('returns empty when there are no gaps', () => {
    expect(gapRows(report([]))).toEqual([]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/coverageGap.test.ts`
Expected: FAIL — cannot find `./coverageGap`.

- [ ] **Step 3: Implement the helper**

`crates/rupu-cp/web/src/lib/coverageGap.ts`:

```ts
import { normFindingSeverity, sevRank, type AuditReport } from './api';

export interface GapRow {
  concern_id: string;
  name: string;
  severity: string;
  gap_files: string[];
}

/** Concerns with unassessed in-scope files, severity-sorted (critical→info). */
export function gapRows(report: AuditReport): GapRow[] {
  return report.concerns
    .filter((c) => c.gap_files.length > 0)
    .map((c) => ({
      concern_id: c.concern_id,
      name: c.name,
      severity: c.severity,
      gap_files: c.gap_files,
    }))
    .sort(
      (a, b) => sevRank(normFindingSeverity(b.severity)) - sevRank(normFindingSeverity(a.severity)),
    );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/coverageGap.test.ts`
Expected: PASS (both cases).

- [ ] **Step 5: Implement the Gap tab**

`CoverageGapTab.tsx`:

```tsx
// Gap tab — concerns whose in-scope files weren't all assessed. Derived from
// the same audit report the Audit tab uses.
import { useEffect, useMemo, useState } from 'react';
import { api, type AuditReport } from '../../lib/api';
import { gapRows } from '../../lib/coverageGap';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageGapTab({ target, wsId }: { target: string; wsId?: string }) {
  const [report, setReport] = useState<AuditReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setReport(null);
    setError(null);
    api
      .getCoverageAudit(target, wsId)
      .then((d) => !cancelled && setReport(d))
      .catch((e: unknown) =>
        !cancelled && setError(e instanceof Error ? e.message : 'Failed to load gaps'),
      );
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  const rows = useMemo(() => (report ? gapRows(report) : []), [report]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!report) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (rows.length === 0)
    return <p className="mt-4 text-sm text-ink-dim">No gaps — every in-scope file assessed.</p>;

  return (
    <section className="mt-6">
      <SectionHeader tone="bad" label="Coverage gaps" count={rows.length} hint="concerns with unassessed files" />
      <ListCard>
        {rows.map((r) => (
          <div key={r.concern_id} className="px-4 py-3">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium text-ink">{r.name}</span>
              <span className="text-[11px] font-mono text-ink-mute">{r.concern_id}</span>
              <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium ring-1 bg-slate-100 text-ink-mute ring-slate-200">
                {r.severity}
              </span>
              <span className="text-[10px] text-amber-700 font-medium tabular-nums">
                {r.gap_files.length} files
              </span>
            </div>
            <ul className="mt-1 space-y-0.5">
              {r.gap_files.map((f) => (
                <li key={f} className="text-[11px] font-mono text-ink-mute break-all">{f}</li>
              ))}
            </ul>
          </div>
        ))}
      </ListCard>
    </section>
  );
}
```

- [ ] **Step 6: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/lib/coverageGap.ts crates/rupu-cp/web/src/lib/coverageGap.test.ts crates/rupu-cp/web/src/components/coverage/CoverageGapTab.tsx
git commit -m "feat(cp/web): coverage gap tab (derived from audit)"
```

---

## Task 10: Templates page + link from Coverage list

**Files:**
- Create: `crates/rupu-cp/web/src/pages/CoverageTemplates.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx`
- Modify: `crates/rupu-cp/web/src/pages/Coverage.tsx`

**Interfaces:**
- Consumes: `api.getCoverageTemplates`, `api.getCoverageTemplate`,
  `TemplateSummary`, `TemplateDetail`, `SectionHeader`, `ListCard`.

- [ ] **Step 1: Implement the page**

`CoverageTemplates.tsx`:

```tsx
// Global coverage Templates page — bundled concern templates (target-independent).
// Route: /coverage/templates
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { api, type TemplateSummary, type TemplateDetail } from '../lib/api';
import { SectionHeader } from '../components/lists/SectionHeader';
import { ListCard } from '../components/lists/ListCard';

export default function CoverageTemplates() {
  const [templates, setTemplates] = useState<TemplateSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getCoverageTemplates()
      .then((d) => !cancelled && setTemplates(d))
      .catch((e: unknown) =>
        !cancelled && setError(e instanceof Error ? e.message : 'Failed to load templates'),
      );
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="p-8 max-w-5xl">
      <Link to="/coverage" className="inline-flex items-center gap-1.5 text-xs font-medium text-ink-dim hover:text-ink">
        <ArrowLeft size={14} />
        Coverage
      </Link>
      <header className="mt-3">
        <h1 className="text-2xl font-semibold text-ink">Concern Templates</h1>
        <p className="mt-1 text-sm text-ink-dim">Bundled concern catalogs (OWASP, CWE, STRIDE, …).</p>
      </header>

      {error && <p className="mt-4 text-sm text-red-700">{error}</p>}
      {templates === null ? (
        <p className="mt-4 text-sm text-ink-dim">Loading…</p>
      ) : (
        <section className="mt-6">
          <SectionHeader tone="muted" label="Templates" count={templates.length} />
          <ListCard>
            {templates.map((t) => (
              <TemplateRow key={t.name} t={t} />
            ))}
          </ListCard>
        </section>
      )}
    </div>
  );
}

function TemplateRow({ t }: { t: TemplateSummary }) {
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<TemplateDetail | null>(null);

  function toggle() {
    const next = !open;
    setOpen(next);
    if (next && !detail) {
      api.getCoverageTemplate(t.name).then(setDetail).catch(() => setDetail(null));
    }
  }

  return (
    <div className="px-4 py-3">
      <button onClick={toggle} className="w-full text-left">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-ink">{t.name}</span>
          <span className="text-[10px] text-ink-mute">v{t.version}</span>
          <span className="text-[11px] text-ink-mute tabular-nums">{t.concern_count} concerns</span>
          {Object.entries(t.severity_breakdown).map(([sev, n]) => (
            <span key={sev} className="text-[10px] text-ink-mute">{sev}:{n}</span>
          ))}
        </div>
        {t.description && <p className="mt-1 text-xs text-ink-dim leading-snug">{t.description}</p>}
      </button>
      {open && detail && (
        <ul className="mt-2 space-y-1 border-l-2 border-border pl-3">
          {detail.concerns.map((c) => (
            <li key={c.id} className="text-xs">
              <span className="font-medium text-ink">{c.name}</span>
              <span className="ml-2 font-mono text-[10px] text-ink-mute">{c.id}</span>
              <span className="ml-2 text-[10px] text-ink-mute">{c.severity}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Add the route (before `/coverage/:target`) in App.tsx**

```tsx
const CoverageTemplates = React.lazy(() => import('./pages/CoverageTemplates'));
```

```tsx
<Route path="/coverage/templates" element={<Suspense fallback={<PageFallback />}><CoverageTemplates /></Suspense>} />
```

(Place this above all `/coverage/:target…` routes.)

- [ ] **Step 3: Link from the Coverage list header**

In `Coverage.tsx`, add a link in the page header (next to the title):

```tsx
import { Link } from 'react-router-dom';
```

```tsx
<Link to="/coverage/templates" className="text-xs font-medium text-brand-700 hover:text-brand-500">
  Templates →
</Link>
```

- [ ] **Step 4: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success. Confirm `/coverage/templates` does not get shadowed by
`/coverage/:target` (it must render the Templates page).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/CoverageTemplates.tsx crates/rupu-cp/web/src/App.tsx crates/rupu-cp/web/src/pages/Coverage.tsx
git commit -m "feat(cp/web): global coverage Templates page + list link"
```

---

## Task 11: Full verification

- [ ] **Step 1: Backend**

Run: `cargo test -p rupu-cp --lib` → all pass.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.
Run: `rustfmt --edition 2021 --check crates/rupu-cp/src/api/coverage.rs` → exit 0.

- [ ] **Step 2: Frontend**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
Run: `cd crates/rupu-cp/web && npx vitest run` → all pass (incl. `coverageGap`).
Run: `cd crates/rupu-cp/web && npm run build` → success; new pages/tabs are
their own lazy chunks and `highlight.js` is not pulled into the main entry.

- [ ] **Step 3: Manual smoke (recommended before merge)**

`rupu cp serve` from the rupu checkout (has sample `.rupu/coverage` targets if
any runs have recorded coverage). Visit `/coverage`, open a target → Catalog /
Audit / Gap tabs; visit `/coverage/templates`. Confirm data renders and tabs
preserve `?ws_id=`.

- [ ] **Step 4: Open PR** (per repo convention — branch already isolated)

```bash
gh pr create --title "feat(cp): coverage templates/catalog/audit/gap in web UI (Plan 1)" --body "…"
```

---

## Self-review notes (author)

- Spec coverage: templates (Tasks 1,2,10), catalog (3,7), audit (4,8), gap
  (9, derived from audit), tab shell (6), api (5). Diff is Plan 2 (out of scope
  here) — matches the spec's PR1/PR2 split.
- Route ordering: `/coverage/templates` and the `:target/<tab>` routes are
  declared before `/coverage/:target`; target ids are content hashes so the
  literal `templates` segment cannot collide.
- Type consistency: backend returns library types (`Template`, `FlatCatalog`,
  `AuditReport`) verbatim; the TS interfaces in Task 5 mirror their serde field
  names (snake_case, lowercase severities). `gapRows` consumes `AuditReport`
  exactly as defined in Task 5.
- The only computed backend logic (`builtin_template_summaries`,
  `read_target_catalog`, `run_target_audit`) is unit-tested; pure pass-through
  serialization relies on `rupu-coverage`'s own tests.
```
