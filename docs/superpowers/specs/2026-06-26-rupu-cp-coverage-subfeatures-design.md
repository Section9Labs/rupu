# rupu CP — coverage subfeatures (templates / catalog / audit / gap / diff)

Date: 2026-06-26
Status: approved (design)

## Problem

The `rupu coverage` CLI exposes rich analysis the CP web UI doesn't surface
yet. The CP coverage area currently has only a list (`/coverage`), a target
detail (`/coverage/:target` — findings, files touched, assessed concerns), and
a per-project tab. We want to bring the CLI's analytical subcommands into the
web UI: **templates, catalog, audit, gap, diff**.

The architecture is favorable: `rupu-coverage` is a clean reusable library that
the CP backend already calls, so every feature maps to an existing library
function and the backend handlers stay thin (CLAUDE.md rule #2).

## Library functions each feature maps to

(All in the `rupu-coverage` crate, already a CP dependency.)

- templates → `builtin_names()`, `resolve_builtin(name) -> Template`
- catalog → `read_snapshot(catalog_path) -> FlatCatalog`
- audit → `run_audit(&CoveragePaths) -> AuditReport`
- gap → derived from `AuditReport` (`ConcernCoverage.gap_files`)
- diff → `list_runs(&paths)`, `run_diff(&paths, base, compare) -> RunDiff`

Relevant types: `Template`/`Concern` (catalog/types.rs), `FlatCatalog`
(catalog/types.rs), `AuditReport`/`ConcernCoverage`/`FileCoverage`/
`CrossModelEntry`/`SerendipitousCluster` (audit/types.rs), `RunListEntry`/
`RunDiff`/`RunSelector` (diff/types.rs, diff/generate.rs).

## Scope distinction

- **templates** is GLOBAL (target-independent) → a standalone page.
- **catalog / audit / gap / diff** are PER-TARGET → tabs on the target detail.

## UX structure

- `/coverage/:target` becomes a **tabbed shell** mirroring the existing
  `ProjectDetail` pattern: `CoverageDetail({ tab })` with a shared header
  (target id, catalog badge, counts) + TabBar.
  - Tabs: **Overview** (today's findings/files/assertions, extracted verbatim)
    | **Catalog** | **Audit** | **Gap** (| **Diff** in PR2).
  - Routes (static before dynamic in `App.tsx`):
    `/coverage/:target` (overview), `/coverage/:target/catalog`, `/audit`,
    `/gap` (`/diff` in PR2). The existing `?ws_id=` query param is preserved
    across tabs.
- **Templates** → new global page `pages/CoverageTemplates.tsx` at
  `/coverage/templates`, linked from the Coverage list header. Declared before
  `/coverage/:target`; target ids are content hashes so there is no collision
  with the literal segment `templates`.

## Phasing (one spec, two PRs)

- **PR1**: tab shell + Templates page + Catalog + Audit + Gap.
- **PR2**: Diff (run-list endpoint + base/compare pickers + delta view).

## Backend (`crates/rupu-cp/src/api/coverage.rs`)

New handlers, workspace-scoped via the existing `?ws_id=` + fallback-scan
pattern used by `get_coverage`. Each is a thin pass-through to the library.

### PR1
- `GET /api/coverage/templates` — for each `builtin_names()` resolve via
  `resolve_builtin()` and return a summary:
  `{ name, version, description, concern_count, severity_breakdown }`.
- `GET /api/coverage/templates/:name` — `resolve_builtin(name)` → full template
  with its `concerns` (id, name, severity, description, tags, applicable_globs).
- `GET /api/coverage/:target/catalog` — `read_snapshot()` → `FlatCatalog`
  (concerns + sources + render_modes). 404 when the target has no catalog.
- `GET /api/coverage/:target/audit` — `run_audit()` → full `AuditReport`.
  The **Gap** tab consumes this same endpoint (gap = audit-filtered), so there
  is no separate `/gap` endpoint — matches the CLI, stays DRY.

### PR2
- `GET /api/coverage/:target/runs` — `list_runs()` → `Vec<RunListEntry>`.
- `GET /api/coverage/:target/diff?base=&compare=` — map query selectors to
  `RunSelector` (id | `latest` | `previous`; default `previous` vs `latest`)
  → `run_diff()` → `RunDiff`.

## Frontend (`crates/rupu-cp/web`)

### PR1
- Refactor `pages/CoverageDetail.tsx` into the tabbed shell. Extract the current
  findings/files/assertions sections into the **Overview** tab (behavior
  unchanged). Shared header + TabBar always rendered; each non-overview tab
  fetches its own endpoint when active.
- New tab components under `components/coverage/`:
  - `CoverageCatalogTab` — concern table (id, name, severity, source
    template/inline, applicable globs, render mode).
  - `CoverageAuditTab` — totals strip (concerns complete / total, total gap
    files); per-concern rows with severity, in-scope/asserted/gap counts and a
    status histogram (clean/finding/examined/N-A) rendered as small inline bars
    reusing the comparative-bar idiom from `Coverage.tsx`; collapsible
    cross-model disagreements and serendipitous finding clusters below.
  - `CoverageGapTab` — derives from the audit `AuditReport`: lists concerns with
    non-empty `gap_files`, each expandable to its gap file list.
- New page `pages/CoverageTemplates.tsx` at `/coverage/templates`: list of
  templates (name, version, description, concern count, severity breakdown);
  expand/click to load and show a template's concerns. Linked from the
  `Coverage.tsx` list header.
- `lib/api.ts`: add types — `TemplateSummary`, `TemplateDetail`, `Concern`,
  `FlatCatalog`, `CatalogConcern` (+ sources/render_modes), `AuditReport`,
  `ConcernCoverage`, `FileCoverage`, `CrossModelEntry`, `SerendipitousCluster`
  — and methods `getCoverageTemplates`, `getCoverageTemplate`,
  `getCoverageCatalog`, `getCoverageAudit`. Reuse existing `normFindingSeverity`,
  `sevRank`, `FindingRow`, `SectionHeader`, `ListCard`.
- Routes are lazy-loaded; new pages land in their own chunks (existing
  convention).

### PR2
- `CoverageDiffTab` at `/coverage/:target/diff`: base/compare run pickers
  populated from `getCoverageRuns`; delta view of newly/no-longer asserted
  cells, verdict flips (highlight clean→finding), finding theme appeared/
  disappeared, and newly/no-longer touched files. `api.ts`: `RunListEntry`,
  `RunDiff`, `getCoverageRuns`, `getCoverageDiff`.

## Testing

- **Backend**
  - `templates` handler: pure (no workspace) — asserts known builtin names
    present and concern counts > 0.
  - target-scoped handlers (`catalog`, `audit`): tempfile `.rupu/coverage/
    <target>` fixture (catalog.yaml + concerns/files/findings jsonl) following
    the autoflows-test pattern; verify wiring + 404 on missing target/catalog.
  - The heavy analytical logic (`run_audit`, `run_diff`) is already unit-tested
    in `rupu-coverage`; handlers only verify wiring.
- **Frontend (vitest)**
  - gap-derivation from an `AuditReport` (concerns with/without gap files).
  - audit per-concern severity ordering.
  - component smoke tests per existing conventions.

## Out of scope
- Mutating coverage from the CP (read-only display).
- Rerun/dispatch from the CP (CLI-only for now).
- Dark-mode theming (CP is light-only).
