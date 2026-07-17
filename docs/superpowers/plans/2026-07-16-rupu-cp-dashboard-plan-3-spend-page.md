# Dashboard Redesign — Plan 3: Spend Page

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give spend a dedicated page that answers **attribution** ("where is the money going?") and **anomaly** ("what spiked?"). Trend is not a third feature — it is what attribution and anomaly *look like* on a time axis.

**Architecture:** Extend `GroupBy` from model-only to model / provider / agent / workflow / host / project, fan the endpoint out across hosts, and surface the unpriced-model gap as an explicit number rather than a `*` footnote. The page reuses `UsageTimelineStacked` and `ModelBreakdownTable`, which Plan 1 deliberately left in place.

**Tech Stack:** Rust (axum, serde), React + TypeScript, Recharts (already a dep), Vitest. **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-cp-dashboard-redesign-design.md` §6
**Depends on:** Plan 2 (host fan-out pattern + `dashboard_summary`). Independent of Plan 1.

## Global Constraints

- **Workspace deps only.** Versions pinned in root `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **rupu-cp stays read-only.**
- **Never silently under-count.** An attribution page that quietly omits unpriced models is worse than no page — the same rule that drove the SSH `list_runs` fix.
- **No hardcoded colors** in the web layer — `useThemeColors()` / `--c-*` tokens only.
- **Never run package-wide `cargo fmt`.** Format only files you touch.
- **`make cp-web` before `make release`.**

**Good news from reconnaissance, correcting the spec:** §6 defers the agent dimension pending investigation. It does not need deferring — `crate::usage::GroupBy` (`src/usage.rs:205`) **already has `Provider | Model | Agent`**, and `UsageBreakdownRow` already carries all three. Agent attribution works today and is simply not exposed in the UI. What genuinely does not exist is **workflow / host / project**, which need a join from `UsageRow` to `RunRecord`.

---

### Task 1: Extend `GroupBy`; stop silently defaulting

**Files:**
- Modify: `crates/rupu-cp/src/usage.rs`
- Modify: `crates/rupu-cp/src/api/usage.rs`
- Test: `crates/rupu-cp/src/usage.rs` (inline tests)

**Interfaces:**
- Produces: `GroupBy::{Provider, Model, Agent, Workflow, Host, Project}`, `GroupBy::parse(&str) -> Option<GroupBy>` (**signature change** — was infallible), `UsageBreakdownRow` gaining `workflow`, `host_id`, `workspace_id`.

**The silent-default bug:** `GroupBy::parse` (`usage.rs:213`) matches `"provider"` / `"agent"` and falls through to `_ => GroupBy::Model`. So `?group_by=workflw` (typo) silently returns a model breakdown, and the caller never learns their pivot was ignored. Same class as the `?range=` bug fixed in Plan 2 Task 7.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn group_by_parses_known_dimensions() {
        assert_eq!(GroupBy::parse("model"), Some(GroupBy::Model));
        assert_eq!(GroupBy::parse("provider"), Some(GroupBy::Provider));
        assert_eq!(GroupBy::parse("agent"), Some(GroupBy::Agent));
        assert_eq!(GroupBy::parse("workflow"), Some(GroupBy::Workflow));
        assert_eq!(GroupBy::parse("host"), Some(GroupBy::Host));
        assert_eq!(GroupBy::parse("project"), Some(GroupBy::Project));
    }

    #[test]
    fn group_by_rejects_unknown_rather_than_defaulting() {
        // A typo must not silently return a model breakdown — the caller would
        // never learn their pivot was ignored.
        assert_eq!(GroupBy::parse("workflw"), None);
        assert_eq!(GroupBy::parse(""), None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp group_by_parses_known_dimensions`
Expected: FAIL — `parse` returns `GroupBy`, not `Option<GroupBy>`; `Workflow` variant missing.

- [ ] **Step 3: Implement**

Replace the enum and `parse` in `crates/rupu-cp/src/usage.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Provider,
    Model,
    Agent,
    /// Needs a UsageRow -> RunRecord join; see `breakdown_joined`.
    Workflow,
    Host,
    Project,
}

impl GroupBy {
    /// Parse the `group_by` query param.
    ///
    /// Returns `None` on anything unknown. Deliberately NOT infallible: the
    /// previous `_ => GroupBy::Model` fallthrough meant a typo silently
    /// returned a model breakdown and the caller never learned their pivot was
    /// ignored.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "provider" => Some(GroupBy::Provider),
            "model" => Some(GroupBy::Model),
            "agent" => Some(GroupBy::Agent),
            "workflow" => Some(GroupBy::Workflow),
            "host" => Some(GroupBy::Host),
            "project" => Some(GroupBy::Project),
            _ => None,
        }
    }

    /// Dimensions resolvable from a `UsageRow` alone, with no run join.
    pub fn is_intrinsic(&self) -> bool {
        matches!(self, GroupBy::Provider | GroupBy::Model | GroupBy::Agent)
    }
}
```

Add to `UsageBreakdownRow`:

```rust
    /// Present when grouping by a joined dimension; empty otherwise.
    #[serde(default)]
    pub workflow: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub workspace_id: String,
```

Update `breakdown`'s key match to cover the new variants, keying joined dimensions off the row's attributed fields:

```rust
        let key = match group_by {
            GroupBy::Provider => row.provider.clone(),
            GroupBy::Model => row.model.clone(),
            GroupBy::Agent => row.agent.clone(),
            GroupBy::Workflow => row.workflow.clone(),
            GroupBy::Host => row.host_id.clone(),
            GroupBy::Project => row.workspace_id.clone(),
        };
```

**Implementer note:** `UsageRow` does not carry `workflow` / `host_id` today. Task 2 adds them. To keep this task compiling and independently reviewable, add the three fields to `UsageRow` now with `#[serde(default)]` and populate them in Task 2. `workspace_id` is already present on the transcript's `run_start` event, so check whether `UsageRow` already surfaces it before adding a duplicate.

- [ ] **Step 4: Fix the call site**

`crates/rupu-cp/src/api/usage.rs:64` currently does:

```rust
let group_by = crate::usage::GroupBy::parse(q.group_by.as_deref().unwrap_or("model"));
```

Replace with:

```rust
    let group_by = match q.group_by.as_deref() {
        None => crate::usage::GroupBy::Model,
        Some(g) => crate::usage::GroupBy::parse(g).ok_or_else(|| {
            ApiError::bad_request(format!(
                "unknown group_by {g:?}; expected provider | model | agent | workflow | host | project"
            ))
        })?,
    };
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-cp usage`
Expected: PASS.

Run: `cargo clippy -p rupu-cp --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/usage.rs
git add crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/usage.rs
git commit -m "feat(cp): GroupBy gains workflow/host/project; parse stops silently defaulting

parse() was infallible with '_ => Model', so ?group_by=workflw silently
returned a model breakdown and the caller never learned their pivot was
ignored. Now returns Option and the handler 400s.

Note: agent attribution already existed (GroupBy::Agent) -- the spec deferred
it unnecessarily. It was simply never exposed in the UI."
```

---

### Task 2: Attribute usage rows to workflow / host / project

**Files:**
- Modify: `crates/rupu-cp/src/usage.rs`
- Modify: `crates/rupu-cp/src/api/usage.rs`
- Test: `crates/rupu-cp/src/usage.rs`

**Interfaces:**
- Consumes: `RunStore`, `RunRecord` (for `workflow_name`, `workspace_id`)
- Produces: `attribute_rows(rows: &mut [UsageRow], store: &RunStore, host_id: &str)` — populates `workflow`, `host_id`, `workspace_id` on each row by joining on `run_id`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn attribute_rows_joins_workflow_name_from_the_run_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        // Seed a run whose workflow_name we expect to land on the usage row.
        let mut rec = rupu_orchestrator::runs::RunRecord::default();
        rec.id = "run_1".into();
        rec.workflow_name = "nightly-review".into();
        rec.workspace_id = Some("ws_a".into());
        store.save(&rec).unwrap();

        let mut rows = vec![UsageRow {
            run_id: "run_1".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            agent: "reviewer".into(),
            ..Default::default()
        }];

        attribute_rows(&mut rows, &store, "local");

        assert_eq!(rows[0].workflow, "nightly-review");
        assert_eq!(rows[0].workspace_id, "ws_a");
        assert_eq!(rows[0].host_id, "local");
    }

    #[test]
    fn attribute_rows_leaves_unknown_runs_blank_not_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let mut rows = vec![UsageRow {
            run_id: "run_missing".into(),
            ..Default::default()
        }];

        attribute_rows(&mut rows, &store, "local");

        // A run we cannot resolve attributes to nothing. It must NOT silently
        // land in some other workflow's bucket.
        assert_eq!(rows[0].workflow, "");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp attribute_rows_joins_workflow_name`
Expected: FAIL — `cannot find function 'attribute_rows'`

- [ ] **Step 3: Implement**

```rust
/// Populate the joined attribution dimensions on each usage row.
///
/// `UsageRow` comes from transcripts, which know `run_id`, `agent`, `provider`,
/// `model`, and `workspace_id` — but not `workflow_name`, which lives on the
/// `RunRecord`. One store read per distinct run, cached in a map: a naive
/// per-row lookup would re-read the same run.json for every token row it
/// produced.
///
/// Rows whose run cannot be resolved are left BLANK, never bucketed under a
/// fallback. Attributing spend to the wrong workflow is worse than attributing
/// it to none.
pub fn attribute_rows(rows: &mut [UsageRow], store: &RunStore, host_id: &str) {
    use std::collections::HashMap;

    let mut cache: HashMap<String, Option<(String, String)>> = HashMap::new();

    for row in rows.iter_mut() {
        row.host_id = host_id.to_string();
        if row.run_id.is_empty() {
            continue;
        }
        let resolved = cache
            .entry(row.run_id.clone())
            .or_insert_with(|| {
                store.load(&row.run_id).ok().map(|rec| {
                    (
                        rec.workflow_name.clone(),
                        rec.workspace_id.clone().unwrap_or_default(),
                    )
                })
            })
            .clone();

        if let Some((workflow, workspace_id)) = resolved {
            row.workflow = workflow;
            // Prefer the transcript's workspace_id if it already set one.
            if row.workspace_id.is_empty() {
                row.workspace_id = workspace_id;
            }
        }
    }
}
```

Call it in `crates/rupu-cp/src/api/usage.rs`, after rows are collected and before `breakdown(...)`:

```rust
    let mut all_rows = all_rows;
    // Only pay the run-store join when the pivot actually needs it.
    if !group_by.is_intrinsic() {
        crate::usage::attribute_rows(&mut all_rows, &s.run_store, "local");
    }
    let breakdown = crate::usage::breakdown(&all_rows, &s.pricing, group_by);
```

**Implementer note:** if `RunStore::load` is named differently (e.g. `get` / `read`), match the actual API — check `rupu-orchestrator/src/runs.rs`. If `UsageRow` has no `Default`, construct the test fixtures with explicit fields rather than adding a `Default` impl for a test's convenience.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp attribute_rows`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/usage.rs
git add crates/rupu-cp/src/usage.rs crates/rupu-cp/src/api/usage.rs
git commit -m "feat(cp): attribute usage rows to workflow/host/project

Joins UsageRow -> RunRecord on run_id, cached per distinct run so a run with
many token rows costs one store read.

Unresolvable runs attribute to BLANK, never a fallback bucket: attributing
spend to the wrong workflow is worse than attributing it to none.

The join only runs for non-intrinsic pivots."
```

---

### Task 3: Unpriced gap as an explicit number + host fan-out

**Files:**
- Modify: `crates/rupu-cp/src/api/usage.rs`
- Test: `crates/rupu-cp/tests/usage.rs` (create if absent)

**Interfaces:**
- Produces: `GET /api/usage?since=&group_by=&host=` returning `{ summary, breakdown, unpriced: UnpricedGap, hosts: Vec<HostFreshness> }` where `UnpricedGap = { models: Vec<String>, rows: u64 }`

**Why:** today a `*` footnote marks partial spend when some models lack a price (`UsageSummary.priced == false`). On a dedicated attribution page that footnote must become a real number — `$12.40 known · 3 models unpriced` — because an attribution page that silently under-counts is worse than no page. Same rule as the SSH `list_runs` fix.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn usage_reports_unpriced_models_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    // Seed a transcript using a model with no configured price.
    seed_transcript_with_model(dir.path(), "run_1", "some-unpriced-model");
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let unpriced = body["unpriced"]["models"].as_array().unwrap();
    assert!(
        unpriced.iter().any(|m| m == "some-unpriced-model"),
        "an unpriced model must be named, not hidden behind a '*'"
    );
    assert!(body["unpriced"]["rows"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn usage_rejects_unknown_group_by() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/usage?group_by=workflw", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "a typo must 400, not silently return a model breakdown");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp usage_reports_unpriced_models_explicitly`
Expected: FAIL — the response has no `unpriced` key.

- [ ] **Step 3: Implement**

Add to `crates/rupu-cp/src/api/usage.rs`:

```rust
/// The models we could not price, named.
///
/// `UsageSummary.priced == false` tells you spend is partial but not by how
/// much or because of what. On an attribution page that is not good enough: a
/// silent under-count is worse than no number.
#[derive(Serialize, Default)]
struct UnpricedGap {
    /// Distinct model ids with no resolvable price.
    models: Vec<String>,
    /// How many token rows those models account for.
    rows: u64,
}

fn unpriced_gap(rows: &[crate::usage::UsageRow], pricing: &rupu_config::PricingConfig) -> UnpricedGap {
    use std::collections::BTreeSet;
    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut count = 0u64;
    for r in rows {
        if pricing.price_for(&r.provider, &r.model).is_none() {
            models.insert(r.model.clone());
            count += 1;
        }
    }
    UnpricedGap {
        models: models.into_iter().collect(),
        rows: count,
    }
}
```

Add `unpriced: unpriced_gap(&all_rows, &s.pricing)` to the response struct and its construction.

**Implementer note:** `pricing.price_for(provider, model)` is a guess at the API. Read `rupu-config`'s `PricingConfig` and use the real lookup — `crate::usage`'s own pricing path (around `usage.rs:40-56`, where `any_priced` is computed) already resolves prices; reuse that function rather than duplicating the lookup logic.

- [ ] **Step 4: Fan out across hosts**

Mirror Plan 2 Task 7's shape: accept `?host=`, and when absent, fan out. Spend that is local-only is wrong for the same reason the dashboard was.

```rust
    // Same rule as /api/dashboard: a host that cannot report contributes
    // NOTHING rather than zeros, and its state is carried in `hosts`.
```

**Implementer note:** remote usage arrives via `HttpHostConnector::proxy_get_json("/api/usage?host=local&...")` for HTTP hosts. SSH hosts have no usage CLI surface, so they report `Unsupported` and render unavailable — do **not** silently omit them from the `hosts` array. If adding an SSH usage surface proves necessary, that is a follow-up, not this task.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-cp usage`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/api/usage.rs crates/rupu-cp/tests/usage.rs
git add crates/rupu-cp/src/api/usage.rs crates/rupu-cp/tests/usage.rs
git commit -m "feat(cp): name unpriced models explicitly; fan /api/usage out across hosts

'priced: false' told you spend was partial but not by how much or because of
what. The attribution page needs the real number -- a silent under-count is
worse than no number.

Hosts that cannot report usage render unavailable, never zeroed."
```

---

### Task 4: Cost outliers

**Files:**
- Create: `crates/rupu-cp/src/api/usage_outliers.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs`, `crates/rupu-cp/src/server.rs`
- Test: `crates/rupu-cp/src/api/usage_outliers.rs`

**Interfaces:**
- Produces: `GET /api/usage/outliers?since=` → `Vec<OutlierRun> { run_id, workflow_name, cost_usd, baseline_usd, ratio, started_at }`

**Why:** anomaly is nearly free once per-dimension cost over time exists, and it is the panel that turns attribution into action.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlier_is_relative_to_its_own_workflow_baseline() {
        // A workflow that normally costs $1 spiking to $10 is an outlier. A
        // workflow that always costs $10 is not — an absolute threshold would
        // flag it forever.
        let runs = vec![
            ("cheap-wf", "r1", 1.0),
            ("cheap-wf", "r2", 1.0),
            ("cheap-wf", "r3", 1.0),
            ("cheap-wf", "spike", 10.0),
            ("pricey-wf", "p1", 10.0),
            ("pricey-wf", "p2", 10.0),
            ("pricey-wf", "p3", 10.0),
        ];
        let out = find_outliers(&to_fixtures(runs), 3.0);
        let ids: Vec<_> = out.iter().map(|o| o.run_id.as_str()).collect();
        assert_eq!(ids, vec!["spike"]);
    }

    #[test]
    fn a_workflow_with_too_few_runs_yields_no_outliers() {
        // One run is not a baseline. Flagging it would make every new workflow
        // look anomalous on its first run.
        let out = find_outliers(&to_fixtures(vec![("new-wf", "r1", 99.0)]), 3.0);
        assert!(out.is_empty());
    }

    #[test]
    fn unpriced_runs_are_not_outliers() {
        // cost_usd: None means "we don't know", not "free". It must not be
        // treated as 0 and it must not be flagged.
        let out = find_outliers(&[RunCost {
            run_id: "r1".into(),
            workflow_name: "wf".into(),
            cost_usd: None,
            started_at: chrono::Utc::now(),
        }], 3.0);
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp outlier_is_relative_to_its_own_workflow`
Expected: FAIL — `cannot find function 'find_outliers'`

- [ ] **Step 3: Implement**

```rust
//! Cost outliers: runs that cost far more than their workflow normally does.
//!
//! Baseline is PER WORKFLOW, not global. An absolute threshold would flag an
//! expensive-by-design workflow forever and never flag a cheap one that
//! regressed 10x — the opposite of useful.

#![deny(clippy::all)]

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct RunCost {
    pub run_id: String,
    pub workflow_name: String,
    /// `None` = unpriced. NOT zero — we do not know what it cost.
    pub cost_usd: Option<f64>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct OutlierRun {
    pub run_id: String,
    pub workflow_name: String,
    pub cost_usd: f64,
    pub baseline_usd: f64,
    pub ratio: f64,
    pub started_at: DateTime<Utc>,
}

/// A workflow needs at least this many priced runs before it has a baseline.
/// Below it, every new workflow's first run would look anomalous.
const MIN_BASELINE_RUNS: usize = 3;

/// Median — robust to the very outliers we are hunting, unlike a mean, which
/// a single 100x spike drags upward until it stops flagging anything.
fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

/// Find runs costing more than `threshold`x their workflow's median.
pub fn find_outliers(runs: &[RunCost], threshold: f64) -> Vec<OutlierRun> {
    use std::collections::HashMap;

    let mut by_wf: HashMap<&str, Vec<&RunCost>> = HashMap::new();
    for r in runs {
        // Unpriced runs contribute to neither the baseline nor the results:
        // None means unknown, and averaging unknown as 0 would drag every
        // baseline down and manufacture outliers.
        if r.cost_usd.is_some() {
            by_wf.entry(r.workflow_name.as_str()).or_default().push(r);
        }
    }

    let mut out = Vec::new();
    for (_wf, wf_runs) in by_wf {
        if wf_runs.len() < MIN_BASELINE_RUNS {
            continue;
        }
        let baseline = median(wf_runs.iter().filter_map(|r| r.cost_usd).collect());
        if baseline <= 0.0 {
            continue;
        }
        for r in wf_runs {
            let cost = r.cost_usd.unwrap_or(0.0);
            let ratio = cost / baseline;
            if ratio >= threshold {
                out.push(OutlierRun {
                    run_id: r.run_id.clone(),
                    workflow_name: r.workflow_name.clone(),
                    cost_usd: cost,
                    baseline_usd: baseline,
                    ratio,
                    started_at: r.started_at,
                });
            }
        }
    }
    out.sort_by(|a, b| b.ratio.partial_cmp(&a.ratio).unwrap_or(std::cmp::Ordering::Equal));
    out
}
```

Add the handler and register the route in `server.rs` alongside the other `/api/usage*` routes.

**Implementer note:** the tests reference a `to_fixtures(Vec<(&str, &str, f64)>) -> Vec<RunCost>` helper. Write it in the test module — it maps tuples to `RunCost` with `chrono::Utc::now()` as `started_at`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rupu-cp outlier`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/api/usage_outliers.rs crates/rupu-cp/src/server.rs
git add crates/rupu-cp/src/api/usage_outliers.rs crates/rupu-cp/src/server.rs crates/rupu-cp/src/api/mod.rs
git commit -m "feat(cp): per-workflow cost outliers

Baseline is per workflow and uses a MEDIAN: an absolute threshold flags an
expensive-by-design workflow forever, and a mean gets dragged up by the very
spikes we hunt until it stops flagging anything.

Unpriced runs are excluded from both baseline and results -- None means
unknown, and averaging it as 0 would manufacture outliers."
```

---

### Task 5: The spend page

**Files:**
- Create: `crates/rupu-cp/web/src/pages/Usage.tsx`
- Create: `crates/rupu-cp/web/src/components/usage/PivotPicker.tsx`
- Create: `crates/rupu-cp/web/src/components/usage/UnpricedBanner.tsx`
- Create: `crates/rupu-cp/web/src/components/usage/OutlierPanel.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx` (add the `/usage` route)
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (types + client)
- Test: `crates/rupu-cp/web/src/components/usage/UnpricedBanner.test.tsx`
- Test: `crates/rupu-cp/web/src/pages/Usage.test.tsx`

**Interfaces:**
- Consumes: Tasks 1–4's endpoints; reuses `UsageTimelineStacked` and `ModelBreakdownTable` (Plan 1 left them in place)

**Layout:** range control + pivot picker in the header → unpriced banner (only when non-zero) → headline spend + `UsageTimelineStacked` pivoted by the chosen dimension → breakdown table → outlier panel. Trend is not a separate section — the pivoted timeline *is* the trend.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { UnpricedBanner } from './UnpricedBanner';

describe('UnpricedBanner', () => {
  it('names the unpriced models rather than showing a bare asterisk', () => {
    render(<UnpricedBanner unpriced={{ models: ['mystery-model', 'other-model'], rows: 42 }} />);
    expect(screen.getByText(/mystery-model/)).toBeInTheDocument();
    expect(screen.getByText(/2 models unpriced/)).toBeInTheDocument();
  });

  it('renders nothing when everything is priced', () => {
    const { container } = render(<UnpricedBanner unpriced={{ models: [], rows: 0 }} />);
    expect(container).toBeEmptyDOMElement();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/usage/UnpricedBanner.test.tsx`
Expected: FAIL — `Cannot find module './UnpricedBanner'`

- [ ] **Step 3: Implement the components**

```tsx
// UnpricedBanner — the spend we cannot account for, stated plainly.
//
// This was a '*' footnote. On an attribution page that is not good enough: if
// some models have no price, the headline number is an UNDER-COUNT, and a
// number that is quietly wrong is worse than no number.

export interface UnpricedGap {
  models: string[];
  rows: number;
}

export function UnpricedBanner({ unpriced }: { unpriced: UnpricedGap }) {
  if (unpriced.models.length === 0) return null;
  return (
    <div className="rounded-lg border border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))] px-4 py-2 text-sm">
      <span className="font-medium text-[rgb(var(--c-ink))]">
        {unpriced.models.length} model{unpriced.models.length === 1 ? '' : 's'} unpriced
      </span>
      <span className="text-[rgb(var(--c-ink-dim))]">
        {' '}
        — spend below excludes {unpriced.rows} token row
        {unpriced.rows === 1 ? '' : 's'} from {unpriced.models.join(', ')}
      </span>
    </div>
  );
}
```

```tsx
// PivotPicker — the attribution dimension.
//
// 'This autoflow costs $40/night' was unanswerable when group_by was
// model-only. That is the actionable question: attribution is what lets you
// change something.

export type Pivot = 'model' | 'provider' | 'agent' | 'workflow' | 'host' | 'project';

const PIVOTS: Pivot[] = ['model', 'provider', 'agent', 'workflow', 'host', 'project'];

export function PivotPicker({ value, onChange }: { value: Pivot; onChange: (p: Pivot) => void }) {
  return (
    <div className="flex rounded-md border border-[rgb(var(--c-border))]">
      {PIVOTS.map((p) => (
        <button
          key={p}
          onClick={() => onChange(p)}
          className={`px-2 py-1 text-xs capitalize ${
            value === p
              ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
              : 'text-[rgb(var(--c-ink-mute))]'
          }`}
        >
          {p}
        </button>
      ))}
    </div>
  );
}
```

```tsx
// OutlierPanel — runs that cost far more than their workflow normally does.

import { Link } from 'react-router-dom';

export interface OutlierRun {
  run_id: string;
  workflow_name: string;
  cost_usd: number;
  baseline_usd: number;
  ratio: number;
  started_at: string;
}

export function OutlierPanel({ outliers }: { outliers: OutlierRun[] }) {
  if (outliers.length === 0) {
    return (
      <div className="p-4 text-sm text-[rgb(var(--c-ink-mute))]">
        No cost outliers in this window
      </div>
    );
  }
  return (
    <ul className="divide-y divide-[rgb(var(--c-border))]">
      {outliers.map((o) => (
        <li key={o.run_id} className="flex items-center gap-3 px-3 py-2 text-sm">
          <Link to={`/runs/${o.run_id}`} className="font-medium text-[rgb(var(--c-ink))]">
            {o.workflow_name}
          </Link>
          <span className="text-xs text-[rgb(var(--c-ink-mute))]">{o.run_id}</span>
          <span className="ml-auto tabular-nums text-[rgb(var(--c-ink))]">
            ${o.cost_usd.toFixed(2)}
          </span>
          <span className="tabular-nums text-[rgb(var(--c-status-failed))]">
            {o.ratio.toFixed(1)}× baseline (${o.baseline_usd.toFixed(2)})
          </span>
        </li>
      ))}
    </ul>
  );
}
```

- [ ] **Step 4a: Add the Dashboard's "Spend →" link — plan 1 deliberately left it out**

Plan 1 Task 6 omitted this link because `/usage` did not exist yet and a link to a 404 is worse
than no link. **Spend is currently ABSENT from the dashboard** — this step ends that regression, so
do not skip it. In `crates/rupu-cp/web/src/pages/Dashboard.tsx`'s header, beside the range control:

```tsx
          <Link
            to="/usage"
            className="rounded-md border border-[rgb(var(--c-border))] px-3 py-1 text-xs text-[rgb(var(--c-ink-dim))] hover:text-[rgb(var(--c-ink))]"
          >
            Spend →
          </Link>
```

Add `import { Link } from 'react-router-dom';` to that file if absent. Verify the link resolves in
a browser — that is the whole point of this step.

- [ ] **Step 4: Implement the page and route**

Create `crates/rupu-cp/web/src/pages/Usage.tsx` composing: header (range control + `PivotPicker`), `UnpricedBanner`, headline spend + `UsageTimelineStacked` (pivoted), `ModelBreakdownTable` (retitled to the active pivot), `OutlierPanel`. Add types + `api.getUsage(range, pivot)` / `api.getUsageOutliers(range)` to `api.ts`, and register the lazy route in `App.tsx` next to the existing ones:

```tsx
const Usage = React.lazy(() => import('./pages/Usage'));
// ...
<Route path="/usage" element={<Usage />} />
```

**Implementer note:** `UsageTimelineStacked` and `ModelBreakdownTable` currently assume a model dimension. Read them first. If their prop names are model-specific (e.g. `byModel`), generalize the prop to a neutral `series` / `rows` name rather than adding a parallel component — but keep `modelColors.ts` as the color source for the model pivot specifically, since those colors are model-identity, not arbitrary categories. For non-model pivots, fall back to a themed categorical ramp from `useThemeColors()`.

- [ ] **Step 5: Run tests + typecheck + build**

Run: `cd crates/rupu-cp/web && npx vitest run`
Expected: PASS.

Run: `npx tsc --noEmit && npm run build`
Expected: clean, SUCCESS.

- [ ] **Step 6: Runtime validation — REQUIRED**

```bash
cargo run -p rupu-cli -- cp serve   # terminal 1
cd crates/rupu-cp/web && npm run dev  # terminal 2
```

Open `http://127.0.0.1:5173/usage` and confirm:
- [ ] Every pivot (model / provider / agent / workflow / host / project) returns data and re-renders the timeline and table.
- [ ] The `workflow` pivot answers "what does this autoflow cost" — the question that motivated the page.
- [ ] The unpriced banner appears when models lack prices and names them; it is absent when everything is priced.
- [ ] The outlier panel lists real runs and links to `/runs/:id`.
- [ ] The Dashboard's "Spend →" link lands here.
- [ ] Light **and** dark themes both read correctly.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Usage.tsx crates/rupu-cp/web/src/pages/Usage.test.tsx crates/rupu-cp/web/src/components/usage/ crates/rupu-cp/web/src/App.tsx crates/rupu-cp/web/src/lib/api.ts
git commit -m "feat(cp-web): dedicated spend page with attribution pivots and outliers

Pivot by model/provider/agent/workflow/host/project. 'This autoflow costs
\$40/night' was unanswerable when group_by was model-only.

Trend is not a separate section: a pivot with a time axis IS the trend view.

The unpriced '*' footnote is now a named, counted banner."
```

---

## Plan 3 Definition of Done

- [ ] `cargo test -p rupu-cp` passes; clippy clean.
- [ ] `npx vitest run`, `npx tsc --noEmit`, `npm run build` all pass.
- [ ] `?group_by=` with a typo returns **400**, not a silent model breakdown.
- [ ] All six pivots return data; the `workflow` pivot answers "what does this autoflow cost".
- [ ] Unpriced models are **named and counted**, never a bare `*`.
- [ ] Outliers are per-workflow and median-based; unpriced runs never appear.
- [ ] Hosts that cannot report usage render unavailable, never zeroed.
- [ ] Browser-validated in both themes.
- [ ] No new dependencies.
