# CP Rich Rows — Plan 2 (Aggregate Areas) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Apply the metric-strip rows + per-entity usage bar graph to the Projects / Agents / Workflows list pages, fed by backend per-entity rollups.

**Architecture:** One backend pass over the run store computes each run's usage once and groups it by entity key (workspace_id / workflow_name / agent) into a `{ usage, run_count, last_active }` rollup, attached to the list rows. Frontend reuses `MetricRow` + `UsageBarChart` (from Plan 1).

**Tech Stack:** Rust 2021 / axum / `rupu-cp::usage`; React 18 + TS + recharts.

**Prerequisite:** Plan 1 is merged (or stacked beneath) — `MetricRow`, `UsageBarChart`, and the `crate::usage` `RunMetrics`/`run_metrics` helpers exist.

**Conventions:** Same as Plan 1 — branch off the post-Plan-1 `main`; never touch `main`; no `rustfmt`/`cargo fmt`; `git status --short` clean before commit; stage only your files; no `any`; static Tailwind; main chunk ~48 KB; `rupu-cp`/web tests + clippy are the gates. Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Perf note (intentional, document — do not silently truncate):** these rollups read every run's transcript once per list fetch (same cost as the existing `/api/usage` overview). Bounded by the number of runs. Caching keyed by transcript mtime is a future optimization, out of scope; if a rollup is measurably slow, `tracing::warn!` the cost rather than capping coverage.

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `crates/rupu-cp/src/usage.rs` | `EntityRollup` + `rollup_by` (one pass, grouped) | 1 |
| `crates/rupu-cp/src/api/projects.rs` | `usage`/`run_count`/`last_active` on `ProjectRow` (list) | 2 |
| `crates/rupu-cp/src/api/workflows.rs` | `usage`/`run_count`/`last_run` on the workflow list rows | 3 |
| `crates/rupu-cp/src/api/agents.rs` | `usage`/`run_count` on the agent list rows | 4 |
| `web/src/lib/api.ts` | rollup fields on ProjectRow/WorkflowSummary/AgentSummary | 5 |
| `web/src/pages/Projects.tsx`, `Workflows.tsx`, `Agents.tsx` | metric rows + bar graph | 6 |

---

## Task 1: `EntityRollup` + `rollup_by`

**Files:** Modify `crates/rupu-cp/src/usage.rs`

One pass over the runs: compute each run's `UsageSummary` once (via `summarize_run`), and fold into per-key rollups. The key function lets callers group by `workspace_id`, `workflow_name`, or the run's agent.

- [ ] **Step 1: failing test** — append to `usage.rs` tests: build two fixture runs in a temp `RunStore` is heavy; instead unit-test the pure fold. Add a pure helper + test:
```rust
    #[test]
    fn entity_rollup_folds_usage_and_counts() {
        let mut r = EntityRollup::default();
        r.add(&UsageSummary { input_tokens: 10, output_tokens: 5, cached_tokens: 0, total_tokens: 15, cost_usd: Some(1.0), priced: true, runs: 1 }, Some("2026-01-02T00:00:00Z".into()));
        r.add(&UsageSummary { input_tokens: 20, output_tokens: 0, cached_tokens: 0, total_tokens: 20, cost_usd: Some(2.0), priced: true, runs: 1 }, Some("2026-01-01T00:00:00Z".into()));
        assert_eq!(r.run_count, 2);
        assert_eq!(r.usage.input_tokens, 30);
        assert_eq!(r.usage.total_tokens, 35);
        assert!((r.usage.cost_usd.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(r.last_active.as_deref(), Some("2026-01-02T00:00:00Z")); // most-recent wins
    }
```
- [ ] **Step 2: implement** in `usage.rs`:
```rust
/// Per-entity rollup: summed usage + run count + most-recent activity.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EntityRollup {
    pub usage: UsageSummary,
    pub run_count: u64,
    /// Most-recent contributing run timestamp (ISO-8601), if any.
    pub last_active: Option<String>,
}

impl EntityRollup {
    /// Fold one run's usage + timestamp into the rollup.
    pub fn add(&mut self, usage: &UsageSummary, at: Option<String>) {
        // Reuse rollup() semantics for the two summaries.
        self.usage = rollup([self.usage.clone(), usage.clone()].into_iter());
        self.run_count += 1;
        if let Some(at) = at {
            match &self.last_active {
                Some(cur) if *cur >= at => {}
                _ => self.last_active = Some(at),
            }
        }
    }
}
```
(`rollup` already exists from A.4; `UsageSummary` derives `Clone`. If not `Clone`, add `Clone` to its derives.)
- [ ] **Step 3:** also add a store-driven grouping helper:
```rust
use std::collections::BTreeMap;

/// Group every run's usage by a caller-chosen key, computing per-key rollups
/// in a single pass over the store. `key_of` returns `None` to skip a run.
pub fn rollup_by(
    store: &RunStore,
    runs: &[rupu_orchestrator::RunRecord],
    pricing: &PricingConfig,
    key_of: impl Fn(&rupu_orchestrator::RunRecord) -> Option<String>,
) -> BTreeMap<String, EntityRollup> {
    let mut out: BTreeMap<String, EntityRollup> = BTreeMap::new();
    for run in runs {
        let Some(key) = key_of(run) else { continue };
        let usage = summarize_run(store, &run.id, pricing);
        let at = Some(run.started_at.to_rfc3339());
        out.entry(key).or_default().add(&usage, at);
    }
    out
}
```
- [ ] **Step 4:** `cargo test -p rupu-cp entity_rollup` → pass; `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean.
- [ ] **Step 5: commit** `git add crates/rupu-cp/src/usage.rs` → `feat(cp): EntityRollup + rollup_by (per-entity usage grouping)`

---

## Task 2: Project list rollups

**Files:** Modify `crates/rupu-cp/src/api/projects.rs`

`ProjectRow` gains `usage: UsageSummary`, `run_count: u64`, `last_active: Option<String>`. `list_projects` computes the rollup grouped by `workspace_id`.

- [ ] Add the three fields to `ProjectRow` (Serialize); set them in `project_row` to defaults, then fill in `list_projects`:
```rust
async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<ProjectRow>>> {
    let workspaces = store(&s).list().unwrap_or_default();
    let runs = s.run_store.list().unwrap_or_default();
    let rollups = crate::usage::rollup_by(&s.run_store, &runs, &s.pricing, |r| Some(r.workspace_id.clone()));
    let mut rows: Vec<ProjectRow> = workspaces.iter().map(project_row).collect();
    for row in &mut rows {
        if let Some(roll) = rollups.get(&row.ws_id) {
            row.usage = roll.usage.clone();
            row.run_count = roll.run_count;
            // keep the workspace's own last_run_at if richer; else the rollup's.
        }
    }
    rows.sort_by(|a, b| b.last_run_at.cmp(&a.last_run_at));
    Ok(Json(rows))
}
```
(`project_row` sets `usage: UsageSummary::default(), run_count: 0, last_active: None`.)
- [ ] Verify `cargo test -p rupu-cp` green + clippy clean; `git status --short` only projects.rs.
- [ ] **commit** → `feat(cp): project list usage rollup + run count`

---

## Task 3: Workflow list rollups

**Files:** Modify `crates/rupu-cp/src/api/workflows.rs`

The workflow list (`list_workflows` → `WorkflowDto { name, scope }`) gains `usage`/`run_count`/`last_run`. Group runs by `workflow_name`.

- [ ] Add `usage: crate::usage::UsageSummary`, `run_count: u64`, `last_run: Option<String>` to `WorkflowDto`. In `list_workflows`, after building the name rows, compute `rollup_by(&s.run_store, &runs, &s.pricing, |r| Some(r.workflow_name.clone()))` and attach by name (default zero for workflows with no runs). `scan_workflow_names` stays; fill the new fields after.
- [ ] Verify green + clippy clean; `git status --short` only workflows.rs.
- [ ] **commit** → `feat(cp): workflow list usage rollup + run count`

---

## Task 4: Agent list rollups

**Files:** Modify `crates/rupu-cp/src/api/agents.rs`

`AgentDto`/`AgentSummary` list rows gain `usage`/`run_count`. A run's agent isn't on `RunRecord`; group by the agent from each run's transcript usage rows instead.

- [ ] READ `agents.rs`. Add `usage: crate::usage::UsageSummary` + `run_count: u64` to the list DTO (default zero). In the list handler, compute usage grouped by agent: for each run, `summarize_run` gives a `UsageSummary` but not the agent name — instead use `rupu_transcript::aggregate` over all runs' transcripts to get `UsageRow`s (which carry `agent`), then `breakdown(rows, pricing, GroupBy::Agent)` (from A.4) to get per-agent `UsageBreakdownRow`. Map each breakdown row's `agent` → `{ usage: {tokens+cost}, run_count: row.runs }` and attach to the matching agent list row by name. (Agents with no runs keep zero.) Document this approach in the commit body.
- [ ] Verify green + clippy clean; `git status --short` only agents.rs.
- [ ] **commit** → `feat(cp): agent list usage rollup + run count`

---

## Task 5: API types

**Files:** Modify `web/src/lib/api.ts`

- [ ] `ProjectRow`: add `usage: UsageSummary; run_count: number; last_active?: string | null;`. `WorkflowSummary`: add `usage: UsageSummary; run_count: number; last_run?: string | null;`. `AgentSummary`: add `usage: UsageSummary; run_count: number;`. `npm run build` strict exit 0 + `npm test -- --run api` green.
- [ ] **commit** → `feat(cp/web): aggregate rollup row types`

---

## Task 6: Wire Projects / Workflows / Agents rows + bar graph

**Files:** Modify `web/src/pages/Projects.tsx`, `web/src/pages/Workflows.tsx`, `web/src/pages/Agents.tsx`

Apply `MetricRow` + a `UsageBarChart` above each list (reusing Plan 1's components; these pages are A.4 client-windowed — the bar graph plots the full fetched set).

- [ ] **Projects:** `ProjectRow` → `MetricRow` (header = name + path + repo/branch chips; trailing = last-active relative time; metrics = `run_count` runs · total tokens · cost from `p.usage` · last-active). `UsageBarChart` above (bars from `p.usage`, label = project name, `to` = the project link).
- [ ] **Workflows:** `WorkflowRow` → `MetricRow` (header = name + scope chip; metrics = run_count · tokens · cost from `usage` · last-run). `UsageBarChart` above.
- [ ] **Agents:** `AgentRow` → `MetricRow` (header = name + provider/model/effort chips; keep the description line; metrics = run_count · tokens · cost from `usage`). `UsageBarChart` above.
- [ ] Guard each `usage`/`run_count` access (rollups may be zero). `npm run build` strict exit 0; `npm test -- --run` green; main chunk ~48 KB.
- [ ] **commit** the three files → `feat(cp/web): metric rows + usage graph on projects / workflows / agents`

---

## Task 7: Whole-slice gate

- [ ] `cargo test -p rupu-cp` + `clippy --all-targets` clean; `npm run build` strict + `npm test -- --run` (counts) + main chunk ~48 KB + `grep -c recharts` in main = 0; no `any`; static Tailwind; `git status --short` clean. Visual handoff: Projects/Workflows/Agents lists show metric rows (run count + tokens + cost) + the per-entity bar graph on top.

---

## Done criteria
- Projects / Workflows / Agents list rows carry a per-entity usage rollup (`usage` + `run_count` + last-active/last-run) and render via `MetricRow` with a `UsageBarChart` on top.
- One pass over the store per list fetch (perf boundary documented); no `rupu-cli` dep; no drift; main chunk ~48 KB.
