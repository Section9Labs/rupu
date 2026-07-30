# Dashboard Fleet Strip — Plan 3: repos and issues

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill `FleetCounts::repos`, `issues_pending`, `issues_open`, and `issues_capped` from a cached SCM read, where "pending" is derived by the **same** code path the cron tick uses to decide what to pick up.

**Architecture:** No new port. Plan 2's `InventorySnapshot` already declares these four fields; Plan 3 fills them in the `cp serve` adapter, which is where the SCM credentials and the autoflow-discovery code already live. The provider half (5-minute TTL) and the SCM half (15-minute TTL) refresh on separate tasks against one cache, and the snapshot's `captured_at` reports the older of the two stamps.

**Tech Stack:** Rust 2021 (tokio, chrono), `rupu-scm` connectors, `rupu-cli`'s existing autoflow discovery.

## Spec

`docs/superpowers/specs/2026-07-30-rupu-dashboard-fleet-strip-design.md` §3 (ScmInventoryCache, Pending derivation).

## Prerequisites

**Plans 1 and 2 must be merged.** This plan fills fields Plan 1 declared, through the cache and adapter Plan 2 built.

## Deviation from the spec — where "pending" is derived

The spec says the port returns raw issues and **rupu-cp** derives pending by matching autoflow selectors. Implement it in the **adapter** instead, by calling the existing pair:

- `discover_tick_autoflows(global, repo_store)` (`crates/rupu-cli/src/cmd/autoflow.rs:9357`)
- `collect_issue_matches(&discovered, resolver)` (`crates/rupu-cli/src/cmd/autoflow.rs:9581`)

`collect_issue_matches` already does everything the spec described and more: it builds the `IssueFilter` from each autoflow's selector, lists issues, applies `selector_matches` (covering `labels_any`, which the spec's field list omitted), applies the author allowlist, and truncates to `selector.limit`.

Reimplementing that inside rupu-cp would create a **second** matcher that can drift from the one the cron tick actually runs — and a strip that says "14 pending" while rupu picks up 9 is worse than no number at all. The number must come from the same code that does the work. rupu-cp keeps no new SCM knowledge either way; the boundary is unchanged.

## Deviation from the spec — no separate `IssueLister` port

The spec calls for a new `IssueLister` port in rupu-cp mirroring `RepoLister`. It is not built. Plan 2's `InventorySnapshot` already carries `repos` / `issues_pending` / `issues_open` / `issues_capped` across the same boundary, on the same cache, with the same staleness stamp. A second port would duplicate that crossing for data with an identical lifetime, and would need its own cache and its own refresh task to avoid blocking the dashboard — for no additional capability.

`RepoLister` still exists and is still used: `refresh_scm` calls it for the repo enumeration rather than opening a second one.

## Global Constraints

- Workspace deps only — versions pinned in the root `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **rupu-cp must not gain a dependency on rupu-providers or grow SCM credential handling.**
- **Never run `cargo fmt` in any form — including `cargo fmt -- <path>`**, which resolves the whole workspace despite reading like a per-file filter. Use `rustfmt --edition 2021 <file>`, one file at a time.
- **`snapshot()` stays synchronous and non-blocking.** All network work happens on the refresh tasks.
- Per-repo issue fetch cap: **500**, a tunable constant. Hitting it sets `issues_capped`, and the UI renders the open count as a floor.
- A count that could not be read is `None`, never `Some(0)`.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/rupu-cli/src/cp_inventory.rs` | **Modify.** Split refresh into provider/SCM halves; add repo + issue collection. |
| `crates/rupu-cli/src/cmd/cp.rs` | **Modify.** Spawn the second (SCM) refresh task. |
| `crates/rupu-cli/src/cmd/autoflow.rs` | **Modify.** Widen `discover_tick_autoflows` / `collect_issue_matches` visibility if they are not already reachable from `cp_inventory`. |

---

### Task 1: Split the cache into two halves

**Files:**
- Modify: `crates/rupu-cli/src/cp_inventory.rs`

**Interfaces:**
- Consumes: `CpFleetInventory`, `InventorySnapshot` (Plan 2).
- Produces: `refresh_providers(&self)` (renamed from Plan 2's `refresh`), `refresh_scm(&self)`, and `pub const SCM_TTL_SECS: u64 = 900`. `snapshot().captured_at` becomes the **older** of the two half-stamps.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/rupu-cli/src/cp_inventory.rs`:

```rust
    /// Two halves refresh on different schedules, so the snapshot's single
    /// stamp must report the OLDER one — a fleet number is only as fresh as
    /// its stalest contributing cache. Reporting the newer stamp would claim
    /// the 15-minute-old issue counts were seconds old.
    #[test]
    fn captured_at_reports_the_older_of_the_two_half_stamps() {
        let inv = CpFleetInventory::new_for_test();
        let older = chrono::Utc::now() - chrono::Duration::minutes(12);
        let newer = chrono::Utc::now();

        inv.set_provider_stamp_for_test(newer);
        inv.set_scm_stamp_for_test(older);

        assert_eq!(inv.snapshot().captured_at, Some(older));
    }

    /// Before EITHER half has run there is no stamp at all.
    #[test]
    fn captured_at_is_none_until_a_half_has_refreshed() {
        let inv = CpFleetInventory::new_for_test();
        assert_eq!(inv.snapshot().captured_at, None);
    }

    /// One half having run does not fabricate a stamp for the other.
    #[test]
    fn captured_at_uses_the_only_stamp_when_just_one_half_has_run() {
        let inv = CpFleetInventory::new_for_test();
        let t = chrono::Utc::now();
        inv.set_provider_stamp_for_test(t);
        assert_eq!(inv.snapshot().captured_at, Some(t));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli cp_inventory`
Expected: FAIL — no `set_provider_stamp_for_test`.

- [ ] **Step 3: Restructure the cache**

Replace `CpFleetInventory`'s single `cache: RwLock<InventorySnapshot>` with two independently-stamped halves, so a slow SCM refresh never blanks fresh provider data (and vice versa):

```rust
/// The provider half of the cache, refreshed on `PROBE_TTL_SECS`.
#[derive(Default)]
struct ProviderHalf {
    rows: Vec<ProviderProbeRow>,
    stamp: Option<DateTime<Utc>>,
}

/// The SCM half, refreshed on the much longer `SCM_TTL_SECS` — filling it is
/// N API calls, one set per connected repo.
#[derive(Default)]
struct ScmHalf {
    repos: Option<u64>,
    issues_pending: Option<u64>,
    issues_open: Option<u64>,
    issues_capped: bool,
    stamp: Option<DateTime<Utc>>,
}

pub struct CpFleetInventory {
    registry: Option<Arc<ProviderRegistry>>,
    /// `cp serve`'s global dir and SCM deps, for the SCM half. `None` in tests.
    scm: Option<ScmDeps>,
    providers: RwLock<ProviderHalf>,
    scm_cache: RwLock<ScmHalf>,
}

/// How long SCM inventory stays authoritative. Much longer than the provider
/// TTL: filling it costs one issue listing per connected repo.
pub const SCM_TTL_SECS: u64 = 900;
```

Rename Plan 2's `refresh` to `refresh_providers` and have it write `ProviderHalf` instead of the whole snapshot. Rewrite `snapshot`:

```rust
impl FleetInventory for CpFleetInventory {
    fn snapshot(&self) -> InventorySnapshot {
        let p = self.providers.read().ok();
        let s = self.scm_cache.read().ok();

        let p_stamp = p.as_ref().and_then(|h| h.stamp);
        let s_stamp = s.as_ref().and_then(|h| h.stamp);
        // Oldest wins; a half that has never run contributes no stamp at all
        // rather than pinning the pair to `None`.
        let captured_at = match (p_stamp, s_stamp) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        InventorySnapshot {
            providers: p.map(|h| h.rows.clone()).unwrap_or_default(),
            repos: s.as_ref().and_then(|h| h.repos),
            issues_pending: s.as_ref().and_then(|h| h.issues_pending),
            issues_open: s.as_ref().and_then(|h| h.issues_open),
            issues_capped: s.as_ref().map(|h| h.issues_capped).unwrap_or(false),
            captured_at,
        }
    }
}
```

Add the test-only stamp setters behind `#[cfg(test)]`:

```rust
    #[cfg(test)]
    pub fn set_provider_stamp_for_test(&self, t: DateTime<Utc>) {
        self.providers.write().expect("lock").stamp = Some(t);
    }

    #[cfg(test)]
    pub fn set_scm_stamp_for_test(&self, t: DateTime<Utc>) {
        self.scm_cache.write().expect("lock").stamp = Some(t);
    }
```

Define `ScmDeps` as whatever `discover_tick_autoflows` and `Registry::discover` need — at minimum the global dir path, a `RepoRegistryStore`, an `AutoflowClaimStore`, a `CredentialResolver`, a `RepoLister`, and the resolved `Config`. Construct it in `cp serve` where those already exist; do not re-resolve credentials here.

Update both constructors for the new fields: `CpFleetInventory::new` takes `(registry: Arc<ProviderRegistry>, scm: ScmDeps)` and sets both halves to `Default::default()`; `new_for_test` sets `registry: None, scm: None` and both halves default. Every existing Plan 2 test keeps passing because an absent half contributes no rows and no stamp.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli cp_inventory`
Expected: PASS, including Plan 2's existing tests (update `cp serve`'s call site from `refresh()` to `refresh_providers()`).

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cp_inventory.rs
rustfmt --edition 2021 crates/rupu-cli/src/cmd/cp.rs
git add crates/rupu-cli/src/cp_inventory.rs crates/rupu-cli/src/cmd/cp.rs
git commit -m "refactor(cli): split the fleet inventory cache into provider and scm halves"
```

---

### Task 2: Count repos and open issues

**Files:**
- Modify: `crates/rupu-cli/src/cp_inventory.rs`

**Interfaces:**
- Produces: `refresh_scm(&self)`; `pub const ISSUE_FETCH_CAP: u32 = 500`; `fn tally_open_issues(counts: &[(u64, bool)]) -> (Option<u64>, bool)`.

- [ ] **Step 1: Write the failing tests**

The network half is exercised by the manual verification at the end; the **cap semantics** are pure and get unit tests. Append to `mod tests`:

```rust
    /// Per-repo counts sum, and a single capped repo makes the total a floor.
    #[test]
    fn tally_marks_capped_when_any_repo_hit_the_cap() {
        let (total, capped) = tally_open_issues(&[(186, false), (500, true), (12, false)]);
        assert_eq!(total, Some(698));
        assert!(capped, "one repo at the cap makes the whole total a floor");
    }

    #[test]
    fn tally_is_not_capped_when_no_repo_hit_the_cap() {
        let (total, capped) = tally_open_issues(&[(3, false), (9, false)]);
        assert_eq!(total, Some(12));
        assert!(!capped);
    }

    /// No repos read at all is not "zero open issues" — report nothing.
    #[test]
    fn tally_of_no_repos_reports_none() {
        let (total, capped) = tally_open_issues(&[]);
        assert_eq!(total, None);
        assert!(!capped);
    }

    /// A genuine zero survives as a zero.
    #[test]
    fn tally_of_repos_with_no_open_issues_is_some_zero() {
        let (total, _) = tally_open_issues(&[(0, false), (0, false)]);
        assert_eq!(total, Some(0));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli tally_`
Expected: FAIL — `tally_open_issues` not found.

- [ ] **Step 3: Implement**

```rust
/// Per-repo issue fetch cap. Hitting it means the open count is a floor, not
/// a total — `issues_capped` says so and the UI renders `312+`.
///
/// Tunable: 500 is an initial value chosen to keep a refresh cheap for
/// ordinary repos. If real orgs routinely exceed it, raise it here — the
/// floor semantics stay correct either way.
pub const ISSUE_FETCH_CAP: u32 = 500;

/// Sum per-repo `(open_count, hit_cap)` pairs.
///
/// Returns `None` for an empty input: zero repos read is "we know nothing",
/// not "you have no open issues". Any capped repo makes the total a floor.
fn tally_open_issues(counts: &[(u64, bool)]) -> (Option<u64>, bool) {
    if counts.is_empty() {
        return (None, false);
    }
    let total = counts.iter().map(|(n, _)| n).sum();
    let capped = counts.iter().any(|(_, c)| *c);
    (Some(total), capped)
}
```

Add `refresh_scm`, reusing the repo listing the CP already has (`CpRepoLister`, `crates/rupu-cli/src/cp_repos.rs`) rather than a second enumeration:

```rust
    /// Refresh the SCM half: connected repos, open-issue totals, and the
    /// autoflow-pending backlog. Every failure degrades that ONE field to
    /// `None` with a warn — a rate-limited GitHub must not blank the repo
    /// count it already returned.
    pub async fn refresh_scm(&self) {
        let Some(deps) = &self.scm else {
            return;
        };

        let repo_entries = match deps.repo_lister.list().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "fleet inventory: repo listing failed; repos unreported");
                None
            }
        };

        // One issue listing per connected repo, capped. A per-repo failure
        // drops that repo from the tally rather than the whole count.
        let mut per_repo: Vec<(u64, bool)> = Vec::new();
        if let Some(entries) = &repo_entries {
            for entry in entries {
                match deps.count_open_issues(entry, ISSUE_FETCH_CAP).await {
                    Ok(n) => per_repo.push((n, n as u32 >= ISSUE_FETCH_CAP)),
                    Err(e) => {
                        tracing::warn!(repo = %entry.repo, error = %e, "fleet inventory: issue listing failed; repo omitted from the open tally")
                    }
                }
            }
        }
        let (issues_open, issues_capped) = tally_open_issues(&per_repo);

        if let Ok(mut c) = self.scm_cache.write() {
            c.repos = repo_entries.map(|v| v.len() as u64);
            c.issues_open = issues_open;
            c.issues_capped = issues_capped;
            c.stamp = Some(Utc::now());
        }
    }
```

Implement `ScmDeps::count_open_issues(entry, cap)` by resolving the tracker for `entry.platform` and calling `IssueConnector::list_issues(project, IssueFilter { state: Some(IssueState::Open), labels: vec![], author: None, limit: Some(cap) })`, returning the length. Reuse the existing platform→tracker and repo→project mapping (`issue_discovery_target` in `cmd/autoflow.rs` is the reference) rather than hand-formatting `"owner/name"`.

Leave `issues_pending` untouched here — Task 3 sets it in the same pass.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli tally_`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cp_inventory.rs
git add crates/rupu-cli/src/cp_inventory.rs
git commit -m "feat(cli): count connected repos and capped open-issue totals"
```

---

### Task 3: Derive the pending backlog

**Files:**
- Modify: `crates/rupu-cli/src/cp_inventory.rs`
- Modify: `crates/rupu-cli/src/cmd/autoflow.rs` (visibility only, if needed)

**Interfaces:**
- Consumes: `discover_tick_autoflows`, `collect_issue_matches`, `IssueMatch`, `AutoflowClaimStore`, `ClaimStatus`.
- Produces: `fn pending_after_claims(issue_refs: &[String], claim_store: &AutoflowClaimStore) -> u64`. It takes issue-ref strings, **not** `&[IssueMatch]`: only `IssueMatch::issue_ref_text` is read, and the string form makes the function testable without building a `ResolvedAutoflowWorkflow` fixture. The caller maps `matches.iter().map(|m| m.issue_ref_text.clone())`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    /// "Pending" is work rupu is ABOUT to pick up. An issue already claimed
    /// and in flight is work rupu has ALREADY picked up — it belongs to the
    /// claims count, not this one. `Complete` / `Released` claims are finished
    /// or handed back, so their issues are pending again if still selected.
    #[test]
    fn pending_excludes_issues_with_an_in_flight_claim() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let claim_store = rupu_workspace::AutoflowClaimStore {
            root: tmp.path().to_path_buf(),
        };
        write_claim(&claim_store, "github:o/r/issues/1", "running");
        write_claim(&claim_store, "github:o/r/issues/2", "complete");
        write_claim(&claim_store, "github:o/r/issues/3", "released");

        let refs: Vec<String> = [
            "github:o/r/issues/1",
            "github:o/r/issues/2",
            "github:o/r/issues/3",
            "github:o/r/issues/4",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert_eq!(
            pending_after_claims(&refs, &claim_store),
            3,
            "issue 1 is in flight; 2 and 3 are finished/handed back and 4 was never claimed"
        );
    }
```

Write the `write_claim` helper against the real `AutoflowClaimRecord` shape (`crates/rupu-workspace/src/autoflow_claim.rs`) — reuse the TOML fixture form from Plan 1's `claims_active_excludes_complete_and_released` test, and note that `AutoflowClaimStore::load` is keyed through `issue_key` (`rupu_workspace::autoflow_claim_store::issue_key`), so the on-disk directory name must be that key, not the raw ref.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli pending_excludes`
Expected: FAIL — `pending_after_claims` not found.

- [ ] **Step 3: Implement**

```rust
/// Issue refs that are selected but not already in flight.
///
/// The claim rule matches `ensure_manual_run_can_take_claim`
/// (`cmd/autoflow.rs`): `Complete` and `Released` are terminal, so their
/// issues are available again; every other status means a run already owns
/// this issue.
fn pending_after_claims(issue_refs: &[String], claim_store: &AutoflowClaimStore) -> u64 {
    issue_refs
        .iter()
        .filter(|r| match claim_store.load(r) {
            Ok(Some(claim)) => {
                matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released)
            }
            // No claim at all — genuinely pending.
            Ok(None) => true,
            // An unreadable claim record must not silently inflate the
            // backlog; treat it as in flight and warn.
            Err(e) => {
                tracing::warn!(issue_ref = %r, error = %e, "fleet inventory: unreadable claim; excluding from pending");
                false
            }
        })
        .count() as u64
}
```

In `refresh_scm`, after the open-issue tally, derive pending through the cron tick's own path:

```rust
        // Derived by the SAME pair the cron tick uses, so the strip's number
        // is exactly what rupu will pick up. A second matcher living in the CP
        // could drift from this one, and a strip that disagrees with the
        // scheduler is worse than no strip.
        let issues_pending = match discover_tick_autoflows(&deps.global_dir, &deps.repo_store) {
            Ok(discovered) => {
                match collect_issue_matches(&discovered, deps.resolver.as_ref()).await {
                    Ok(matches) => {
                        let refs: Vec<String> =
                            matches.iter().map(|m| m.issue_ref_text.clone()).collect();
                        Some(pending_after_claims(&refs, &deps.claim_store))
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "fleet inventory: issue matching failed; issues_pending unreported");
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "fleet inventory: autoflow discovery failed; issues_pending unreported");
                None
            }
        };
```

and set `c.issues_pending = issues_pending;` in the cache write.

If `discover_tick_autoflows` / `collect_issue_matches` / `IssueMatch` are `pub(crate)` in a module not reachable from `cp_inventory`, widen them to `pub(crate)` at the crate root — do **not** copy their bodies.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rupu-cli pending_excludes`
Expected: PASS.

- [ ] **Step 5: Spawn the SCM refresh task**

In `crates/rupu-cli/src/cmd/cp.rs`, beside the provider refresher Plan 2 added:

```rust
    {
        let inv = std::sync::Arc::clone(&inventory);
        tokio::spawn(async move {
            loop {
                inv.refresh_scm().await;
                tokio::time::sleep(std::time::Duration::from_secs(
                    crate::cp_inventory::SCM_TTL_SECS,
                ))
                .await;
            }
        });
    }
```

- [ ] **Step 6: Run the full suite**

Run: `cargo test -p rupu-cli && cargo test -p rupu-cp && cargo clippy -p rupu-cli -- -D warnings`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cp_inventory.rs
rustfmt --edition 2021 crates/rupu-cli/src/cmd/cp.rs
rustfmt --edition 2021 crates/rupu-cli/src/cmd/autoflow.rs
git add crates/rupu-cli/src/
git commit -m "feat(cli): autoflow-pending backlog via the cron tick's own matcher"
```

---

## Verification

- [ ] `cargo clippy -p rupu-cli -p rupu-cp -- -D warnings` clean.
- [ ] Start `rupu cp serve` with GitHub credentials: within one SCM refresh the strip shows a real repo count and `N pending (M open)`.
- [ ] Cross-check the pending number against `rupu autoflow` on the same checkout — **they must agree**. A disagreement means the adapter is not using the cron tick's path and is the one bug this plan exists to prevent.
- [ ] Claim an issue, then wait for a refresh: `pending` drops by one and `claimed` rises by one.
- [ ] Point at a repo with more than 500 open issues: the strip renders `500+ open` and does not claim a total.
- [ ] Revoke SCM credentials and restart: repos and issues render as em-dashes; the provider and local counts are unaffected.
- [ ] Confirm the first page load after startup is not blocked by the SCM refresh — the strip shows em-dashes, then fills.
- [ ] `make cp-web` before any release build (the binary embeds `web/dist`).
