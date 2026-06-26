# CP Coverage Subfeatures — Plan 2 (Diff)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `rupu coverage diff` capability to the CP web UI — a Diff tab on the coverage target-detail page with base/compare run pickers and a delta view.

**Architecture:** Two thin backend handlers over `rupu-coverage` (`list_runs`, `run_diff`); `RunListEntry`/`RunDiff` derive `Serialize` so they return as JSON directly. The frontend adds a `CoverageDiffTab` mounted as a fourth tab in the existing `CoverageDetail` shell (Plan 1).

**Tech Stack:** Rust + axum (backend), React 18 + TypeScript + Vite + Vitest + Tailwind (frontend).

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-coverage-subfeatures-design.md`
Builds on Plan 1 (already merged): the tabbed shell, `scoped_workspaces` helper, and coverage api types/methods already exist.

## Global Constraints

- Workspace deps only; `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- `rupu-cp` is thin: delegate to `rupu-coverage`.
- Backend tests: factor pure logic into free functions and test those; no server.
- Run per-file `rustfmt`, never package-wide `cargo fmt`.
- Frontend component tests use `// @vitest-environment jsdom`; pure-logic tests
  run in node env. New routes already exist in the shell; no new lazy page.

## Reference: exact library types/signatures (already `Serialize`)

- `list_runs(paths: &CoveragePaths) -> Result<Vec<RunListEntry>, DiffError>`
- `run_diff(paths: &CoveragePaths, base: &RunSelector, compare: &RunSelector) -> Result<RunDiff, DiffError>`
- `RunSelector` implements `FromStr` (infallible): `"latest"` → `Latest`,
  `"previous"` → `Previous`, anything else → `RunId(s)`.
- `DiffError::{Io, UnknownRun(String), NoRunMatches(String)}`.
- `RunListEntry { run_id: String, started_at: DateTime<Utc>, model: String, surface: Surface, cells_asserted: usize, findings: usize, files_touched: usize }` (`Surface` serializes lowercase: `workflow|agent|autoflow|session`).
- `RunDiff { base_runs: Vec<String>, compare_runs: Vec<String>, newly_asserted: Vec<CellRef>, no_longer_asserted: Vec<CellRef>, verdict_flips: Vec<VerdictFlip>, findings_appeared: Vec<FindingThemeRef>, findings_disappeared: Vec<FindingThemeRef>, newly_touched: Vec<String>, no_longer_touched: Vec<String> }`.
- `CellRef { concern_id: String, file_path: String, status: AssertionStatus }` (status snake_case: `clean|finding|examined|not_applicable`).
- `VerdictFlip { concern_id, file_path, base_status, compare_status, high_signal: bool }`.
- `FindingThemeRef { concern_id: Option<String>, theme: String }`.

---

## Task 1: Runs endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/:target/runs?ws_id=…` → `Vec<RunListEntry>`.
  404 when target not found under any candidate workspace.
- Consumes: existing `scoped_workspaces`, `discover_targets`, `CoveragePaths`.

- [ ] **Step 1: Write the failing test** (add to the `tests` module in coverage.rs)

```rust
    #[test]
    fn list_runs_for_existing_target_only() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        let paths = CoveragePaths::new(wp, "tgt-runs");
        std::fs::create_dir_all(&paths.root).unwrap();
        // Empty concerns ledger → target discovered, zero runs.
        std::fs::File::create(&paths.concerns).unwrap().write_all(b"").unwrap();

        let got = list_target_runs(wp, "tgt-runs").expect("ok");
        assert!(got.is_some(), "existing target resolves");
        assert_eq!(got.unwrap().len(), 0, "no runs recorded");
        assert!(list_target_runs(wp, "missing").unwrap().is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib list_runs_for_existing_target_only`
Expected: FAIL — `list_target_runs` not found.

- [ ] **Step 3: Write minimal implementation**

Extend the `rupu_coverage` import with `list_runs`:

```rust
use rupu_coverage::{
    builtin_names, coverage_status, discover_targets, file_views, list_runs, read_file_events,
    read_findings, read_snapshot, resolve_builtin, run_audit, CoveragePaths, CoverageStatusInput,
};
```

Add helper + handler (next to the audit handler):

```rust
/// List runs for a target under one workspace path. `Ok(None)` when the target
/// isn't present under this workspace.
fn list_target_runs(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<Vec<rupu_coverage::RunListEntry>>, String> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    list_runs(&paths).map(Some).map_err(|e| e.to_string())
}

async fn get_runs(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<Vec<rupu_coverage::RunListEntry>>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match list_target_runs(wp, &target) {
            Ok(Some(runs)) => return Ok(Json(runs)),
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
        .route("/api/coverage/:target/runs", get(get_runs))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib list_runs_for_existing_target_only`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage runs endpoint"
```

---

## Task 2: Diff endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/coverage.rs`

**Interfaces:**
- Produces: `GET /api/coverage/:target/diff?ws_id=&base=&compare=` → `RunDiff`.
  `base` defaults to `previous`, `compare` to `latest`. Selector strings parse
  via `RunSelector::from_str`. 404 when target missing; 400 when a selector
  can't resolve (e.g. fewer than two runs).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn diff_query_selectors_default_and_parse() {
        use rupu_coverage::RunSelector;
        // Defaults when absent.
        assert_eq!(parse_selector(&None, RunSelector::Latest), RunSelector::Latest);
        // Keywords.
        assert_eq!(
            parse_selector(&Some("previous".to_string()), RunSelector::Latest),
            RunSelector::Previous
        );
        // Explicit id.
        assert_eq!(
            parse_selector(&Some("run_123".to_string()), RunSelector::Latest),
            RunSelector::RunId("run_123".to_string())
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib diff_query_selectors_default_and_parse`
Expected: FAIL — `parse_selector` not found.

- [ ] **Step 3: Write minimal implementation**

Extend the import with `run_diff`, `DiffError`, `RunSelector`:

```rust
use rupu_coverage::{
    builtin_names, coverage_status, discover_targets, file_views, list_runs, read_file_events,
    read_findings, read_snapshot, resolve_builtin, run_audit, run_diff, CoveragePaths,
    CoverageStatusInput, DiffError, RunSelector,
};
```

Add the query struct, selector parser, target-diff helper, and handler:

```rust
#[derive(Deserialize)]
struct DiffQuery {
    ws_id: Option<String>,
    base: Option<String>,
    compare: Option<String>,
}

/// Parse an optional selector string, falling back to `default` when absent.
/// `RunSelector::from_str` is infallible (`latest`/`previous`/explicit id).
fn parse_selector(raw: &Option<String>, default: RunSelector) -> RunSelector {
    match raw {
        Some(s) => s.parse().unwrap_or(default),
        None => default,
    }
}

/// Run a diff for a target under one workspace path. `Ok(None)` when the target
/// isn't present; `Err(DiffError)` when selectors can't resolve.
fn run_target_diff(
    wp: &std::path::Path,
    target: &str,
    base: &RunSelector,
    compare: &RunSelector,
) -> Result<Option<rupu_coverage::RunDiff>, DiffError> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    run_diff(&paths, base, compare).map(Some)
}

async fn get_diff(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<rupu_coverage::RunDiff>> {
    let base = parse_selector(&q.base, RunSelector::Previous);
    let compare = parse_selector(&q.compare, RunSelector::Latest);
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match run_target_diff(wp, &target, &base, &compare) {
            Ok(Some(diff)) => return Ok(Json(diff)),
            Ok(None) => continue,
            // A selector that can't resolve (too few runs / bad id) is a client
            // condition, not a server fault.
            Err(DiffError::Io(e)) => return Err(ApiError::internal(e.to_string())),
            Err(e @ (DiffError::UnknownRun(_) | DiffError::NoRunMatches(_))) => {
                return Err(ApiError::bad_request(e.to_string()))
            }
        }
    }
    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}
```

Register the route:

```rust
        .route("/api/coverage/:target/diff", get(get_diff))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cp --lib diff_query_selectors_default_and_parse`
Expected: PASS.

- [ ] **Step 5: Verify backend clean**

Run: `cargo test -p rupu-cp --lib` → all pass.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.
Run: `rustfmt --edition 2021 --check crates/rupu-cp/src/api/coverage.rs` → exit 0.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/coverage.rs
git commit -m "feat(cp): coverage diff endpoint (base/compare selectors)"
```

---

## Task 3: Frontend api types + methods

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`

**Interfaces:**
- Produces (for Task 4): the types + methods below.

- [ ] **Step 1: Add types** (in the coverage block, after `AuditReport`)

```ts
export interface RunListEntry {
  run_id: string;
  started_at: string;
  model: string;
  surface: string; // lowercase: workflow|agent|autoflow|session
  cells_asserted: number;
  findings: number;
  files_touched: number;
}

export interface CellRef {
  concern_id: string;
  file_path: string;
  status: string; // snake_case: clean|finding|examined|not_applicable
}

export interface VerdictFlip {
  concern_id: string;
  file_path: string;
  base_status: string;
  compare_status: string;
  high_signal: boolean;
}

export interface FindingThemeRef {
  concern_id: string | null;
  theme: string;
}

export interface RunDiff {
  base_runs: string[];
  compare_runs: string[];
  newly_asserted: CellRef[];
  no_longer_asserted: CellRef[];
  verdict_flips: VerdictFlip[];
  findings_appeared: FindingThemeRef[];
  findings_disappeared: FindingThemeRef[];
  newly_touched: string[];
  no_longer_touched: string[];
}
```

- [ ] **Step 2: Add methods** (after `getCoverageAudit`)

```ts
  getCoverageRuns(target: string, wsId?: string): Promise<RunListEntry[]> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<RunListEntry[]>(`/api/coverage/${encodeURIComponent(target)}/runs${qs}`);
  },
  getCoverageDiff(
    target: string,
    opts?: { wsId?: string; base?: string; compare?: string },
  ): Promise<RunDiff> {
    const p = new URLSearchParams();
    if (opts?.wsId) p.set('ws_id', opts.wsId);
    if (opts?.base) p.set('base', opts.base);
    if (opts?.compare) p.set('compare', opts.compare);
    const qs = p.toString() ? `?${p.toString()}` : '';
    return request<RunDiff>(`/api/coverage/${encodeURIComponent(target)}/diff${qs}`);
  },
```

- [ ] **Step 3: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts
git commit -m "feat(cp/web): api types + methods for coverage runs/diff"
```

---

## Task 4: Diff tab

**Files:**
- Create: `crates/rupu-cp/web/src/components/coverage/CoverageDiffTab.tsx`
- Modify: `crates/rupu-cp/web/src/pages/CoverageDetail.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx`

**Interfaces:**
- Consumes: `api.getCoverageRuns`, `api.getCoverageDiff`, `RunListEntry`,
  `RunDiff`, `SectionHeader`, `ListCard`.

- [ ] **Step 1: Add 'diff' to the shell tab type, tab bar, and a route**

In `CoverageDetail.tsx`:

```tsx
export type CoverageTab = 'overview' | 'catalog' | 'audit' | 'gap' | 'diff';
```

Add the tab-bar entry (after gap) inside the `tabs` array literal:

```tsx
            { id: 'diff', label: 'Diff', path: `/coverage/${enc}/diff${qs}` },
```

Mount it next to the other tab renders:

```tsx
      {tab === 'diff' && <CoverageDiffTab target={target} wsId={wsId} />}
```

Add the import:

```tsx
import CoverageDiffTab from '../components/coverage/CoverageDiffTab';
```

In `App.tsx`, add the route (with the other `/coverage/:target/...` routes,
before `/coverage/:target`):

```tsx
            <Route path="/coverage/:target/diff" element={<Suspense fallback={<PageFallback />}><CoverageDetail tab="diff" /></Suspense>} />
```

- [ ] **Step 2: Implement the Diff tab**

`CoverageDiffTab.tsx`:

```tsx
// Diff tab — compare two runs' contributions to a target. Base/compare pickers
// default to previous vs latest (the CLI default).
import { useEffect, useState } from 'react';
import { api, type RunListEntry, type RunDiff } from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageDiffTab({ target, wsId }: { target: string; wsId?: string }) {
  const [runs, setRuns] = useState<RunListEntry[] | null>(null);
  const [base, setBase] = useState('previous');
  const [compare, setCompare] = useState('latest');
  const [diff, setDiff] = useState<RunDiff | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Load the run list once (for the pickers + the "need 2 runs" guard).
  useEffect(() => {
    let cancelled = false;
    api
      .getCoverageRuns(target, wsId)
      .then((r) => {
        if (!cancelled) setRuns(r);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load runs');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  // Recompute the diff whenever the selectors change (and there are ≥2 runs).
  useEffect(() => {
    if (!runs || runs.length < 2) return;
    let cancelled = false;
    setDiff(null);
    setError(null);
    api
      .getCoverageDiff(target, { wsId, base, compare })
      .then((d) => {
        if (!cancelled) setDiff(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load diff');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId, base, compare, runs]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!runs) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (runs.length < 2)
    return (
      <p className="mt-4 text-sm text-ink-dim">
        Need at least two runs on this target to diff (found {runs.length}).
      </p>
    );

  const options = [
    { value: 'previous', label: 'previous' },
    { value: 'latest', label: 'latest' },
    ...runs.map((r) => ({ value: r.run_id, label: `${r.run_id} (${r.model})` })),
  ];

  return (
    <div className="mt-6 space-y-6">
      <div className="flex items-end gap-3">
        <Picker label="Base" value={base} onChange={setBase} options={options} />
        <span className="pb-1.5 text-ink-mute">→</span>
        <Picker label="Compare" value={compare} onChange={setCompare} options={options} />
      </div>

      {!diff ? (
        <p className="text-sm text-ink-dim">Computing diff…</p>
      ) : (
        <>
          <CellSection title="Newly asserted" tone="good" cells={diff.newly_asserted} />
          <FlipSection flips={diff.verdict_flips} />
          <CellSection title="No longer asserted" tone="muted" cells={diff.no_longer_asserted} />
          <ThemeSection title="Findings appeared" tone="bad" themes={diff.findings_appeared} />
          <ThemeSection
            title="Findings disappeared"
            tone="muted"
            themes={diff.findings_disappeared}
          />
          <FileSection title="Newly touched" files={diff.newly_touched} />
          <FileSection title="No longer touched" files={diff.no_longer_touched} />
        </>
      )}
    </div>
  );
}

function Picker({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] text-ink-mute">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="rounded-md border border-border bg-panel px-2 py-1 text-sm text-ink"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function CellSection({
  title,
  tone,
  cells,
}: {
  title: string;
  tone: 'good' | 'muted';
  cells: { concern_id: string; file_path: string; status: string }[];
}) {
  if (cells.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={title} count={cells.length} />
      <ListCard>
        {cells.map((c, i) => (
          <div key={`${c.concern_id}:${c.file_path}:${i}`} className="px-4 py-2 text-xs">
            <span className="font-mono text-ink">{c.concern_id}</span>
            <span className="text-ink-mute"> · {c.file_path}</span>
            <span className="ml-2 text-ink-mute">{c.status}</span>
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function FlipSection({
  flips,
}: {
  flips: {
    concern_id: string;
    file_path: string;
    base_status: string;
    compare_status: string;
    high_signal: boolean;
  }[];
}) {
  if (flips.length === 0) return null;
  return (
    <section>
      <SectionHeader tone="bad" label="Verdict flips" count={flips.length} hint="clean→finding highlighted" />
      <ListCard>
        {flips.map((f, i) => (
          <div key={`${f.concern_id}:${f.file_path}:${i}`} className="px-4 py-2 text-xs">
            <span className="font-mono text-ink">{f.concern_id}</span>
            <span className="text-ink-mute"> · {f.file_path}</span>
            <span className={f.high_signal ? 'ml-2 font-medium text-red-700' : 'ml-2 text-ink-mute'}>
              {f.base_status} → {f.compare_status}
            </span>
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function ThemeSection({
  title,
  tone,
  themes,
}: {
  title: string;
  tone: 'bad' | 'muted';
  themes: { concern_id: string | null; theme: string }[];
}) {
  if (themes.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={title} count={themes.length} />
      <ListCard>
        {themes.map((t, i) => (
          <div key={`${t.theme}:${i}`} className="px-4 py-2 text-xs">
            <span className="text-ink">{t.theme}</span>
            {t.concern_id && <span className="ml-2 font-mono text-ink-mute">{t.concern_id}</span>}
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function FileSection({ title, files }: { title: string; files: string[] }) {
  if (files.length === 0) return null;
  return (
    <section>
      <SectionHeader tone="muted" label={title} count={files.length} />
      <ListCard>
        {files.map((f) => (
          <div key={f} className="px-4 py-2 text-[11px] font-mono text-ink-mute break-all">
            {f}
          </div>
        ))}
      </ListCard>
    </section>
  );
}
```

- [ ] **Step 3: Typecheck + build**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npm run build`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/web/src/components/coverage/CoverageDiffTab.tsx crates/rupu-cp/web/src/pages/CoverageDetail.tsx crates/rupu-cp/web/src/App.tsx
git commit -m "feat(cp/web): coverage diff tab (base/compare run pickers + delta view)"
```

---

## Task 5: Full verification + PR

- [ ] **Step 1: Backend**

Run: `cargo test -p rupu-cp --lib` → all pass.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.
Run: `rustfmt --edition 2021 --check crates/rupu-cp/src/api/coverage.rs` → exit 0.

- [ ] **Step 2: Frontend**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
Run: `cd crates/rupu-cp/web && npx vitest run` → all pass.
Run: `cd crates/rupu-cp/web && npm run build` → success.

- [ ] **Step 3: Manual smoke (recommended)**

`rupu cp serve`; open a coverage target with ≥2 runs → Diff tab; change
base/compare; confirm deltas render and a target with <2 runs shows the guard.

- [ ] **Step 4: Open PR**

```bash
gh pr create --title "feat(cp): coverage diff in web UI (Plan 2)" --body "…"
```

---

## Self-review notes (author)

- Spec coverage (PR2): runs endpoint (Task 1), diff endpoint + selectors
  (Task 2), api types/methods (Task 3), Diff tab + pickers + delta view
  (Task 4). Completes the spec's deferred Diff feature.
- Route ordering: `/coverage/:target/diff` is declared with the other tab
  routes, before `/coverage/:target`.
- Type consistency: backend returns library types (`RunListEntry`, `RunDiff`)
  verbatim; the TS interfaces in Task 3 mirror their serde field names. The Diff
  tab consumes exactly those types. `parse_selector` defaults match the spec
  (base=previous, compare=latest) and the CLI.
- Tested units: `list_target_runs` (target resolution), `parse_selector`
  (default + keyword + id). `run_diff`/`list_runs` are library-tested.
```
