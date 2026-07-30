# Dashboard fleet strip — design

**Date:** 2026-07-30
**Status:** Implemented — PR #569 (Plans 1-3). See "As-built deviations" below.
**Surface:** `rupu-cp` (API + web)

## Problem

Moving spend to `/usage` left the Dashboard narrow rather than empty. Every
element on the page derives from a single source — the run store — so the page
answers exactly one question: "how are runs doing." It says nothing about
whether the installation is wired up: which repos are connected, whether the
configured providers actually work, how many autoflows are enabled, how much
issue backlog is waiting to be picked up.

## Decision

The Dashboard stays an **ops monitor**. It gains a thin **fleet strip** of
inventory facts at the bottom — supporting cast for the operational story, not
a competing genre. Inventory takes visual weight only when something is
*wrong*.

Rejected alternatives:

- **Triage cockpit** (reorient the page around actionable state, add a live
  event feed). The live-event surface is its own approved design; duplicating a
  feed here would fragment it.
- **Fleet/system health overview** (make inventory the subject). Demotes the
  run data that operators actually leave the tab open for.

## §1 — Composition

The existing stack is unchanged; one band is appended:

```
header (range · host freshness strip · Spend →)
KeyPointTiles          active / awaiting / paused / failed / findings
two charts             outcomes over time · throughput by trigger
CycleSummaryLine       N cycles · N clean · N with failures        /runs →
─────────────────────────────────────────────────────────────────────────
FleetStrip  (new)      7 repos · 4 providers (1 unhealthy) · 6 autoflows
                       (2 off) · 3 workers · 9 claimed · 14 pending (312 open)
```

`FleetStrip` is one dim row. Each segment links to the page that owns it:

| Segment    | Links to            |
|------------|---------------------|
| repos      | `/settings`         |
| providers  | `/settings`         |
| autoflows  | `/autoflows`        |
| workers    | `/workers`          |
| claimed    | `/runs/autoflows`   |
| issues     | `/settings`         |

Repos and issues link to Settings deliberately: neither has a CP list page, and
growing one is out of scope for this design. If a `/repos` page is built later,
those two links move to it — that is a link change, not a contract change.

**Weighting rule.** Segments render dim by default. A segment takes red weight
only when it reports a fault — currently only `providers_unhealthy > 0`. No
other segment ever takes weight; a count is context, not an alarm.

Like every other block on the page, `FleetStrip` renders from whatever has
arrived. A hung SCM cache leaves its segments reading em-dash; it never blanks
the page.

## §2 — Data contract

The fleet numbers ride the **existing** per-host fan-out. `dashboard_summary`
is already a `HostConnector` trait method with local / http-node / ssh impls,
so adding fields to `DashboardSummary` requires no new connector method.

`DashboardSummary` gains one nested struct:

```rust
pub struct FleetCounts {
    pub repos: Option<u64>,
    pub providers_configured: Option<u64>,
    pub providers_unhealthy: Option<u64>,
    pub autoflows_enabled: Option<u64>,
    pub autoflows_disabled: Option<u64>,
    pub workers: Option<u64>,
    pub claims_active: Option<u64>,   // renamed; see As-built
    pub issues_pending: Option<u64>,
    pub issues_open: Option<u64>,
    /// `issues_open` is a FLOOR, not a total — a repo hit the fetch cap, OR a
    /// repo could not be read at all and was dropped from the tally.
    pub issues_capped: bool,
    /// When the SCM / provider caches behind these numbers were filled.
    /// Distinct from the summary's own `captured_at`, which stamps the
    /// run-store read.
    pub inventory_captured_at: Option<DateTime<Utc>>,
}
```

`FleetCounts` is a field on `DashboardSummary`. Every count is `Option<u64>`.

**Aggregation follows the `findings_open` rule exactly.** The aggregator in
`api::dashboard` sums only `Some` values and sets `fleet_partial` on the
response when any reporting host contributed `None` for a field it summed.
`None` is never summed as `0`, and a partial sum is never presented as a fleet
total. `Some(0)` is a genuine zero and renders as `0`; `None` renders as an
em-dash.

**Per-transport reporting.** Local and http-node report every field. SSH
reports what its CLI-invoked reads can source and `None` for the rest — the
partial marker is the honest outcome, not a bug to work around.

## §3 — Caches

Two caches live in `rupu cp serve`. Both are **read-only from the dashboard's
perspective**: building a summary reads a cache and never blocks on the
network. Neither cache is populated without `cp serve` running; in that case
the dependent fields are `None`.

### ProviderProbeCache

- **TTL:** ~5 minutes, refreshed by a background task.
- **Probe:** one cheap authenticated call per configured provider (models-list
  or the provider's equivalent).
- **State per provider:** `Ok` | `AuthFailed { status }` | `Unreachable { err }`
  | `NeverProbed`, each with its own stamp.
- **Mapping:** `providers_unhealthy` counts `AuthFailed + Unreachable`.
  `NeverProbed` counts as **neither** healthy nor unhealthy — it is an absence
  of information and must not be rendered as either.
- **Without `cp serve`:** BOTH provider fields are `None` and the strip shows
  an em-dash. (The spec originally had `providers_configured` coming from
  config; as-built it does not — see As-built deviations.)

This is the reason the design does not derive provider health from config
presence alone: a green dot that only means "a key exists in the config file"
asserts something it has not checked.

### ScmInventoryCache

- **TTL:** ~15 minutes — longer than the provider cache because filling it is
  N API calls, one set per connected repo.
- **Source:** the `FleetInventory` port's snapshot (as-built; the spec
  originally called for a separate `IssueLister` port — see As-built
  deviations). `RepoLister` still supplies the repo enumeration.
- **Per-repo fetch cap:** 500 open issues. Hitting the cap sets `issues_capped`
  and the UI renders the open count as a floor (`312+ open`). The cap is a
  tunable constant; 500 is the initial value and should move if real orgs
  exceed it.

### Pending derivation

As-built, the **`cp serve` adapter** derives `issues_pending` by calling the
same pair the cron tick uses — `discover_tick_autoflows` +
`collect_issue_matches` — and subtracting in-flight claims. Those already
apply the full selector (`states`, `labels_all`, `labels_any`, `labels_none`),
the author allowlist, and `selector.limit`.

The spec originally put this derivation in the CP. Reimplementing the matcher
there would create a second one that can drift from the scheduler, and a strip
reading "14 pending" while rupu picks up 9 is worse than no number. `issues_pending` therefore means "work rupu is about to pick up,"
and is correctly `0` for a repo rupu does not automate. `issues_open` is shown
alongside as context.

## §4 — Testing

- **Merge discipline.** `None` never sums as `0`; any `None` among reporting
  hosts sets `fleet_partial`. Mirrors the existing `findings_open` merge tests.
- **Provider state mapping.** `AuthFailed` and `Unreachable` both increment
  `providers_unhealthy`; `NeverProbed` increments neither.
- **Pending derivation.** Selector matching (`states` / `labels_all` /
  `labels_none`) and claim subtraction against fixture issues. Pure function,
  no network.
- **Cap handling.** A repo at the fetch cap sets `issues_capped`, and the count
  is presented as a floor.
- **`FleetStrip` component.** Renders em-dash for `null`, `+` suffix when
  capped, a `(partial)` marker when `fleet_partial`, and red weight *only* for
  unhealthy providers.
- **Progressive render.** The Dashboard paints the strip from whatever has
  arrived; an unresolved SCM cache does not blank the page.

## Out of scope

- A `/repos` CP list page. Repos and issues link to `/settings` until one
  exists.
- A live event feed on the Dashboard — owned by the separate live-events
  design.
- Findings severity breakdown. The findings tile keeps its current bare count;
  splitting it is an ops-tile change, not a fleet-strip change.

## As-built deviations

All three landed in PR #569 and are documented in the plans.

1. **`claims_queued` → `claims_active`.** `ClaimStatus` has no `Queued`
   variant; the field counts every claim except `Complete` / `Released`.
2. **`providers_configured` is probe-sourced, not config-sourced.**
   `config.providers` is a per-provider *knob-override* map, so an operator
   authenticated via OAuth with no `[providers.x]` block would have counted as
   zero. The honest source is the credential store, which lives behind
   rupu-providers — a crate rupu-cp does not depend on. Both provider fields
   therefore arrive through the `FleetInventory` port and are `None` without
   `cp serve`.
3. **No separate `IssueLister` port; pending derived in the adapter.** The
   `FleetInventory` snapshot already carries repos and issues across the same
   boundary on the same cache with the same staleness stamp. See §3.

One correction found by running against a real account: a repo that cannot be
read (e.g. `451 Repository access blocked`) is dropped from the open-issue
tally, so it must ALSO set `issues_capped` — otherwise the count silently
shrinks while presenting itself as complete.
