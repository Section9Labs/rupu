# Dashboard Fleet Strip — Plan 1: contract + local counts + strip UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `FleetCounts` block to `DashboardSummary`, fill the counts that come from `<global_dir>` alone, merge them with the `findings_open` partial discipline, and render the fleet strip on the Dashboard.

**Architecture:** `FleetCounts` is a new field on the existing `DashboardSummary`, so it rides the per-host `HostConnector::dashboard_summary` fan-out already in place — no new connector method. `LocalHostConnector` fills the counts sourceable from `<global_dir>` (autoflows enabled/disabled, workers, active claims); every other field stays `None` for Plans 2 and 3 to fill. `SshHostConnector` reports `FleetCounts::default()` (all `None`). Both the server merge (`api::dashboard`) and its client-side twin (`mergeSummaries.ts`) sum only `Some` values and raise a single `fleet_partial` flag.

**Tech Stack:** Rust 2021 (axum, serde, chrono, thiserror), React + TypeScript, vitest, Tailwind.

## Spec

`docs/superpowers/specs/2026-07-30-rupu-dashboard-fleet-strip-design.md`

## Plan decomposition

The spec spans three subsystems. Each plan ships working, testable software:

- **Plan 1 (this document)** — the `FleetCounts` contract, the counts readable from `<global_dir>`, the merge, and the strip UI. After this plan the strip is live and honest: it shows autoflows / workers / claims, and renders an em-dash for everything not yet sourced.
- **Plan 2** — `LlmProvider::probe()`, the `ProviderProbe` port, the probe cache, and `cp serve` wiring. Fills `providers_configured` and `providers_unhealthy`. Both live here rather than in Plan 1 because the honest source for "configured" is the credential store, which sits behind rupu-providers — a crate rupu-cp deliberately does not depend on.
- **Plan 3** — repos, open-issue totals with a per-repo cap, and the pending backlog derived through the cron tick's own matcher. Fills `repos`, `issues_pending`, `issues_open`, `issues_capped`. (No separate `IssueLister` port: Plan 2's snapshot already carries these across the same boundary — see Plan 3's deviation note.)

## Global Constraints

- Workspace deps only — versions pinned in the root `Cargo.toml`, never in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-cli` stays thin: arg parsing + delegation, no business logic. Nothing in this plan touches it.
- Errors: `thiserror` in libraries, `anyhow` in the CLI binary.
- **Never run `cargo fmt` in any form — including `cargo fmt -- <path>`.** That form reads like a per-file filter but resolves the whole workspace and treats the paths as rustfmt flags; `main` is fmt-dirty under the pinned toolchain, so it rewrites ~100 unrelated files. Use `rustfmt --edition 2021 <file>`, one file at a time, and `git diff` each to confirm it touched only your additions.
- **A host that cannot report is not a host with zero.** Every count added here is `Option<u64>`; `None` is never summed as `0`, and any `None` from a reporting host raises `fleet_partial`.
- Web tests run from `crates/rupu-cp/web` with `npm test`. Rust tests run with `cargo test -p rupu-cp`.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/rupu-cp/src/host/dashboard_summary.rs` | **Modify.** Add the `FleetCounts` DTO and the `fleet` field on `DashboardSummary`. |
| `crates/rupu-cp/src/host/summary_build.rs` | **Modify.** `build_summary` takes `fleet` and passes it through. |
| `crates/rupu-cp/src/host/fleet_counts.rs` | **Create.** Pure-ish collector: reads `<global_dir>` and returns `FleetCounts`. Kept out of `local.rs` (already 450+ lines) so the collection logic is testable on its own against a tempdir. |
| `crates/rupu-cp/src/host/local.rs` | **Modify.** Call the collector, pass the result to `build_summary`. |
| `crates/rupu-cp/src/host/ssh.rs` | **Modify.** Report `FleetCounts::default()`. |
| `crates/rupu-cp/src/host/mod.rs` | **Modify.** Register `fleet_counts`. |
| `crates/rupu-cp/src/api/dashboard.rs` | **Modify.** Merge `fleet`, add `fleet_partial` to the response. |
| `crates/rupu-cp/web/src/lib/api.ts` | **Modify.** `FleetCounts` interface, `fleet` on `DashboardSummary`, `fleet_partial` on `DashboardResponse`. |
| `crates/rupu-cp/web/src/lib/dashboard/mergeSummaries.ts` | **Modify.** Client-side fleet merge. |
| `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts` | **Modify.** Derive `fleet_partial`. |
| `crates/rupu-cp/web/src/components/dashboard/FleetStrip.tsx` | **Create.** The strip. |
| `crates/rupu-cp/web/src/components/dashboard/FleetStrip.test.tsx` | **Create.** Its tests. |
| `crates/rupu-cp/web/src/pages/Dashboard.tsx` | **Modify.** Render the strip. |

## Deviation from the spec

The spec names the claims field `claims_queued`. `ClaimStatus` (`crates/rupu-workspace/src/autoflow_claim.rs`) has no `Queued` variant — its variants are `Eligible`, `Claimed`, `Running`, `AwaitHuman`, `AwaitExternal`, `RetryBackoff`, `Blocked`, `Complete`, `Released`. The field is therefore named **`claims_active`** and counts every claim not in `Complete` or `Released`. The UI label stays "claimed". No other spec field is renamed.

---

### Task 1: `FleetCounts` DTO

**Files:**
- Modify: `crates/rupu-cp/src/host/dashboard_summary.rs`

**Interfaces:**
- Produces: `pub struct FleetCounts` with fields `repos`, `providers_configured`, `providers_unhealthy`, `autoflows_enabled`, `autoflows_disabled`, `workers`, `claims_active` (all `Option<u64>`), `issues_pending`, `issues_open` (`Option<u64>`), `issues_capped: bool`, `inventory_captured_at: Option<DateTime<Utc>>`. Derives `Default`. `DashboardSummary` gains `pub fleet: FleetCounts` carrying `#[serde(default)]`.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` block at the bottom of `crates/rupu-cp/src/host/dashboard_summary.rs`:

```rust
    #[test]
    fn fleet_counts_default_is_all_none() {
        let f = FleetCounts::default();
        assert_eq!(f.repos, None);
        assert_eq!(f.providers_configured, None);
        assert_eq!(f.providers_unhealthy, None);
        assert_eq!(f.autoflows_enabled, None);
        assert_eq!(f.autoflows_disabled, None);
        assert_eq!(f.workers, None);
        assert_eq!(f.claims_active, None);
        assert_eq!(f.issues_pending, None);
        assert_eq!(f.issues_open, None);
        assert!(!f.issues_capped);
        assert!(f.inventory_captured_at.is_none());
    }

    /// A summary produced by an OLDER rupu (no `fleet` key at all) must still
    /// deserialize — `HttpHostConnector` parses remote-CP bodies as a bare
    /// `DashboardSummary`, so a missing key here would take the whole remote
    /// host offline rather than degrade one strip segment.
    #[test]
    fn summary_without_a_fleet_key_deserializes_to_default() {
        let json = serde_json::json!({
            "active": {"running": 1, "awaiting_approval": 0, "paused": 0, "pending": 0},
            "terminal_buckets": [],
            "throughput_buckets": [],
            "cycles": {"total": 0, "clean": null, "with_failures": null},
            "findings_open": null,
            "captured_at": "2026-07-30T00:00:00Z",
        });
        let sum: DashboardSummary = serde_json::from_value(json).expect("must deserialize");
        assert_eq!(
            sum.fleet.workers, None,
            "a pre-fleet host must degrade to all-None, never to fabricated zeros"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp fleet_counts_default_is_all_none summary_without_a_fleet_key`
Expected: FAIL — `cannot find type FleetCounts in this scope`.

- [ ] **Step 3: Add the type and the field**

In `crates/rupu-cp/src/host/dashboard_summary.rs`, insert immediately above `pub struct DashboardSummary`:

```rust
/// One host's inventory contribution — the fleet strip beneath the ops
/// blocks (spec §2).
///
/// Every count is `Option<u64>` for the same reason `findings_open` is: a
/// host that cannot source a field is NOT a host whose count is zero. The
/// aggregation layer (`api::dashboard`) sums only `Some` values and raises
/// `fleet_partial` when any reporting host contributed `None`.
///
/// Fields are filled across three plans. Plan 1 fills the ones readable from
/// `<global_dir>` alone (`autoflows_*`, `workers`, `claims_active`); Plan 2
/// fills the `providers_*` pair; Plan 3 fills `repos` and the `issues_*`
/// family. An unfilled field is `None` and renders as an em-dash — never as a
/// zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetCounts {
    /// Repos visible to the SCM connector. Plan 3.
    pub repos: Option<u64>,
    /// Providers rupu holds usable credentials for. Plan 2.
    ///
    /// Deliberately NOT `config.providers.len()`: that map holds per-provider
    /// knob overrides (`base_url`, `models`, …), so an operator authenticated
    /// to Anthropic via OAuth with no `[providers.anthropic]` block would
    /// count as zero. The honest source is the credential store, which lives
    /// behind rupu-providers — a crate rupu-cp does not depend on. It
    /// therefore arrives through the Plan 2 port, not from a config read.
    pub providers_configured: Option<u64>,
    /// Providers whose last probe failed (auth or reachability). Plan 2.
    /// `None` without a probe cache — the strip then claims nothing about
    /// health, which is the point: config presence is not evidence a
    /// provider works.
    pub providers_unhealthy: Option<u64>,
    /// Workflow definitions carrying `autoflow.enabled: true`.
    pub autoflows_enabled: Option<u64>,
    /// Workflow definitions carrying `autoflow.enabled: false`.
    pub autoflows_disabled: Option<u64>,
    /// Registered local worker identities.
    pub workers: Option<u64>,
    /// Tracked autoflow claims that are still in flight — every
    /// `ClaimStatus` except `Complete` and `Released`. Named `_active`
    /// rather than the spec's `_queued` because `ClaimStatus` has no
    /// `Queued` variant.
    pub claims_active: Option<u64>,
    /// Issues matched by an enabled autoflow's selector and not yet
    /// claimed. Plan 3.
    pub issues_pending: Option<u64>,
    /// Open issues across connected repos. Plan 3.
    pub issues_open: Option<u64>,
    /// True when a repo hit the per-repo issue fetch cap, making
    /// `issues_open` a floor rather than a total. ORs across hosts at the
    /// merge. Plan 3.
    #[serde(default)]
    pub issues_capped: bool,
    /// When the SCM / provider caches behind these numbers were filled.
    /// Deliberately distinct from `DashboardSummary::captured_at`, which
    /// stamps the run-store read — the inventory is minutes-stale by design
    /// while the run data is seconds-stale. Plans 2 and 3.
    pub inventory_captured_at: Option<DateTime<Utc>>,
}
```

Then add the field to `DashboardSummary`, immediately after `findings_open`:

```rust
    /// This host's inventory contribution. `#[serde(default)]` is
    /// load-bearing: `HttpHostConnector` parses a remote CP's body as a bare
    /// `DashboardSummary`, and a remote running an older rupu emits no
    /// `fleet` key at all. Without the default that host would fail to parse
    /// and drop off the dashboard entirely instead of degrading to an
    /// all-`None` strip contribution.
    #[serde(default)]
    pub fleet: FleetCounts,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp fleet_counts_default_is_all_none summary_without_a_fleet_key`
Expected: 2 passed. Other crates will not compile yet — `DashboardSummary` now has a field every constructor must supply. That is Task 2's job.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/host/dashboard_summary.rs
git add crates/rupu-cp/src/host/dashboard_summary.rs
git commit -m "feat(cp): FleetCounts DTO on DashboardSummary"
```

---

### Task 2: Collect the local counts

**Files:**
- Create: `crates/rupu-cp/src/host/fleet_counts.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs`
- Modify: `crates/rupu-cp/src/host/summary_build.rs:50-56`
- Modify: `crates/rupu-cp/src/host/local.rs:360-389`
- Modify: `crates/rupu-cp/src/host/ssh.rs:1430`

**Interfaces:**
- Consumes: `FleetCounts` from Task 1.
- Produces: `pub fn collect_fleet_counts(global_dir: &std::path::Path) -> FleetCounts`. `build_summary` gains a `fleet: FleetCounts` parameter, inserted **after** `findings_open` and **before** `range`: `build_summary(runs, cycles, findings_open, fleet, range, now)`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/src/host/fleet_counts.rs` containing ONLY the test module and the module doc for now:

```rust
//! Collects this host's [`FleetCounts`] from `<global_dir>`.
//!
//! Split out of `local.rs` (already long) so the collection can be exercised
//! against a tempdir without standing up a `LocalHostConnector`. Every reader
//! here degrades a failure to `None` + a `tracing::warn!` — never to a zero,
//! which would make a broken store indistinguishable from an empty one.

#![deny(clippy::all)]

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty global dir has no stores at all. Every count must be a
    /// genuine `Some(0)` — the dirs are absent because nothing has been
    /// created yet, which IS zero — except the fields Plans 2 and 3 own,
    /// which stay `None`.
    #[test]
    fn empty_global_dir_yields_zeros_for_owned_fields_and_none_for_deferred() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = collect_fleet_counts(tmp.path());
        assert_eq!(f.workers, Some(0));
        assert_eq!(f.claims_active, Some(0));
        assert_eq!(f.autoflows_enabled, Some(0));
        assert_eq!(f.autoflows_disabled, Some(0));
        assert_eq!(f.repos, None, "Plan 3 owns repos");
        assert_eq!(
            f.providers_configured, None,
            "Plan 2 owns providers — the credential store is behind rupu-providers, \
             and config.providers is a knob-override map, not the usable set"
        );
        assert_eq!(f.providers_unhealthy, None, "Plan 2 owns provider health");
        assert_eq!(f.issues_pending, None, "Plan 3 owns issues");
    }

    /// Enabled and disabled autoflow definitions are counted separately, and
    /// a workflow with no `autoflow:` block at all is not an autoflow.
    #[test]
    fn autoflow_defs_split_by_enabled_flag() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let wf = tmp.path().join("workflows");
        std::fs::create_dir_all(&wf).expect("mkdir");
        std::fs::write(
            wf.join("on.yaml"),
            "name: on\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n",
        )
        .expect("write");
        std::fs::write(
            wf.join("off.yaml"),
            "name: off\nautoflow:\n  enabled: false\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n",
        )
        .expect("write");
        std::fs::write(
            wf.join("plain.yaml"),
            "name: plain\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n",
        )
        .expect("write");

        let f = collect_fleet_counts(tmp.path());
        assert_eq!(f.autoflows_enabled, Some(1));
        assert_eq!(
            f.autoflows_disabled,
            Some(1),
            "a workflow with no autoflow: block is not a disabled autoflow — it is not an autoflow"
        );
    }

    /// `Complete` and `Released` claims are finished work, not in-flight
    /// work; only the rest count toward `claims_active`.
    #[test]
    fn claims_active_excludes_complete_and_released() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claims = tmp.path().join("autoflows").join("claims");
        for (slug, status) in [
            ("a", "running"),
            ("b", "await_human"),
            ("c", "complete"),
            ("d", "released"),
        ] {
            let dir = claims.join(slug);
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(
                dir.join("claim.toml"),
                format!(
                    "issue_ref = \"github:o/r/issues/1\"\n\
                     repo_ref = \"github:o/r\"\n\
                     workflow = \"wf\"\n\
                     status = \"{status}\"\n\
                     updated_at = \"2026-07-30T00:00:00Z\"\n"
                ),
            )
            .expect("write");
        }

        let f = collect_fleet_counts(tmp.path());
        assert_eq!(
            f.claims_active,
            Some(2),
            "only running + await_human are in flight"
        );
    }
}
```

> **Note for the implementer:** run the claims test first against the real
> `AutoflowClaimRecord` shape (`crates/rupu-workspace/src/autoflow_claim.rs`).
> If a required field is missing from the TOML above, add it to the fixture —
> do not change the assertion.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp fleet_counts::`
Expected: FAIL — the module is not registered and `collect_fleet_counts` does not exist.

- [ ] **Step 3: Implement the collector**

Register the module in `crates/rupu-cp/src/host/mod.rs`, next to the existing `pub mod dashboard_summary;`:

```rust
pub mod fleet_counts;
```

Add to `crates/rupu-cp/src/host/fleet_counts.rs`, above the test module:

```rust
use crate::api::repo_scope::{distinct_repo_workspaces, ScopeKind};
use crate::host::dashboard_summary::FleetCounts;
use rupu_workspace::{AutoflowClaimStore, ClaimStatus, RepoRegistryStore, WorkspaceStore};
use rupu_workspace::worker_store::WorkerStore;

/// Read every `<global_dir>`-sourced fleet count. Fields owned by Plans 2 and
/// 3 are left `None`.
pub fn collect_fleet_counts(global_dir: &std::path::Path) -> FleetCounts {
    let (autoflows_enabled, autoflows_disabled) = count_autoflow_defs(global_dir);
    FleetCounts {
        repos: None,
        providers_configured: None,
        providers_unhealthy: None,
        autoflows_enabled,
        autoflows_disabled,
        workers: count_workers(global_dir),
        claims_active: count_claims_active(global_dir),
        issues_pending: None,
        issues_open: None,
        issues_capped: false,
        inventory_captured_at: None,
    }
}

/// Registered local worker identities, from the same store `GET /api/workers`
/// reads.
fn count_workers(global_dir: &std::path::Path) -> Option<u64> {
    let store = WorkerStore {
        root: global_dir.join("autoflows").join("workers"),
    };
    match store.list() {
        Ok(w) => Some(w.len() as u64),
        Err(e) => {
            tracing::warn!(error = %e, "fleet_counts: worker store list failed; workers unreported");
            None
        }
    }
}

/// Claims still in flight — every status except `Complete` and `Released`.
fn count_claims_active(global_dir: &std::path::Path) -> Option<u64> {
    let store = AutoflowClaimStore {
        root: global_dir.join("autoflows").join("claims"),
    };
    match store.list() {
        Ok(claims) => Some(
            claims
                .iter()
                .filter(|c| !matches!(c.status, ClaimStatus::Complete | ClaimStatus::Released))
                .count() as u64,
        ),
        Err(e) => {
            tracing::warn!(error = %e, "fleet_counts: claim store list failed; claims_active unreported");
            None
        }
    }
}

/// Autoflow definitions split by their on-disk `autoflow.enabled` flag.
///
/// Scans the same set of directories `GET /api/autoflows` does — the global
/// workflows dir plus one representative workspace per distinct repo — via the
/// shared `scan_autoflow_defs`, so the strip's number can never disagree with
/// the list the operator sees when they click through. Both counts are
/// infallible (`scan_autoflow_defs` tolerates a missing dir and skips
/// unparseable files with a warn), hence `Some` unconditionally.
fn count_autoflow_defs(global_dir: &std::path::Path) -> (Option<u64>, Option<u64>) {
    let mut rows = crate::api::autoflows::scan_autoflow_defs(
        &global_dir.join("workflows"),
        "global",
        ScopeKind::Global,
        None,
    );

    let workspaces = WorkspaceStore {
        root: global_dir.join("workspaces"),
    }
    .list()
    .unwrap_or_default();
    let repo_store = RepoRegistryStore {
        root: global_dir.join("repos"),
    };
    let mut project_rows = Vec::new();
    for r in distinct_repo_workspaces(workspaces, &repo_store) {
        let dir = std::path::Path::new(&r.workspace.path)
            .join(".rupu")
            .join("workflows");
        let scope_id = Some(r.workspace.id.clone());
        project_rows.extend(crate::api::autoflows::scan_autoflow_defs(
            &dir,
            r.scope,
            ScopeKind::Project,
            scope_id,
        ));
    }

    // A project def shadows a same-named global one — mirror the endpoint's
    // dedupe so the strip counts each definition exactly once.
    let project_names: std::collections::BTreeSet<&str> =
        project_rows.iter().map(|r| r.name.as_str()).collect();
    rows.retain(|r| !project_names.contains(r.name.as_str()));
    rows.extend(project_rows);

    let enabled = rows.iter().filter(|r| r.enabled).count() as u64;
    let disabled = rows.len() as u64 - enabled;
    (Some(enabled), Some(disabled))
}
```

`scan_autoflow_defs` and `AutoflowDefRow::enabled` are `pub(crate)` in `crate::api::autoflows` — reachable from here without a visibility change. If `AutoflowClaimStore` / `ClaimStatus` / `WorkerStore` are not re-exported at `rupu_workspace`'s root, import them from their declaring modules instead; do not widen any visibility to make the import shorter.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp fleet_counts::`
Expected: 3 passed.

- [ ] **Step 5: Thread `fleet` through `build_summary`**

In `crates/rupu-cp/src/host/summary_build.rs`, change the signature and the constructed value:

```rust
pub fn build_summary(
    runs: &[RunRecord],
    cycles: &[CycleRollup],
    findings_open: Option<u64>,
    fleet: crate::host::dashboard_summary::FleetCounts,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> DashboardSummary {
```

and add `fleet,` to the `DashboardSummary { .. }` literal it returns.

In `crates/rupu-cp/src/host/local.rs`, replace the `build_summary` call at the end of `dashboard_summary`:

```rust
        // This host reads its own stores, so it reports the `<global_dir>`
        // fleet counts directly. The SCM- and probe-backed fields stay `None`
        // until Plans 2 and 3 install the inventory port.
        let fleet = crate::host::fleet_counts::collect_fleet_counts(&self.global_dir);
        Ok(crate::host::summary_build::build_summary(
            &runs,
            &cycles,
            findings_open,
            fleet,
            range,
            chrono::Utc::now(),
        ))
```

In `crates/rupu-cp/src/host/ssh.rs`, in the `DashboardSummary { .. }` literal built by `dashboard_summary` (around line 1430+), add:

```rust
            // SSH shells to the remote `rupu` CLI, which has no fleet-inventory
            // surface. Report nothing rather than zeros — the merge raises
            // `fleet_partial` so the strip says so.
            fleet: crate::host::dashboard_summary::FleetCounts::default(),
```

Fix any other `DashboardSummary { .. }` literals the compiler flags (test fixtures in `api/dashboard.rs`, `local.rs`, `ssh.rs`) by adding `fleet: FleetCounts::default(),` — or, where the fixture already uses `..empty_summary(now)`, add the field to that helper only.

- [ ] **Step 6: Run the full crate test suite**

Run: `cargo test -p rupu-cp`
Expected: PASS. Any failure here is a fixture that still omits `fleet`.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/host/fleet_counts.rs
rustfmt --edition 2021 crates/rupu-cp/src/host/local.rs
rustfmt --edition 2021 crates/rupu-cp/src/host/ssh.rs
rustfmt --edition 2021 crates/rupu-cp/src/host/summary_build.rs
rustfmt --edition 2021 crates/rupu-cp/src/host/mod.rs
git add crates/rupu-cp/src/host/
git commit -m "feat(cp): collect global-dir fleet counts on the local host"
```

---

### Task 3: Merge `fleet` server-side

**Files:**
- Modify: `crates/rupu-cp/src/api/dashboard.rs:60-86` (response DTO), `:233-371` (merge fn)

**Interfaces:**
- Consumes: `FleetCounts` (Task 1), `DashboardSummary::fleet` (Task 2).
- Produces: `merge_dashboard_summaries` returns a 4-tuple `(DashboardSummary, bool, bool, bool)` — the fourth element is `fleet_partial`. `DashboardResponse` gains `fleet_partial: bool`.

- [ ] **Step 1: Write the failing tests**

Append to `mod merge_tests` in `crates/rupu-cp/src/api/dashboard.rs`. Note every existing call to `merge_dashboard_summaries` in this module destructures a 3-tuple and must be widened to 4 — do that in Step 3, not now.

```rust
    /// The `findings_open` rule, applied to every fleet count: sum only the
    /// hosts that reported, flag the aggregate partial, and NEVER fold a
    /// non-reporting host in as a zero.
    #[test]
    fn fleet_counts_sum_only_reporting_hosts_and_flag_partial() {
        let now = Utc::now();
        let local = DashboardSummary {
            fleet: FleetCounts {
                workers: Some(3),
                claims_active: Some(9),
                autoflows_enabled: Some(4),
                providers_configured: Some(2),
                ..FleetCounts::default()
            },
            ..empty_summary(now)
        };
        let ssh = DashboardSummary {
            fleet: FleetCounts::default(),
            ..empty_summary(now)
        };

        let (merged, _findings, _cycles, fleet_partial) =
            merge_dashboard_summaries(vec![local, ssh], DashboardRange::All, now);

        assert_eq!(merged.fleet.workers, Some(3), "must not fabricate 0 for ssh");
        assert_eq!(merged.fleet.claims_active, Some(9));
        assert_eq!(merged.fleet.autoflows_enabled, Some(4));
        assert!(
            fleet_partial,
            "the ssh host reported None for every fleet field; the aggregate is partial"
        );
    }

    #[test]
    fn fleet_counts_sum_across_hosts_and_are_not_partial_when_all_report() {
        let now = Utc::now();
        let a = DashboardSummary {
            fleet: FleetCounts {
                workers: Some(2),
                claims_active: Some(1),
                autoflows_enabled: Some(1),
                autoflows_disabled: Some(0),
                providers_configured: Some(3),
                ..FleetCounts::default()
            },
            ..empty_summary(now)
        };
        let b = DashboardSummary {
            fleet: FleetCounts {
                workers: Some(5),
                claims_active: Some(4),
                autoflows_enabled: Some(2),
                autoflows_disabled: Some(1),
                providers_configured: Some(3),
                ..FleetCounts::default()
            },
            ..empty_summary(now)
        };

        let (merged, _f, _c, fleet_partial) =
            merge_dashboard_summaries(vec![a, b], DashboardRange::All, now);

        assert_eq!(merged.fleet.workers, Some(7));
        assert_eq!(merged.fleet.claims_active, Some(5));
        assert_eq!(merged.fleet.autoflows_enabled, Some(3));
        assert_eq!(merged.fleet.autoflows_disabled, Some(1));
        assert_eq!(merged.fleet.providers_configured, Some(6));
        assert!(
            !fleet_partial,
            "every host reported every field it was asked for"
        );
        assert!(
            !merged.fleet.issues_capped,
            "no host reported a cap"
        );
    }

    /// `issues_capped` is a disjunction, not a sum: one capped host makes the
    /// whole aggregate a floor.
    #[test]
    fn issues_capped_ors_across_hosts_and_inventory_stamp_takes_the_oldest() {
        let now = Utc::now();
        let older = now - chrono::Duration::minutes(20);
        let a = DashboardSummary {
            fleet: FleetCounts {
                issues_open: Some(300),
                issues_capped: true,
                inventory_captured_at: Some(older),
                ..FleetCounts::default()
            },
            ..empty_summary(now)
        };
        let b = DashboardSummary {
            fleet: FleetCounts {
                issues_open: Some(12),
                issues_capped: false,
                inventory_captured_at: Some(now),
                ..FleetCounts::default()
            },
            ..empty_summary(now)
        };

        let (merged, _f, _c, _p) =
            merge_dashboard_summaries(vec![a, b], DashboardRange::All, now);

        assert_eq!(merged.fleet.issues_open, Some(312));
        assert!(
            merged.fleet.issues_capped,
            "one capped host makes the merged open count a floor"
        );
        assert_eq!(
            merged.fleet.inventory_captured_at,
            Some(older),
            "the honest staleness bound is the OLDEST contributing cache, not the newest"
        );
    }

    #[test]
    fn fleet_is_not_partial_when_no_host_reports_at_all() {
        let now = Utc::now();
        let (merged, _f, _c, fleet_partial) =
            merge_dashboard_summaries(vec![], DashboardRange::All, now);
        assert_eq!(merged.fleet.workers, None);
        assert!(
            !fleet_partial,
            "partial means 'a reporting host omitted a field' — with zero reporting \
             hosts there is nothing to be partial about (hosts[] carries the outage)"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp merge_tests`
Expected: FAIL — `merge_dashboard_summaries` returns a 3-tuple.

- [ ] **Step 3: Implement the merge**

Add `FleetCounts` to the `dashboard_summary::{...}` import list at the top of `crates/rupu-cp/src/api/dashboard.rs`.

Add the field to `DashboardResponse`, next to `cycles_partial`:

```rust
    /// Same "not reported ≠ 0" rule as `findings_partial`, for the fleet
    /// strip: true when at least one reporting host contributed `None` for a
    /// fleet count that other hosts DID report. When true the strip's numbers
    /// are a partial sum across the reporting hosts, never a fleet total.
    fleet_partial: bool,
```

Replace the merge function's signature, add the accumulator, and extend the loop and the return.

Signature:

```rust
fn merge_dashboard_summaries(
    reported: Vec<DashboardSummary>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> (DashboardSummary, bool, bool, bool) {
```

Declare beside the other accumulators:

```rust
    let mut fleet = FleetCounts::default();
    let mut fleet_partial = false;
```

Inside the `for sum in reported` loop, after the `findings_open` match:

```rust
        // Every fleet count follows the `findings_open` rule — sum the
        // `Some`s, flag partial on a `None`, never fabricate a zero. Kept as
        // one closure so a field added in Plan 2 or 3 cannot accidentally get
        // different semantics from the rest.
        let mut add = |acc: &mut Option<u64>, v: Option<u64>| match v {
            Some(n) => *acc = Some(acc.unwrap_or(0) + n),
            None => fleet_partial = true,
        };
        add(&mut fleet.repos, sum.fleet.repos);
        add(&mut fleet.providers_configured, sum.fleet.providers_configured);
        add(&mut fleet.providers_unhealthy, sum.fleet.providers_unhealthy);
        add(&mut fleet.autoflows_enabled, sum.fleet.autoflows_enabled);
        add(&mut fleet.autoflows_disabled, sum.fleet.autoflows_disabled);
        add(&mut fleet.workers, sum.fleet.workers);
        add(&mut fleet.claims_active, sum.fleet.claims_active);
        add(&mut fleet.issues_pending, sum.fleet.issues_pending);
        add(&mut fleet.issues_open, sum.fleet.issues_open);

        // A cap anywhere makes the merged open count a floor.
        fleet.issues_capped |= sum.fleet.issues_capped;
        // Oldest wins, for the same reason `oldest_captured_at` does: a fleet
        // number is only as fresh as its stalest contributing cache.
        if let Some(ts) = sum.fleet.inventory_captured_at {
            fleet.inventory_captured_at = Some(match fleet.inventory_captured_at {
                Some(cur) => cur.min(ts),
                None => ts,
            });
        }
```

Add `fleet,` to the returned `DashboardSummary { .. }` literal, and return the 4-tuple:

```rust
        findings_partial,
        cycles_partial,
        fleet_partial,
    )
```

Update the handler's destructuring and the response literal:

```rust
    let (summary, findings_partial, cycles_partial, fleet_partial) =
        merge_dashboard_summaries(reported, range, Utc::now());

    Ok(Json(DashboardResponse {
        hosts,
        findings_partial,
        cycles_partial,
        fleet_partial,
        summary,
    }))
```

Widen every pre-existing 3-tuple destructuring in `mod merge_tests` to 4 by adding a trailing `_fleet_partial` binding.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp`
Expected: PASS, including the four new merge tests.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/api/dashboard.rs
git add crates/rupu-cp/src/api/dashboard.rs
git commit -m "feat(cp): merge fleet counts with the findings_open partial rule"
```

---

### Task 4: Wire types and the client merge

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts:689-728`
- Modify: `crates/rupu-cp/web/src/lib/dashboard/mergeSummaries.ts`
- Modify: `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts:105`, `:268-270`
- Test: `crates/rupu-cp/web/src/lib/dashboard/mergeSummaries.test.ts`

**Interfaces:**
- Consumes: the wire shape from Task 3.
- Produces: `export interface FleetCounts`; `DashboardSummary.fleet: FleetCounts`; `DashboardResponse.fleet_partial: boolean`; `mergeSummaries` merges `fleet`; the hook returns `fleet_partial`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rupu-cp/web/src/lib/dashboard/mergeSummaries.test.ts`. Reuse the file's existing summary-fixture helper; if it is named something other than `emptySummary`, adapt these calls rather than adding a second helper.

```ts
describe('fleet merge', () => {
  it('sums only reporting hosts and never fabricates a zero', () => {
    const local = {
      ...emptySummary(),
      fleet: { ...emptyFleet(), workers: 3, claims_active: 9, autoflows_enabled: 4 },
    };
    const ssh = { ...emptySummary(), fleet: emptyFleet() };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.fleet.workers).toBe(3);
    expect(merged.fleet.claims_active).toBe(9);
    expect(merged.fleet.autoflows_enabled).toBe(4);
  });

  it('leaves a field null when no host reports it', () => {
    const merged = mergeSummaries([
      { ...emptySummary(), fleet: emptyFleet() },
    ]);
    expect(merged.fleet.workers).toBeNull();
    expect(merged.fleet.repos).toBeNull();
  });

  it('ORs issues_capped and takes the oldest inventory stamp', () => {
    const older = '2026-07-30T10:00:00Z';
    const newer = '2026-07-30T10:20:00Z';
    const a = {
      ...emptySummary(),
      fleet: { ...emptyFleet(), issues_open: 300, issues_capped: true, inventory_captured_at: older },
    };
    const b = {
      ...emptySummary(),
      fleet: { ...emptyFleet(), issues_open: 12, issues_capped: false, inventory_captured_at: newer },
    };

    const merged = mergeSummaries([a, b]);

    expect(merged.fleet.issues_open).toBe(312);
    expect(merged.fleet.issues_capped).toBe(true);
    expect(merged.fleet.inventory_captured_at).toBe(older);
  });
});
```

Add the fixture helper to the same file:

```ts
function emptyFleet(): FleetCounts {
  return {
    repos: null,
    providers_configured: null,
    providers_unhealthy: null,
    autoflows_enabled: null,
    autoflows_disabled: null,
    workers: null,
    claims_active: null,
    issues_pending: null,
    issues_open: null,
    issues_capped: false,
    inventory_captured_at: null,
  };
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npm test -- mergeSummaries`
Expected: FAIL — `FleetCounts` is not exported and `merged.fleet` is undefined.

- [ ] **Step 3: Add the types**

In `crates/rupu-cp/web/src/lib/api.ts`, above `interface DashboardSummary`:

```ts
/**
 * One host's inventory contribution — the fleet strip beneath the ops blocks.
 * Mirrors `FleetCounts` in `rupu-cp/src/host/dashboard_summary.rs`.
 *
 * Every count is `number | null`. `null` means "this host does not report the
 * field", NOT zero — render it as an em-dash and never as a `0`.
 */
export interface FleetCounts {
  repos: number | null;
  providers_configured: number | null;
  providers_unhealthy: number | null;
  autoflows_enabled: number | null;
  autoflows_disabled: number | null;
  workers: number | null;
  claims_active: number | null;
  issues_pending: number | null;
  issues_open: number | null;
  /** True when a repo hit the per-repo issue fetch cap — `issues_open` is a floor. */
  issues_capped: boolean;
  /** When the SCM/provider caches behind these numbers were filled. */
  inventory_captured_at: string | null;
}
```

Add `fleet: FleetCounts;` to `interface DashboardSummary`, and to `interface DashboardResponse`:

```ts
  /**
   * Same "not reported ≠ 0" rule as `findings_partial`, for the fleet strip:
   * true when a reporting host contributed `null` for a count other hosts did
   * report. The strip must mark itself partial rather than present a partial
   * sum as a fleet total.
   */
  fleet_partial: boolean;
```

- [ ] **Step 4: Implement the client merge**

In `crates/rupu-cp/web/src/lib/dashboard/mergeSummaries.ts`, add `FleetCounts` to the type import and add:

```ts
/**
 * Sum only the non-null contributors — the `mergeFindingsOpen` rule, not the
 * poisoning `mergeCycles` rule. A host that does not report a field is skipped;
 * the response-level `fleet_partial` flag (derived in useDashboardData.ts from
 * these same per-host summaries) is what tells the user the sum is partial.
 *
 * `issues_capped` ORs — one capped host makes the merged open count a floor.
 * `inventory_captured_at` takes the OLDEST stamp, the honest staleness bound.
 */
function mergeFleet(byHost: DashboardSummary[]): FleetCounts {
  const out: FleetCounts = {
    repos: null,
    providers_configured: null,
    providers_unhealthy: null,
    autoflows_enabled: null,
    autoflows_disabled: null,
    workers: null,
    claims_active: null,
    issues_pending: null,
    issues_open: null,
    issues_capped: false,
    inventory_captured_at: null,
  };

  const numericKeys = [
    'repos',
    'providers_configured',
    'providers_unhealthy',
    'autoflows_enabled',
    'autoflows_disabled',
    'workers',
    'claims_active',
    'issues_pending',
    'issues_open',
  ] as const;

  for (const s of byHost) {
    for (const k of numericKeys) {
      const v = s.fleet[k];
      if (v !== null && v !== undefined) out[k] = (out[k] ?? 0) + v;
    }
    if (s.fleet.issues_capped) out.issues_capped = true;
    const ts = s.fleet.inventory_captured_at;
    if (ts && (out.inventory_captured_at === null || Date.parse(ts) < Date.parse(out.inventory_captured_at))) {
      out.inventory_captured_at = ts;
    }
  }

  return out;
}
```

Add `fleet: mergeFleet(byHost),` to the object `mergeSummaries` returns.

- [ ] **Step 5: Derive `fleet_partial` in the hook**

In `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts`, add to the result interface beside `cycles_partial`:

```ts
  fleet_partial: boolean;
```

and beside the existing derivations near line 268:

```ts
    // A reporting host that contributed null for ANY fleet count makes the
    // strip's numbers a partial sum. Mirrors the server's single
    // `fleet_partial` flag rather than one flag per field.
    const fleet_partial = okSummaries.some(
      (s) =>
        s.fleet.repos === null ||
        s.fleet.providers_configured === null ||
        s.fleet.providers_unhealthy === null ||
        s.fleet.autoflows_enabled === null ||
        s.fleet.autoflows_disabled === null ||
        s.fleet.workers === null ||
        s.fleet.claims_active === null ||
        s.fleet.issues_pending === null ||
        s.fleet.issues_open === null,
    );
    return { ...merged, findings_partial, cycles_partial, fleet_partial };
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npm test -- mergeSummaries useDashboardData`
Expected: PASS. Fix any pre-existing fixture in these test files that now lacks `fleet` by adding `fleet: emptyFleet()`.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/lib/
git commit -m "feat(cp-web): fleet counts in the wire types and the client merge"
```

---

### Task 5: The `FleetStrip` component

**Files:**
- Create: `crates/rupu-cp/web/src/components/dashboard/FleetStrip.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/FleetStrip.test.tsx`

**Interfaces:**
- Consumes: `FleetCounts` (Task 4).
- Produces: `export function FleetStrip({ fleet, fleetPartial }: { fleet: FleetCounts; fleetPartial: boolean })`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cp/web/src/components/dashboard/FleetStrip.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { FleetStrip } from './FleetStrip';
import type { FleetCounts } from '../../lib/api';

function fleet(over: Partial<FleetCounts> = {}): FleetCounts {
  return {
    repos: null,
    providers_configured: null,
    providers_unhealthy: null,
    autoflows_enabled: null,
    autoflows_disabled: null,
    workers: null,
    claims_active: null,
    issues_pending: null,
    issues_open: null,
    issues_capped: false,
    inventory_captured_at: null,
    ...over,
  };
}

function renderStrip(f: FleetCounts, partial = false) {
  return render(
    <MemoryRouter>
      <FleetStrip fleet={f} fleetPartial={partial} />
    </MemoryRouter>,
  );
}

describe('FleetStrip', () => {
  it('renders an em-dash for a null count, never a zero', () => {
    renderStrip(fleet({ workers: null }));
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('—');
    expect(screen.getByTestId('fleet-workers')).not.toHaveTextContent('0');
  });

  it('renders a genuine zero as 0', () => {
    renderStrip(fleet({ workers: 0 }));
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('0');
  });

  it('shows the disabled autoflow count only when some are disabled', () => {
    const { rerender } = renderStrip(fleet({ autoflows_enabled: 6, autoflows_disabled: 2 }));
    expect(screen.getByTestId('fleet-autoflows')).toHaveTextContent('2 off');

    rerender(
      <MemoryRouter>
        <FleetStrip fleet={fleet({ autoflows_enabled: 6, autoflows_disabled: 0 })} fleetPartial={false} />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('fleet-autoflows')).not.toHaveTextContent('off');
  });

  it('suffixes the open issue count with + when capped', () => {
    renderStrip(fleet({ issues_pending: 14, issues_open: 312, issues_capped: true }));
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('312+');
  });

  it('does not suffix the open issue count when not capped', () => {
    renderStrip(fleet({ issues_pending: 14, issues_open: 312, issues_capped: false }));
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('312');
    expect(screen.getByTestId('fleet-issues')).not.toHaveTextContent('312+');
  });

  it('weights the providers segment only when some are unhealthy', () => {
    const { rerender } = renderStrip(fleet({ providers_configured: 4, providers_unhealthy: 1 }));
    expect(screen.getByTestId('fleet-providers')).toHaveAttribute('data-fault', 'true');

    rerender(
      <MemoryRouter>
        <FleetStrip fleet={fleet({ providers_configured: 4, providers_unhealthy: 0 })} fleetPartial={false} />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('fleet-providers')).toHaveAttribute('data-fault', 'false');
  });

  it('claims nothing about health when providers_unhealthy is null', () => {
    renderStrip(fleet({ providers_configured: 4, providers_unhealthy: null }));
    const seg = screen.getByTestId('fleet-providers');
    expect(seg).toHaveTextContent('4');
    expect(seg).toHaveAttribute('data-fault', 'false');
    expect(seg).not.toHaveTextContent('unhealthy');
  });

  it('marks itself partial when a reporting host omitted a count', () => {
    renderStrip(fleet({ workers: 3 }), true);
    expect(screen.getByTestId('fleet-strip')).toHaveTextContent('partial');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npm test -- FleetStrip`
Expected: FAIL — cannot resolve `./FleetStrip`.

- [ ] **Step 3: Implement the component**

Create `crates/rupu-cp/web/src/components/dashboard/FleetStrip.tsx`:

```tsx
// FleetStrip — the inventory band beneath the ops blocks (spec §1).
//
// The Dashboard is an ops monitor; this row is supporting context, not a
// second subject. It therefore renders dim by default and takes weight ONLY
// where something is actually wrong — today that is exclusively "a configured
// provider failed its last probe". A count is not an alarm.
//
// The `null` discipline is the whole point of the component: a count this host
// does not report renders as an em-dash. Rendering it as `0` would make an
// unconfigured SCM connector look like an operator with no repos.

import { Link } from 'react-router-dom';
import type { FleetCounts } from '../../lib/api';

/** Em-dash for "not reported"; the number itself for a genuine value (incl. 0). */
function num(v: number | null): string {
  return v === null || v === undefined ? '—' : String(v);
}

function Segment({
  testId,
  to,
  fault = false,
  children,
}: {
  testId: string;
  to: string;
  fault?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Link
      to={to}
      data-testid={testId}
      data-fault={fault ? 'true' : 'false'}
      className={fault ? 'text-status-failed hover:underline' : 'text-ink-mute hover:text-ink'}
    >
      {children}
    </Link>
  );
}

export function FleetStrip({
  fleet,
  fleetPartial,
}: {
  fleet: FleetCounts;
  /** True = these numbers are a partial sum, not a fleet total. */
  fleetPartial: boolean;
}) {
  // `providers_unhealthy === null` means no probe has run (Plan 2 not
  // installed, or no `cp serve`). That is an absence of information, not a
  // clean bill of health — so it must not weight the segment and must not
  // render an "unhealthy" clause.
  const unhealthy = fleet.providers_unhealthy;
  const providersFault = unhealthy !== null && unhealthy > 0;

  const disabled = fleet.autoflows_disabled;
  const showDisabled = disabled !== null && disabled > 0;

  return (
    <div
      data-testid="fleet-strip"
      className="flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-border pt-2 text-xs text-ink-mute"
    >
      <Segment testId="fleet-repos" to="/settings">
        {num(fleet.repos)} repos
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-providers" to="/settings" fault={providersFault}>
        {num(fleet.providers_configured)} providers
        {providersFault && ` (${unhealthy} unhealthy)`}
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-autoflows" to="/autoflows">
        {num(fleet.autoflows_enabled)} autoflows
        {showDisabled && ` (${disabled} off)`}
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-workers" to="/workers">
        {num(fleet.workers)} workers
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-claims" to="/autoflows/claims">
        {num(fleet.claims_active)} claimed
      </Segment>
      <span aria-hidden>·</span>
      <Segment testId="fleet-issues" to="/settings">
        {num(fleet.issues_pending)} pending
        {fleet.issues_open !== null && ` (${fleet.issues_open}${fleet.issues_capped ? '+' : ''} open)`}
      </Segment>
      {fleetPartial && (
        <span
          className="text-ink-dim"
          title="Some reporting hosts do not supply these counts — this is a partial sum, not a fleet total."
        >
          (partial)
        </span>
      )}
    </div>
  );
}
```

Confirm the `/runs/autoflows` (Claims tab) and `/workers` route paths against `crates/rupu-cp/web/src/App.tsx` before committing; if either differs, use the real path rather than adding a route.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd crates/rupu-cp/web && npm test -- FleetStrip`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/dashboard/FleetStrip.tsx crates/rupu-cp/web/src/components/dashboard/FleetStrip.test.tsx
git commit -m "feat(cp-web): FleetStrip component"
```

---

### Task 6: Render the strip on the Dashboard

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Dashboard.tsx:23-32` (imports), `:126` (below `CycleSummaryLine`)
- Test: `crates/rupu-cp/web/src/pages/Dashboard.test.tsx`

**Interfaces:**
- Consumes: `FleetStrip` (Task 5), `data.fleet` / `data.fleet_partial` (Task 4).

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-cp/web/src/pages/Dashboard.test.tsx`, adapting the file's existing mocking of `useDashboardData` rather than introducing a second mocking style:

```tsx
  it('renders the fleet strip beneath the cycle summary', async () => {
    renderDashboardWith({
      ...baseData,
      fleet: {
        repos: 7,
        providers_configured: 4,
        providers_unhealthy: 0,
        autoflows_enabled: 6,
        autoflows_disabled: 2,
        workers: 3,
        claims_active: 9,
        issues_pending: null,
        issues_open: null,
        issues_capped: false,
        inventory_captured_at: null,
      },
      fleet_partial: false,
    });

    expect(await screen.findByTestId('fleet-strip')).toBeInTheDocument();
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('3 workers');
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('—');
  });
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/rupu-cp/web && npm test -- Dashboard`
Expected: FAIL — no element with testid `fleet-strip`.

- [ ] **Step 3: Render it**

Add the import in `crates/rupu-cp/web/src/pages/Dashboard.tsx`:

```tsx
import { FleetStrip } from '../components/dashboard/FleetStrip';
```

and render it immediately after `<CycleSummaryLine ... />`:

```tsx
          <FleetStrip fleet={data.fleet} fleetPartial={data.fleet_partial} />
```

Update the module doc comment at the top of the file: the composition list now ends `→ CycleSummaryLine → FleetStrip`, and the strip is inventory context that takes weight only on a fault.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/rupu-cp/web && npm test -- Dashboard`
Expected: PASS.

- [ ] **Step 5: Run the full suite both sides**

Run: `cd crates/rupu-cp/web && npm test -- --run` then `cargo test -p rupu-cp`
Expected: both green.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Dashboard.tsx crates/rupu-cp/web/src/pages/Dashboard.test.tsx
git commit -m "feat(cp-web): render the fleet strip on the Dashboard"
```

---

## Verification

Beyond the suites, confirm the honest-degradation behaviour by hand — this is the part unit tests cannot fully cover:

- [ ] `cargo build -p rupu-cp && cargo clippy -p rupu-cp -- -D warnings` clean.
- [ ] `cd crates/rupu-cp/web && npx tsc -b` clean.
- [ ] Run `rupu cp` (read-only, no `cp serve`) against a real `~/.rupu`: the strip shows real autoflow / worker / claim counts, and em-dashes for repos, providers, and issues. **No segment shows a fabricated `0`.**
- [ ] With a registered SSH host in the fleet, the strip renders `(partial)`.
- [ ] `make cp-web` rebuilds the embedded UI (required before any release — the binary embeds `web/dist`).
