# Dashboard Reshape — Plan 2b (data) + Plan 1-rev (page)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Reshape the (already-merged) multi-host dashboard from per-item lists to aggregate key-points, and make it load async so no host or panel ever blocks another.

**Why:** Operator review of the first build — *"key points, not lists; this is a detailed table, not a dashboard"* — and it took ~10s to load because the fan-out blocked on an unreachable SSH host (`?host=local` alone was 0.26s). See the revised spec §5 (`docs/superpowers/specs/2026-07-16-rupu-cp-dashboard-redesign-design.md`).

**Architecture:** The server's per-host `dashboard_summary()` returns aggregate-only data (no row arrays). The page loads each host independently and async, painting local first and merging remotes as they answer; a small pure client function combines the already-correct per-host summaries.

**Tech Stack:** Rust (axum, serde, chrono), React + TS + Recharts (no new deps), Vitest.

## Global Constraints

- Workspace deps only; NEVER a version literal in a crate Cargo.toml. **NO new npm deps** — Recharts covers every graph.
- rupu-cp stays READ-ONLY. `rupu-cli` stays thin.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden. `thiserror` for libs, `anyhow` for the CLI.
- **NEVER run `cargo fmt` in any form.** `rustfmt --edition 2021 <path>`, one file at a time, never a crate root or `mod.rs`. `git diff --stat` after, revert stray files.
- **NEVER hardcode a color literal** in web code — `useThemeColors()` / `--c-*` tokens only.
- Reports MUST paste literal command output. A prior subagent claimed clippy clean without running it.
- rustc resolves 1.95 (Homebrew) vs pinned 1.88. `cargo clippy -p rupu-cp --all-targets` has exactly **5** pre-existing errors — verify the count is unchanged; do not chase them.
- Do NOT `git push` (push.default=matching force-updates unrelated branches — the controller pushes with an explicit refspec). No `git checkout <commit>`, no detached HEAD, no `git stash`.

---

## Phase A — Rust data reshape (Plan 2b)

### Task R1: Reshape the `DashboardSummary` DTOs

**Files:** `crates/rupu-cp/src/host/dashboard_summary.rs` (+ its inline tests)

**Interfaces produced (consumed by R2–R5 and Phase B):**

```rust
/// Runs STARTED in a bucket, split by trigger. Same day-key alignment as TerminalBucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputBucket {
    pub ts: DateTime<Utc>,
    pub manual: u64,
    pub cron: u64,
    pub event: u64,
}

/// Scalar cycle summary — the one line of cycle numbers (spec §5.5). NOT a row array.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CycleCounts {
    pub total: u64,
    /// `None` when the host cannot report the ran/failed breakdown (SSH). Never fabricate 0.
    pub clean: Option<u64>,
    pub with_failures: Option<u64>,
}

/// The "Active now" key point (spec §5.2): the longest currently-running run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveLongest {
    pub run_id: String,
    pub workflow_name: String,
    pub age_ms: u64,
}
```

`DashboardSummary` becomes:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub active: ActiveCounts,
    /// The single longest-running run, or None if nothing is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_longest: Option<ActiveLongest>,
    pub terminal_buckets: Vec<TerminalBucket>,
    pub throughput_buckets: Vec<ThroughputBucket>,
    pub cycles: CycleCounts,
    pub findings_open: Option<u64>,
    pub captured_at: DateTime<Utc>,
}
```

**DELETE:** `ActiveRunBar`, `CycleRollup`, `CycleRun`, `RecentRun`. Keep `ActiveCounts`, `TerminalBucket`, `DashboardRange`.

- [ ] **Step 1: Write failing tests** — `ThroughputBucket` / `CycleCounts` / `ActiveLongest` serialize with the right field names; `CycleCounts::clean = None` serializes as absent (or null), never `0`; a `DashboardSummary` round-trips.
- [ ] **Step 2: Run — fail** (`cargo test -p rupu-cp dashboard_summary`).
- [ ] **Step 3: Implement** the DTOs above; delete the four row DTOs.
- [ ] **Step 4: `cargo build -p rupu-cp` WILL fail** in `local.rs` / `ssh.rs` / `summary_build.rs` / `api/dashboard.rs` — they construct the deleted types. That is expected; R2–R5 fix each. Do NOT patch them in this task beyond what's needed to keep THIS file's tests compiling. Report the break.
- [ ] **Step 5: Commit.**

---

### Task R2: Rewrite `build_summary` (local) for the aggregate shape

**Files:** `crates/rupu-cp/src/host/summary_build.rs`, `crates/rupu-cp/src/host/local.rs`

**Everything the new shape needs is already gathered** — `build_summary` already iterates every run (for `active` counts and `terminal_buckets`) and already reads cycle rollups. This task changes the *outputs*, not the inputs.

In the existing per-run loop, additionally accumulate:
- **throughput:** for every run in range, `throughput_buckets[day_key(started_at)].{manual|cron|event} += 1` keyed on `run.trigger_str()`. Reuse the `pub(crate) fn day_key` promoted during the C1 fix and `fill_bucket_grid`'s zero-fill discipline (add a throughput-grid fill mirroring the terminal one, or generalize the fill).
- **active_longest:** track the running run with the oldest `started_at`; emit `ActiveLongest { run_id, workflow_name, age_ms = now - started_at }`, or `None` if none running.
- **cycles:** from the cycle rollups it already reads, compute `CycleCounts { total, clean: Some(count where failed==0), with_failures: Some(count where failed>0) }`. Local reads real counts, so both are `Some`.

**Delete** the now-dead code: `active_runs` bar construction, the `recent_manual` accumulation + its cycle-exclusion guard, the cycle→run status join. Those served the removed swimlane/feed.

- [ ] **Step 1: Rewrite the `summary_build` tests** for the new shape. Keep/adapt: active-count tally, `Paused` counts as active, terminal bucket exclusion of active runs, range filtering, contiguous (zero-filled) terminal grid. **Add:**
  - throughput buckets tally by trigger (a manual + a cron run in the same day → `{manual:1, cron:1}`);
  - throughput grid is contiguous/zero-filled just like terminal;
  - `active_longest` picks the oldest running run (and is `None` with nothing running);
  - `CycleCounts` splits clean vs with_failures.
- [ ] **Step 2: Run — fail.**
- [ ] **Step 3: Implement.** Then wire `LocalHostConnector::dashboard_summary` in `local.rs` to the new shape (the cycle-rollup helper now returns counts, not rows — simplify `collect_cycle_rollups` to a counter or keep it and count in `build_summary`, your call; keep the warn-on-error degradation from the earlier fix).
- [ ] **Step 4:** `cargo test -p rupu-cp summary_build` green; `cargo clippy` count still 5.
- [ ] **Step 5: Commit.**

---

### Task R3: Rewrite SSH `dashboard_summary` for the aggregate shape

**Files:** `crates/rupu-cp/src/host/ssh.rs`

The SSH path already fetches `run_rows` (RunListRow-shaped: `id`, `status`, `started_at`, `trigger`, …) and autoflow history. Reshape its output exactly as R2 did for local, from the same two sources:
- throughput buckets from `run_rows` by `trigger` (day-key aligned — reuse `day_key`, matching the C1 fix that made SSH buckets midnight-aligned);
- `active_longest` from the non-terminal `run_rows` with the oldest `started_at`;
- `CycleCounts`: `total` = history rows in range. **`clean`/`with_failures` stay `None`** — the CLI's autoflow history has no ran/failed breakdown (established during the final-review I4 fix). Do NOT fabricate them.
- `findings_open: None` (unchanged — SSH has no findings surface).

**Delete** the SSH `active_runs` / `recent_manual` / cycle-`runs` construction and the `cycle_of` guard.

Keep: the two-round-trip bound, the warn-and-degrade on history failure, and `Unsupported` mapping for an old host.

- [ ] **Step 1: Rewrite the SSH summary test** to assert the new shape (throughput tallied, active_longest set, `cycles.clean == None`, captured_at stamped). Reuse the `StubExec` + `make_conn` fixtures.
- [ ] **Step 2: Run — fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** `cargo test -p rupu-cp ssh` green; clippy still 5.
- [ ] **Step 5: Commit.**

---

### Task R4: HTTP connector + server merge for the aggregate shape

**Files:** `crates/rupu-cp/src/host/http.rs`, `crates/rupu-cp/src/api/dashboard.rs`

**HTTP** (`http.rs`): the connector deserializes the flattened body as `DashboardSummary` — the type changed, so it recompiles for free. Keep the I2 fix: reject when the scoped host's `hosts[]` entry is not `ok` (still parse `hosts[]`). Verify the httpmock stub body matches the new shape.

**Server merge** (`api/dashboard.rs` `merge_dashboard_summaries`): update for the new fields —
- `active`: sum (unchanged);
- `active_longest`: take the **max by `age_ms`** across hosts (the single longest-running run fleet-wide);
- `terminal_buckets` + `throughput_buckets`: merge by `ts` day-key, then `fill_bucket_grid` BOTH (the C1 discipline — day-key-normalize on merge; add the throughput grid fill);
- `cycles`: `total` sums; `clean`/`with_failures` sum only the `Some` contributors, and if any reporting host had `None`, the merged value stays `None` (same "not reported ≠ 0" rule as findings). Alternatively carry a `cycles_partial` flag mirroring `findings_partial`; pick one and document it.
- `findings_open` + `findings_partial`: unchanged from the shipped logic.
- `captured_at`: oldest reporting host (unchanged).

- [ ] **Step 1:** Update the C1 seam test + the freshness/partial tests in `tests/dashboard.rs` and the httpmock test in `tests/host_http.rs` to the new shape. **Add:** a throughput-bucket merge assertion (local-shaped + SSH-shaped throughput bucket, same day → one merged bucket, summed) — the throughput analogue of the C1 seam test.
- [ ] **Step 2: Run — fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** `cargo test -p rupu-cp dashboard` + `host_http` green; `cargo test -p rupu-cp` fully green; clippy still 5. Manual: `cp serve` on a scratch port, `curl '.../api/dashboard?range=all' | head -c 400` — paste it; assert `throughput_buckets` present, no `active_runs`/`cycles[]`/`recent_manual`.
- [ ] **Step 5: Commit.**

---

### Task R5: Bounded SSH connect timeout

**Files:** `crates/rupu-cp/src/host/ssh.rs` (the `RemoteExec` / `ssh` invocation)

The SSH `dashboard_summary` path is unbounded, so a dead host stalls ~10s on the OS connect default. Add a bounded connect timeout (~3s) to the `ssh` command the connector runs — pass `-o ConnectTimeout=3` (and `-o BatchMode=yes` if not already, so it never hangs on a password prompt) wherever the connector builds its `ssh` argv (`build_remote_command` / the `RemoteExec` impl). This is the SSH analogue of the HTTP 5s/30s bound already shipped.

**Do NOT** bound the launch-path pump's long-lived `tail -F` ssh — that is a streaming connection, not a probe. Only the short request/response calls (`remote_json_rows` / `remote_json_item`) get the connect timeout.

- [ ] **Step 1:** Test that the built ssh command carries `ConnectTimeout` (a `StubExec` can't exercise real ssh, so assert on the argv/command string the connector constructs — find where `ssh` args are assembled and unit-test that function).
- [ ] **Step 2: Run — fail.** **Step 3: Implement. Step 4: verify (clippy 5, ssh tests green). Step 5: Commit.**

**Real-host note:** the controller will separately confirm a dead host now resolves in ~3s, not ~10s, once the remote host is reachable. Do not block on that here.

---

### Task R6: Cheap host-list endpoint (no probe)

**Files:** `crates/rupu-cp/src/api/hosts.rs`, `crates/rupu-cp/tests/`

Add a fast route that returns the *registered* hosts from `HostRegistry::list_hosts()` (store read, no `info()` probe): `{ id, name, transport_kind }` per host, local first. Either `GET /api/hosts?probe=false` (branch before the probe fan-out) or a dedicated `GET /api/hosts/registered`. The page uses this to render the freshness strip instantly and to know which `?host=<id>` calls to issue.

- [ ] **Step 1:** Test: the endpoint returns in well under the probe path's time and lists local + any registered remote WITHOUT calling `info()` (seed a remote whose `info()` would hang — e.g. an unreachable SSH host — and assert the endpoint still returns promptly with that host listed). Use the existing `spawn_server_with_remote` helper.
- [ ] **Step 2–5:** fail → implement → verify (clippy 5) → commit.

---

## Phase B — Page rework (Plan 1-rev)

### Task P1: TS types + client for the aggregate shape

**Files:** `crates/rupu-cp/web/src/lib/api.ts`, `src/lib/api.test.ts` (exists — append)

Replace the row types with the aggregate ones. **Read `crates/rupu-cp/src/host/dashboard_summary.rs` (post-R1) as the source of truth.**

```ts
export interface ThroughputBucket { ts: string; manual: number; cron: number; event: number; }
export interface CycleCounts { total: number; clean: number | null; with_failures: number | null; }
export interface ActiveLongest { run_id: string; workflow_name: string; age_ms: number; }

export interface DashboardSummary {   // one host's contribution
  active: ActiveCounts;
  active_longest?: ActiveLongest | null;
  terminal_buckets: TerminalBucket[];
  throughput_buckets: ThroughputBucket[];
  cycles: CycleCounts;
  findings_open: number | null;
  captured_at: string;
}
export interface DashboardResponse extends DashboardSummary {   // merged, flattened + hosts[]
  hosts: HostFreshness[];
  findings_partial: boolean;
  // cycles_partial?: boolean;  // if R4 chose the flag approach
}
```

**DELETE** `ActiveRunBar`, `CycleRollup`, `CycleRun`, `DashboardRecentRun`. Add a client method for the cheap host list (R6): `api.getRegisteredHosts()`. Keep `getDashboard(range, host?)` — it already accepts `?host=`.

- [ ] TDD the `getRegisteredHosts` URL + `getDashboard(range,'local')` host param. `tsc --noEmit` errors confined to files later tasks rewrite are expected; report them. Commit.

### Task P2: Per-host async loading hook + client merge

**Files:** `src/lib/dashboard/useDashboardData.ts` (rewrite), `src/lib/dashboard/mergeSummaries.ts` (new) + tests. **DELETE** `src/lib/dashboard/feed.ts`, `feed.test.ts`, `swimlane.ts`, `swimlane.test.ts`.

`mergeSummaries(byHost: Map<string, DashboardSummary>): DashboardSummary` — the pure combine described in spec §5.4: sum `active`, max `active_longest` by `age_ms`, merge both bucket series by day-key (a co-located test = the client seam test: local-shaped + ssh-shaped same-day bucket → one merged), sum `cycles`/`findings` with the `Some`-only + partial rule, oldest `captured_at`. **Combining correct inputs, never deriving from events.**

`useDashboardData(range)` rewrite — the load-time fix:
1. `getRegisteredHosts()` → seed a `Map<hostId, {state:'loading'|'ok'|'unavailable', summary?}>`; render the strip immediately.
2. Fire `getDashboard(range,'local')` AND each `getDashboard(range, remoteId)` **independently, no shared await**; each resolves its own host's slice on arrival (or flips to `unavailable` on error/timeout).
3. Recompute the merged view via `mergeSummaries` whenever any host's slice changes.
4. Keep the local SSE invalidation (debounced) → refetch `?host=local` only. Keep the visibility-gated reconciling poll. Keep stale-on-error.

- [ ] TDD: `mergeSummaries` cases (incl. the seam test + partial rules); `coalesce` retained; **a hung remote host does not delay local** (mock one host's fetch to never resolve; assert local slice + strip render). Commit.

### Task P3: Key-point + throughput components; delete swimlane/feed

**Files:** create `src/components/dashboard/KeyPointTiles.tsx`, `ThroughputChart.tsx`, `CycleSummaryLine.tsx` (+ tests). **DELETE** `Swimlane.tsx`, `ActivityFeed.tsx` (+ their tests). Keep `HostFreshnessStrip`, `AttentionRow`, `ActiveStatusTiles`, `TerminalTrend` (adapt props as needed).

- **KeyPointTiles** — Awaiting you / Paused (weight when nonzero) · Failed (+ sparkline) · Success rate · **Active now** (`active.running` + `active_longest` → "3 · longest 2h14m — nightly-review", linking `/runs`) · Open findings. `null`→em-dash; partial→"(partial)". Recharts sparklines; colors via `useThemeColors()`.
- **ThroughputChart** — Recharts stacked area/bar of `throughput_buckets` by manual/cron/event, colors from the `TriggerChip` palette.
- **CycleSummaryLine** — `cycles.total` cycles · `clean ?? '—'` clean · `with_failures ?? '—'` with failures · "see all →" `/runs`.

- [ ] TDD each (fireEvent, not user-event — not a dep; include `// @vitest-environment jsdom` + jest-dom import + `afterEach(cleanup)` per repo config). Commit.

### Task P4: Rewrite the page

**Files:** `src/pages/Dashboard.tsx`, `Dashboard.test.tsx`

Compose per spec §5.1: header (range + freshness strip) → key-point tiles → split status (active tiles + terminal trend) → throughput chart + cycle line. No swimlane, no feed, no "Spend →" (that's Plan 3). Consume `useDashboardData`; each block renders from whatever slices have arrived. **`tsc --noEmit` must end fully clean; `npm run build` must succeed.**

- [ ] TDD (renders strip + tiles from a mocked payload; subscribes for invalidation). **Step: browser validation is REQUIRED** — controller drives it (per CLAUDE.md; subagents can't validate GPUI/DOM rendering). Commit.

---

## Definition of Done
- `cargo test -p rupu-cp` fully green; clippy at baseline 5; `cargo test -p rupu-cli` unaffected.
- API payload for `range=all` carries `throughput_buckets` + scalar `cycles` + `active_longest`, and NO `active_runs`/`cycles[]`/`recent_manual` arrays.
- `?host=local` returns in <0.5s; a dead remote host never delays it; the freshness strip flips it to `unavailable` on a bounded (~3s) timeout.
- Frontend: `npx vitest run`, `tsc --noEmit`, `npm run build` all clean; NO new deps; no color literals; `feed.ts`/`swimlane.ts`/`Swimlane.tsx`/`ActivityFeed.tsx` deleted.
- Browser-validated by the controller in both themes: no per-item list on the page; loads without the ~10s stall.
