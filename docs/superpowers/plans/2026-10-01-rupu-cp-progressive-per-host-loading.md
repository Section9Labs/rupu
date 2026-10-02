# CP Progressive Per-Host Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every Activity table, the Usage page and the ⌘K run list paint local data immediately and merge each remote host in as it answers, with honest per-host state. SSH load on remote hosts drops instead of rising, even though All hosts becomes the default.

**Architecture:**
- **Web.** One request per host via the existing `?host=<id>` single-host paths. A pure watermark merge keeps infinite scroll correctly ordered across hosts. A `PerHostListEngine` class serializes requests per host and runs catch-up. A thin React hook wraps it.
- **Server.** Three small changes:
  - honest 502/501 statuses on the single-host list paths;
  - a 5 s single-flight listing cache inside `SshHostConnector`, cleared on every mutation;
  - nothing else (no new endpoints).

**Tech Stack:** Rust (axum, tokio, futures-util `Shared`), React 18 + TypeScript, vitest + @testing-library/react.

**Status:** complete — executed and merged via Section9Labs/rupu#718. Execution deviations are recorded in the spec's §13.

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md`. Read it before starting any task. Section numbers below (§N) refer to it.

## Global Constraints

- **Branch:** `claude/cp-progressive-per-host-loading` (already created; the spec is committed on it).
- **Never** run bare `git stash` / `git stash pop` (the stash stack is shared with other sessions). Set work aside with a temporary WIP commit instead.
- **Never** run package-wide `cargo fmt`; main is fmt-dirty.
  - Format only files that were clean before you touched them: `crates/rupu-cp/src/api/runs.rs`, `crates/rupu-cp/src/host/ssh.rs`, and the new `crates/rupu-cp/src/host/listing_cache.rs`. Use `rustfmt --edition 2021 <file>`.
  - `crates/rupu-cp/src/api/run_streams.rs`, `crates/rupu-cp/src/api/sessions.rs` and `crates/rupu-cp/src/host/mod.rs` are already dirty. Hand-format your edits to them and never run rustfmt on them.
- Workspace deps only; no version pins in crate `Cargo.toml`. No new crates or npm packages.
- `#![deny(clippy::all)]` is workspace-wide, so `cargo clippy -p rupu-cp --all-targets` must be clean for touched code.
- **Web commands** run from `crates/rupu-cp/web`:
  - tests: `npx vitest run <path>`
  - typecheck: `npx tsc -b`. Run it; `ci.yml` skips it.
- **Rust lib tests:** `cargo test -p rupu-cp --lib <filter>`.
- **Constants (verbatim from the spec):**

  | Constant | Value |
  |---|---|
  | page size | 20 |
  | overlap | 5 |
  | server `MAX_LIMIT` | 200 |
  | local poll | 5 s |
  | remote poll | 60 s, visible-only |
  | SSH listing TTL | 5 s |
  | Usage local tick | 30 s (existing) |

- **Every commit message** ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Live checks** never point at real SSH hosts. Task 18 uses local stub HTTP hosts only.
- **Public repo:** fixtures use invented names (`host-a`, `prod`, `staging`). Never use real host names or assessment data.

---

## File Structure

**Server (Rust, `crates/rupu-cp/src/`)**

| File | Change | Responsibility |
|---|---|---|
| `api/runs.rs` | modify | `host_list_error()` next to `resolve_host()`. Wired into `list_runs` / `list_workflow_runs`. `FakeHostConnector::list_runs` becomes scriptable. |
| `api/run_streams.rs` | modify | Wire `host_list_error()` into `list_agent_runs`, `list_autoflow_runs`, `list_autoflow_events`. |
| `api/sessions.rs` | modify | Wire `host_list_error()` into `list_sessions`. |
| `host/connector.rs` | modify | `#[derive(Clone)]` on `HostConnectorError`, needed so a `Shared` future can hand one error to every waiter. |
| `host/listing_cache.rs` | **create** | `ListingCache`: TTL, single-flight, epoch-guarded `clear()`, `ClearOnDrop`. |
| `host/mod.rs` | modify | `pub mod listing_cache;` |
| `host/ssh.rs` | modify | `listings` field; `exec_rupu_json` free fn; `cached_rows()`. Listing methods go through the cache; every mutation holds a `ClearOnDrop`. |

**Web (`crates/rupu-cp/web/src/`)**

| File | Change | Responsibility |
|---|---|---|
| `lib/perHost/types.ts` | create | `HostSlice`, `RowAccess`, `instantOf`, `rowAccess`, `emptySlice`, `HostSeed` |
| `lib/perHost/watermarkMerge.ts` | create | `coveredOf`, `isGating`, `floorOf`, `compareNewestFirst`, `watermarkMerge`, `displacedCount` |
| `lib/perHost/spliceHead.ts` | create | `withHost`, `sortSlice`, `appendPage`, `spliceHead` |
| `lib/perHost/status.ts` | create | `classifyFailure`, `waitingOn`, `waitingLabel`, `notIncluded`, `pagingFailedHosts`, `toFreshnessEntries` |
| `lib/perHost/engine.ts` | create | `PerHostListEngine` (per-host queues, head / join / catch-up / next / poll) |
| `lib/perHost/usePerHostPagedList.ts` | create | React wrapper: bootstrap, cadence timers, merged output |
| `lib/perHost/testUtils.ts` | create | Test-only helpers: `deferred`, `REG_LOCAL`, `REG_PROD`, `callsFor`, `onlyHost` |
| `components/lists/PerHostStatus.tsx` | create | `PerHostStrip`, `perHostFooterText`, `PagingFailures` |
| `lib/usage/mergeUsage.ts` | create | `rollupSummaries`, `mergeUnpriced`, `mergeUsage` |
| `lib/usage/useUsageData.ts` | create | Per-host `/api/usage` loader |
| `components/HostSelect.tsx` | modify | The `allowAll` branch reads `getRegisteredHosts` |
| `pages/runs/WorkflowRuns.tsx`, `pages/runs/AgentRuns.tsx`, `pages/runs/AutoflowRuns.tsx`, `pages/Sessions.tsx` | modify | All-hosts default; new hook; strip, footer and honest empty states |
| `lib/usePagedList.ts` | modify | Remove `poll` and the buggy splice |
| `pages/Usage.tsx` | modify | Use `useUsageData` |
| `components/usage/UsageTimeline.tsx`, `components/dashboard/UsageTimelineStacked.tsx`, `components/dashboard/ModelBreakdownTable.tsx` | modify | `hosts` prop typed as name refs |
| `components/CommandPalette.tsx` | modify | Per-host runs source |
| `lib/paletteSources.ts` | modify | `runItems` adds `?host=` for remote runs (bug fix) |

**Docs**
- `CLAUDE.md`: a Read-first entry plus a `rupu-cp` crate note.

---

### Task 0: Baseline

**Files:** none.

- [ ] **Step 1: Confirm the branch and a clean tree**

Run: `git status -sb && git log --oneline -2`
Expected: `## claude/cp-progressive-per-host-loading`, a clean tree, and the top commit is `docs: CP progressive per-host loading design …`.

- [ ] **Step 2: Measure the Rust baseline**

Run: `cargo test -p rupu-cp --lib 2>&1 | tail -5`
Expected: record the pass/fail counts. Any pre-existing failure is noted here and is not yours to fix in this plan unless a later task touches it.

- [ ] **Step 3: Measure the web baseline**

Run: `cd crates/rupu-cp/web && npx vitest run 2>&1 | tail -8 && npx tsc -b && echo TSC_OK`
Expected: record the counts and whether `TSC_OK` printed.

---

### Task 1: Honest error statuses on single-host list paths

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs`
  - add `host_list_error` after `resolve_host` (~line 418)
  - wire it into `list_runs` (~line 699) and `list_workflow_runs` (~line 836)
  - edit `FakeHostConnector::list_runs` (~line 3013)
- Modify: `crates/rupu-cp/src/api/run_streams.rs`: lines ~857, ~1021, ~1226
- Modify: `crates/rupu-cp/src/api/sessions.rs`: line ~428
- Test: the existing `#[cfg(test)]` modules of those three files

**Interfaces:**
- Produces: `pub(crate) fn host_list_error(e: HostConnectorError) -> ApiError` in `crate::api::runs`, and `pub(crate) fn state_with_fake_host(tmp: &tempfile::TempDir, run_json: serde_json::Value) -> AppState` in `crate::api::runs::tests`.

- [ ] **Step 1: Write the failing mapping test.** Add it to `runs.rs` `mod tests`, right after the `FakeHostConnector` impl block:

```rust
    #[test]
    fn host_list_error_maps_connector_failures_to_honest_statuses() {
        use axum::http::StatusCode;
        let cases = [
            (HostConnectorError::Unsupported("x".into()), StatusCode::NOT_IMPLEMENTED),
            (HostConnectorError::Invalid("x".into()), StatusCode::NOT_IMPLEMENTED),
            (HostConnectorError::NotFound("x".into()), StatusCode::NOT_FOUND),
            (HostConnectorError::Unreachable("x".into()), StatusCode::BAD_GATEWAY),
            (HostConnectorError::Unauthorized, StatusCode::BAD_GATEWAY),
            (HostConnectorError::Remote(500, "x".into()), StatusCode::BAD_GATEWAY),
            (HostConnectorError::NotJson("x".into()), StatusCode::BAD_GATEWAY),
        ];
        for (err, want) in cases {
            let label = err.to_string();
            assert_eq!(host_list_error(err).0, want, "{label}");
        }
    }

    /// An `AppState` whose registry resolves `host_fake` to a
    /// [`FakeHostConnector`] scripted with `run_json` (same seam as
    /// `get_run_proxies_to_host_when_resolver_says_host`: a `Local`-transport
    /// entry under a distinct id resolves to the injected connector).
    pub(crate) fn state_with_fake_host(
        tmp: &tempfile::TempDir,
        run_json: serde_json::Value,
    ) -> AppState {
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: "host_fake".into(),
                name: "fake".into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        let fake: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(FakeHostConnector { run_json });
        test_state(tmp).with_hosts(Arc::new(crate::host::registry::HostRegistry::new(
            host_store, fake,
        )))
    }

    #[tokio::test]
    async fn single_remote_host_run_lists_report_an_unreachable_host_as_502() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No "runs" scripted → the fake's list_runs answers Unreachable.
        let s = state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_runs(
            State(s.clone()),
            Query(RunsListQuery { offset: None, limit: None, host: Some("host_fake".into()) }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
        let err = list_workflow_runs(
            State(s),
            Query(WorkflowRunsQuery {
                offset: None,
                limit: None,
                lifecycle: None,
                host: Some("host_fake".into()),
                since: None,
                until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    }
```

- [ ] **Step 2: Make `FakeHostConnector::list_runs` scriptable.** Its body is currently `unimplemented!`; no existing test calls it. Replace it with:

```rust
        /// Rows come from `run_json["runs"]` when present; otherwise the
        /// host answers as unreachable (exercises the 502 mapping).
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self.run_json.get("runs").and_then(|v| v.as_array()) {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unreachable("fake host: no runs scripted".into())),
            }
        }
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -p rupu-cp --lib api::runs::tests::host_list_error api::runs::tests::single_remote_host_run_lists 2>&1 | tail -15`
Expected: a compile error, `cannot find function host_list_error`.

- [ ] **Step 4: Add `host_list_error` in `runs.rs`, directly after `resolve_host`:**

```rust
/// Map a connector failure on a single-host LIST path (`?host=<remote-id>`)
/// to the status the web's per-host loader reads (spec
/// `2026-10-01-rupu-cp-progressive-per-host-loading-design.md` §7.1):
///
/// - `Unsupported` / `Invalid` → 501: the host cannot serve this listing (an
///   old remote rupu, a transport with no such surface). The web shows the
///   host as *unavailable*, with this reason.
/// - `NotFound` → 404.
/// - everything else → 502: the host gave no usable answer. The web shows it
///   as *offline*, with this reason.
///
/// Was a bare 500 for all of them, which cannot tell "down" from "too old".
pub(crate) fn host_list_error(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::Unsupported(_) | HostConnectorError::Invalid(_) => {
            ApiError::not_available(e.to_string())
        }
        HostConnectorError::NotFound(_) => ApiError::not_found(e.to_string()),
        other => ApiError::bad_gateway(other.to_string()),
    }
}
```

- [ ] **Step 5: Wire it into the six single-host list paths.** At each site below, replace `.map_err(|e| ApiError::internal(e.to_string()))` (or `.map_err(|e| crate::error::ApiError::internal(e.to_string()))`) on the `conn.list_*(…)` call with the mapper shown:

| File | Handler | Call | Replacement |
|---|---|---|---|
| `runs.rs` | `list_runs`, `if let Some(host_id) = &q.host` branch | `conn.list_runs(params).await` | `.map_err(host_list_error)?` |
| `runs.rs` | `list_workflow_runs`, remote branch after the `host_id == "local"` block | `conn.list_runs(params).await` | `.map_err(host_list_error)?` |
| `run_streams.rs` | `list_agent_runs` | `conn.list_agent_runs()` | `.map_err(crate::api::runs::host_list_error)?` |
| `run_streams.rs` | `list_autoflow_runs` | `conn.list_autoflow_runs()` | `.map_err(crate::api::runs::host_list_error)?` |
| `run_streams.rs` | `list_autoflow_events` | `conn.list_autoflow_events()` | `.map_err(crate::api::runs::host_list_error)?` |
| `sessions.rs` | `list_sessions` | `conn.list_sessions(q.scope.as_deref())` | `.map_err(crate::api::runs::host_list_error)?` |

Do not touch the other `ApiError::internal` sites in these files (local-store errors, serialization).

- [ ] **Step 6: Add the 501 handler tests.** These connectors' trait-default list methods answer `Unsupported`.

In `run_streams.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn single_remote_host_autoflow_and_agent_lists_report_unsupported_as_501() {
        let tmp = tempfile::TempDir::new().unwrap();
        // FakeHostConnector has no autoflow overrides (trait default:
        // Unsupported) and no "agent_runs" key (its own Unsupported branch).
        let s = crate::api::runs::tests::state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_autoflow_runs(
            State(s.clone()),
            Query(AutoflowRunsQuery {
                offset: None, limit: None, host: Some("host_fake".into()), since: None, until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
        let err = list_autoflow_events(
            State(s.clone()),
            Query(AutoflowEventsQuery {
                offset: None, limit: None, host: Some("host_fake".into()), since: None, until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
        let err = list_agent_runs(
            State(s),
            Query(AgentRunsQuery {
                offset: None, limit: None, lifecycle: None, host: Some("host_fake".into()),
                since: None, until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
```

In `sessions.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn single_remote_host_session_list_reports_unsupported_as_501() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = crate::api::runs::tests::state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_sessions(
            State(s),
            Query(SessionsQuery {
                offset: None, limit: None, scope: None, host: Some("host_fake".into()),
                since: None, until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
```

If `State`/`Query` are not already imported in a test module, add `use axum::extract::{Query, State};` to it.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p rupu-cp --lib host_list_error single_remote_host 2>&1 | tail -15`
Expected: 4 tests pass.

- [ ] **Step 8: Run the wider rupu-cp lib suite, format and lint**

Run:

```
cargo test -p rupu-cp --lib 2>&1 | tail -5
rustfmt --edition 2021 crates/rupu-cp/src/api/runs.rs
cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "^(warning|error)" | head
```

Expected: the same pass count as the baseline plus 4, and no clippy output for the touched files. Do not rustfmt `run_streams.rs`/`sessions.rs`; check your edits there by eye.

- [ ] **Step 9: Commit**

```bash
git add crates/rupu-cp/src/api/runs.rs crates/rupu-cp/src/api/run_streams.rs crates/rupu-cp/src/api/sessions.rs
git commit -m "$(cat <<'EOF'
fix(cp): 502/501 instead of 500 on single-host list paths

A remote host's listing failure was a bare 500, so the web could not tell
a down host (offline) from an old one (unavailable). Unsupported/Invalid
now answer 501, NotFound 404, everything else 502, on runs, workflow
runs, agent runs, autoflow cycles/events and sessions.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `ListingCache`

**Files:**
- Create: `crates/rupu-cp/src/host/listing_cache.rs`
- Modify: `crates/rupu-cp/src/host/mod.rs`: add `pub mod listing_cache;` after `pub mod lazy_tail;`. Hand-edit only.
- Modify: `crates/rupu-cp/src/host/connector.rs`: `#[derive(Debug, thiserror::Error)]` on `HostConnectorError` becomes `#[derive(Debug, Clone, thiserror::Error)]`.
- Test: inline `mod tests` in `listing_cache.rs`

**Interfaces:**
- Produces:
  - `pub struct ListingCache`, with `ListingCache::new(ttl: Duration)` and `Default` (TTL = `LISTING_TTL`)
  - `pub async fn get<F, Fut>(&self, key: &str, fetch: F) -> Result<Rows, HostConnectorError>`, where `F: FnOnce() -> Fut + Send` and `Fut: Future<Output = Result<Vec<serde_json::Value>, HostConnectorError>> + Send + 'static`
  - `pub fn clear(&self)` and `pub fn clear_on_drop(&self) -> ClearOnDrop<'_>`
  - `pub type Rows = Arc<Vec<serde_json::Value>>` and `pub const LISTING_TTL: Duration`

- [ ] **Step 1: Write the module with its tests.** Implementation and tests go in one file. Write the tests first, then run them against a `todo!()` body for `get` (Step 2), then fill the body in (Step 3).

```rust
//! Short-lived, single-flight cache of one SSH host's LISTING commands (spec
//! `docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md`
//! §7.2).
//!
//! SSH listings are all-or-nothing (`rupu run list --limit 10000`,
//! `rupu autoflow history`), and every `RemoteExec::run` is a fresh `ssh`
//! with a full handshake. The web loads each host independently and pages it
//! with offset cursors, and the dashboard, the Activity tables and ⌘K can
//! all ask one host at once. Without this cache, each of those requests
//! re-ran the whole remote listing.
//!
//! - An `Ok` listing younger than `ttl` is served as is. An older one waits
//!   for a fresh fetch. Stale data is never served.
//! - Concurrent callers for one key share ONE in-flight fetch, including its
//!   error. An error is never stored past that fetch.
//! - [`ListingCache::clear`], which every mutating connector method calls via
//!   [`ClearOnDrop`], drops everything. A fetch that STARTED before the clear
//!   never stores its (pre-mutation) answer.

use crate::host::connector::HostConnectorError;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a listing is served from the cache.
///
/// List responses carry no capture time, so the web strip measures a host's
/// age from when the browser received the answer. That makes this TTL the
/// most a cached answer can be older than it looks. It matches the strip's
/// `LIVE_THRESHOLD_MS` (5 s, `web/src/components/dashboard/HostFreshnessStrip.tsx`).
/// Raise it only together with a server-stamped capture time (spec §11).
pub const LISTING_TTL: Duration = Duration::from_secs(5);

/// One listing's rows, shared between every caller that received it.
pub type Rows = Arc<Vec<serde_json::Value>>;

type Fetch = Shared<BoxFuture<'static, Result<Rows, HostConnectorError>>>;

#[derive(Default)]
struct State {
    /// Bumped by `clear`. A fetch started under an older epoch never stores.
    epoch: u64,
    /// Identity of each in-flight fetch, so only ITS first waiter retires it.
    next_id: u64,
    fresh: HashMap<String, (Instant, Rows)>,
    inflight: HashMap<String, (u64, u64, Fetch)>,
}

/// See the module docs.
pub struct ListingCache {
    ttl: Duration,
    state: Mutex<State>,
}

impl Default for ListingCache {
    fn default() -> Self {
        Self::new(LISTING_TTL)
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl ListingCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            state: Mutex::default(),
        }
    }

    /// Drop every listing, and make every fetch already in flight unable to
    /// store its answer.
    pub fn clear(&self) {
        let mut s = lock(&self.state);
        s.epoch += 1;
        s.fresh.clear();
        s.inflight.clear();
    }

    /// Clear when the returned guard drops, i.e. when the mutating call
    /// holding it returns, success or failure (a failed mutation may still
    /// have partly applied on the remote).
    pub fn clear_on_drop(&self) -> ClearOnDrop<'_> {
        ClearOnDrop(self)
    }

    /// `key`'s rows: fresh from the cache, joined onto an in-flight fetch, or
    /// fetched now via `fetch`. `fetch` is only called when neither exists.
    pub async fn get<F, Fut>(&self, key: &str, fetch: F) -> Result<Rows, HostConnectorError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Vec<serde_json::Value>, HostConnectorError>> + Send + 'static,
    {
        let (epoch, id, fut) = {
            let mut s = lock(&self.state);
            if let Some((at, rows)) = s.fresh.get(key) {
                if at.elapsed() < self.ttl {
                    return Ok(Arc::clone(rows));
                }
            }
            if let Some(joined) = s.inflight.get(key).cloned() {
                joined
            } else {
                s.next_id += 1;
                let (epoch, id) = (s.epoch, s.next_id);
                let fut: Fetch = fetch().map(|r| r.map(Arc::new)).boxed().shared();
                s.inflight
                    .insert(key.to_string(), (epoch, id, fut.clone()));
                (epoch, id, fut)
            }
        };
        let out = fut.await;
        let mut s = lock(&self.state);
        // The first waiter back retires the entry. On success, and only if no
        // `clear` happened since the fetch started, it stores the rows.
        if matches!(s.inflight.get(key), Some((_, cur, _)) if *cur == id) {
            s.inflight.remove(key);
            if let Ok(rows) = &out {
                if s.epoch == epoch {
                    s.fresh
                        .insert(key.to_string(), (Instant::now(), Arc::clone(rows)));
                }
            }
        }
        out
    }
}

/// Clears its [`ListingCache`] when dropped. See [`ListingCache::clear_on_drop`].
pub struct ClearOnDrop<'a>(&'a ListingCache);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    type Out = Result<Vec<serde_json::Value>, HostConnectorError>;

    fn rows(n: u64) -> Vec<serde_json::Value> {
        (0..n).map(|i| serde_json::json!({ "id": i })).collect()
    }

    /// A fetch that counts its calls, sleeps `delay_ms`, then returns `out`.
    fn counted(
        calls: &Arc<AtomicU32>,
        out: Out,
        delay_ms: u64,
    ) -> impl FnOnce() -> BoxFuture<'static, Out> + Send {
        let calls = Arc::clone(calls);
        move || {
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                out
            }
            .boxed()
        }
    }

    fn down() -> HostConnectorError {
        HostConnectorError::Unreachable("connection timed out".into())
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_fetch() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (a, b) = tokio::join!(
            cache.get("k", counted(&calls, Ok(rows(2)), 30)),
            cache.get("k", counted(&calls, Ok(rows(9)), 30)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(a.unwrap().len(), 2);
        assert_eq!(b.unwrap().len(), 2, "the second caller joined the first fetch");
    }

    #[tokio::test]
    async fn concurrent_callers_share_an_error() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (a, b) = tokio::join!(
            cache.get("k", counted(&calls, Err(down()), 30)),
            cache.get("k", counted(&calls, Err(down()), 30)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(a.is_err() && b.is_err());
    }

    #[tokio::test]
    async fn a_fresh_listing_is_served_without_refetching() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_expired_listing_is_refetched() {
        let cache = ListingCache::new(Duration::from_millis(20));
        let calls = Arc::new(AtomicU32::new(0));
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_error_is_not_stored() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        assert!(cache.get("k", counted(&calls, Err(down()), 0)).await.is_err());
        let ok = cache.get("k", counted(&calls, Ok(rows(3)), 0)).await.unwrap();
        assert_eq!(ok.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn clear_drops_a_fresh_listing() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        cache.clear();
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_fetch_started_before_clear_never_stores() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        let (first, ()) = tokio::join!(
            cache.get("k", counted(&calls, Ok(rows(1)), 50)),
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                cache.clear();
            },
        );
        first.unwrap();
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the pre-clear answer must not have been stored"
        );
    }

    #[tokio::test]
    async fn clear_on_drop_clears() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        {
            let _guard = cache.clear_on_drop();
        }
        cache.get("k", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn keys_are_independent() {
        let cache = ListingCache::default();
        let calls = Arc::new(AtomicU32::new(0));
        cache.get("a", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        cache.get("b", counted(&calls, Ok(rows(1)), 0)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
```

- [ ] **Step 2: Watch the tests fail before the body exists.** Temporarily replace the body of `get` with `todo!()`, then run:

Run: `cargo test -p rupu-cp --lib host::listing_cache 2>&1 | tail -15`
Expected: every test panics with `not yet implemented`.

- [ ] **Step 3: Restore the real `get` body** from Step 1.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cp --lib host::listing_cache 2>&1 | tail -15`
Expected: 9 passed.

- [ ] **Step 5: Format and lint**

Run: `rustfmt --edition 2021 crates/rupu-cp/src/host/listing_cache.rs && cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "^(warning|error)" | head`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/host/listing_cache.rs crates/rupu-cp/src/host/mod.rs crates/rupu-cp/src/host/connector.rs
git commit -m "$(cat <<'EOF'
feat(cp): ListingCache — 5s single-flight cache for SSH listings

Concurrent callers share one in-flight fetch (errors included), errors
are never stored, and clear() makes a fetch already in flight unable to
store its pre-mutation answer. HostConnectorError derives Clone so a
Shared future can hand it to every waiter.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Route SSH listings through the cache; clear it on every mutation

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`
- Test: the `mod tests` in `ssh.rs` (starts ~line 2976)

**Interfaces:**
- Consumes: `crate::host::listing_cache::{ListingCache, Rows}` (Task 2).
- Produces:
  - `SshHostConnector.listings: ListingCache` (private field)
  - `async fn cached_rows(&self, argv: &[&str]) -> Result<Vec<serde_json::Value>, HostConnectorError>`
  - free `async fn exec_rupu_json(exec: Arc<dyn RemoteExec>, argv: Vec<String>) -> Result<serde_json::Value, HostConnectorError>`

- [ ] **Step 1: Write the failing tests.** Add them to `ssh.rs` `mod tests`, after `make_conn`:

```rust
    // ── Listing cache (spec 2026-10-01 §7.2) ─────────────────────────────────

    /// Answers every listing with one run row (or fails), recording each
    /// remote command so tests can count how many actually ran.
    struct ListingExec {
        commands: std::sync::Mutex<Vec<String>>,
        fail: std::sync::atomic::AtomicBool,
        delay: std::time::Duration,
    }

    impl ListingExec {
        fn new(delay_ms: u64) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                commands: Default::default(),
                fail: Default::default(),
                delay: std::time::Duration::from_millis(delay_ms),
            })
        }
        fn count(&self, needle: &str) -> usize {
            self.commands
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.contains(needle))
                .count()
        }
    }

    #[async_trait::async_trait]
    impl RemoteExec for ListingExec {
        async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
            self.commands.lock().unwrap().push(remote.to_string());
            tokio::time::sleep(self.delay).await;
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: "ssh: connect to host host-a port 22: Operation timed out".into(),
                    success: false,
                });
            }
            Ok(RemoteOutput {
                stdout: r#"{"rows":[{"id":"run_1","workflow_name":"wf","status":"completed","started_at":"2026-09-01T00:00:00Z","trigger":"manual"}]}"#.into(),
                stderr: String::new(),
                success: true,
            })
        }
        fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
            unimplemented!("not used by the listing-cache tests")
        }
        async fn run_bytes(
            &self,
            _c: &str,
            _s: Option<Vec<u8>>,
        ) -> Result<Vec<u8>, RemoteExecError> {
            unimplemented!("not used by the listing-cache tests")
        }
    }

    fn all_runs() -> RunListQuery {
        RunListQuery {
            kind: RunKind::All,
            offset: 0,
            limit: 20,
            lifecycle: None,
        }
    }

    const RUN_LIST: &str = "'run' 'list'";
    const AUTOFLOW_HISTORY: &str = "'autoflow' 'history'";
    const SESSION_LIST: &str = "'session' 'list'";

    #[tokio::test]
    async fn concurrent_list_runs_share_one_remote_listing() {
        let exec = ListingExec::new(30);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        let (a, b) = tokio::join!(conn.list_runs(all_runs()), conn.list_runs(all_runs()));
        a.unwrap();
        b.unwrap();
        assert_eq!(exec.count(RUN_LIST), 1);
    }

    #[tokio::test]
    async fn list_runs_and_dashboard_summary_share_the_run_list() {
        let exec = ListingExec::new(0);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        conn.list_runs(all_runs()).await.unwrap();
        conn.dashboard_summary(crate::host::dashboard_summary::DashboardRange::Days30)
            .await
            .unwrap();
        assert_eq!(exec.count(RUN_LIST), 1);
    }

    #[tokio::test]
    async fn autoflow_cycles_and_events_share_one_history_listing() {
        let exec = ListingExec::new(0);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        conn.list_autoflow_runs().await.unwrap();
        conn.list_autoflow_events().await.unwrap();
        assert_eq!(exec.count(AUTOFLOW_HISTORY), 1);
    }

    #[tokio::test]
    async fn a_failed_listing_is_not_cached() {
        let exec = ListingExec::new(0);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        exec.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(conn.list_runs(all_runs()).await.is_err());
        exec.fail.store(false, std::sync::atomic::Ordering::SeqCst);
        conn.list_runs(all_runs()).await.unwrap();
        assert_eq!(exec.count(RUN_LIST), 2);
    }

    #[tokio::test]
    async fn session_scopes_are_cached_separately() {
        let exec = ListingExec::new(0);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        conn.list_sessions(Some("active")).await.unwrap();
        conn.list_sessions(Some("active")).await.unwrap();
        conn.list_sessions(Some("archived")).await.unwrap();
        assert_eq!(exec.count(SESSION_LIST), 2);
    }

    #[tokio::test]
    async fn every_mutation_clears_the_listing_cache() {
        let exec = ListingExec::new(0);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        let mut expected = 1;
        conn.list_runs(all_runs()).await.unwrap();
        assert_eq!(exec.count(RUN_LIST), expected);

        // Each mutation, followed by a listing that must hit the remote again.
        macro_rules! after {
            ($label:literal, $call:expr) => {{
                let _ = $call.await;
                conn.list_runs(all_runs()).await.unwrap();
                expected += 1;
                assert_eq!(exec.count(RUN_LIST), expected, "{} must clear the cache", $label);
            }};
        }
        after!("approve_run", conn.approve_run("run_1", ""));
        after!("reject_run", conn.reject_run("run_1", None));
        after!("cancel_run", conn.cancel_run("run_1"));
        after!("pause_run", conn.pause_run("run_1"));
        after!("resume_run", conn.resume_run("run_1"));
        after!("archive_run", conn.archive_run("run_1"));
        after!("restore_run", conn.restore_run("run_1"));
        after!("delete_run", conn.delete_run("run_1"));
        after!("archive_session", conn.archive_session("ses_1"));
        after!("restore_session", conn.restore_session("ses_1"));
        after!("delete_session", conn.delete_session("ses_1"));
    }

    /// `list_runs` / `dashboard_summary` used to report EVERY `run list`
    /// failure as `Unsupported` ("host may predate the command"). That
    /// included a host that is simply down, which then rendered as
    /// "unavailable" (501 → retried only on manual Refresh) instead of
    /// offline. Only a failure in the remote rupu itself means "too old".
    #[tokio::test]
    async fn list_runs_reports_a_down_host_as_unreachable_not_unsupported() {
        let exec = ListingExec::new(0);
        exec.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&exec));
        let err = conn.list_runs(all_runs()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)), "{err}");
        let err = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::Days30)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)), "{err}");
    }

    #[tokio::test]
    async fn list_runs_still_reports_an_old_remote_rupu_as_unsupported() {
        let fake = std::sync::Arc::new(FakeExec::offline("error: agent 'list' not found"));
        let (conn, _store, _tmp) = make_conn(fake);
        let err = conn.list_runs(all_runs()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err}");
    }

    #[test]
    fn ssh_transport_failures_are_told_apart_from_remote_cli_failures() {
        for stderr in [
            "ssh: connect to host host-a port 22: Connection refused",
            "ssh: Could not resolve hostname host-a: nodename nor servname provided, or not known",
            "user@host-a: Permission denied (publickey).",
            "kex_exchange_identification: read: Connection reset by peer",
            "Connection closed by 10.0.0.9 port 22",
            "Host key verification failed.",
            "host unreachable: ssh spawn failed: No such file or directory",
        ] {
            assert!(is_ssh_transport_failure(stderr), "{stderr}");
        }
        for stderr in [
            "error: agent 'list' not found",
            "unrecognized subcommand 'show'",
            "run does not support `--format json` (supported: `table`)",
        ] {
            assert!(!is_ssh_transport_failure(stderr), "{stderr}");
        }
    }
```

`launch_run`, `launch_agent`, `start_session` and `send_session_turn` also take the guard (Step 4). They aren't exercised here because their request fixtures pull in workspace staging. Those four one-line guards are checked by inspection in review.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -p rupu-cp --lib host::ssh::tests::concurrent_list_runs host::ssh::tests::list_runs_and_dashboard host::ssh::tests::autoflow_cycles_and_events host::ssh::tests::a_failed_listing host::ssh::tests::session_scopes host::ssh::tests::every_mutation 2>&1 | tail -20`
Expected: FAIL. Counts are 2 where 1 is expected; nothing is cached yet.

- [ ] **Step 3: Add the field and the cached path.**

In `struct SshHostConnector`, after `lazy`:

```rust
    /// Short-lived single-flight cache of listing commands (`run list`,
    /// `autoflow history`, `transcript list`, `session list`) — see
    /// [`crate::host::listing_cache`]. Cleared by every mutating method.
    listings: crate::host::listing_cache::ListingCache,
```

In `SshHostConnector::new`, add `listings: Default::default(),` to the `Self { … }` literal.

Replace the body of `remote_json` and add the free function and `cached_rows`. `remote_json_rows` stays as is (used by `session usage-timeline`).

```rust
/// Run `rupu <argv…>` over `exec` and parse its JSON stdout. A free function
/// (owned `exec` and argv) so the listing cache can hold the future as
/// `'static`. [`SshHostConnector::remote_json`] delegates here.
async fn exec_rupu_json(
    exec: Arc<dyn RemoteExec>,
    argv: Vec<String>,
) -> Result<serde_json::Value, HostConnectorError> {
    let owned: Vec<String> = std::iter::once("rupu".to_string())
        .chain(argv.iter().cloned())
        .collect();
    let cmd = build_remote_command(&owned);
    let out = exec
        .run(&cmd)
        .await
        .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
    if !out.success {
        return Err(HostConnectorError::Unreachable(out.stderr));
    }
    serde_json::from_str(out.stdout.trim()).map_err(|e| {
        HostConnectorError::Remote(0, format!("parse `rupu {}` output: {e}", argv.join(" ")))
    })
}

/// The `rows` array of a CLI `--format json` report (empty when absent).
fn json_rows(parsed: &serde_json::Value) -> Vec<serde_json::Value> {
    parsed
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
}
```

Place both free functions immediately above `impl SshHostConnector {` (the inherent impl that holds `remote_json`). Then, inside that impl:

```rust
    async fn remote_json(&self, argv: &[&str]) -> Result<serde_json::Value, HostConnectorError> {
        exec_rupu_json(
            Arc::clone(&self.exec),
            argv.iter().map(|s| s.to_string()).collect(),
        )
        .await
    }

    /// [`remote_json_rows`](Self::remote_json_rows) through [`Self::listings`]:
    /// one remote run per argv per [`crate::host::listing_cache::LISTING_TTL`],
    /// shared by every concurrent caller.
    async fn cached_rows(&self, argv: &[&str]) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let key = argv.join("\u{1f}");
        let exec = Arc::clone(&self.exec);
        let rows = self
            .listings
            .get(&key, move || async move {
                exec_rupu_json(exec, argv).await.map(|v| json_rows(&v))
            })
            .await?;
        Ok(rows.as_ref().clone())
    }
```

Change `remote_json_rows` to use `json_rows`:

```rust
    async fn remote_json_rows(
        &self,
        argv: &[&str],
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        Ok(json_rows(&self.remote_json(argv).await?))
    }
```

- [ ] **Step 4: Switch the listing methods to `cached_rows`, and guard every mutation.**
  - **Listings.** Replace `self.remote_json_rows(` with `self.cached_rows(` in:
    - `list_runs`: the `run list --limit 10000` call
    - `dashboard_summary`: the `run list --limit 10000` call
    - `list_agent_runs`: `transcript list --format json`
    - `list_autoflow_runs` and `list_autoflow_events`: `autoflow history --format json`
  - **`list_sessions`.** Replace its whole body with:

```rust
        // The CLI lists active sessions by default; `--archived` restricts
        // to the archived scope. "active"/None → default (no flag).
        let mut argv = vec!["session", "list", "--format", "json"];
        if let Some("archived") = scope {
            argv.push("--archived");
        }
        self.cached_rows(&argv).await
```

  - **Mutations.** Add `let _clear = self.listings.clear_on_drop();` as the first statement of each of these methods: `remote_workflow`, `remote_session`, `resume_run`, `launch_run`, `launch_agent`, `start_session`, `send_session_turn`.
    - `remote_workflow` covers approve, reject, cancel, pause, archive, restore and delete run.
    - `remote_session` covers archive, restore and delete session.
    - Add a one-line comment on `remote_workflow`'s guard: `// Every caller mutates the remote: drop listings on return (spec §7.2).`

- [ ] **Step 4b: Tell a down host from an old one.** This is a bug found while planning; without it, the 501/502 mapping from Task 1 would label every down SSH host *unavailable*, on the dashboard as well as the lists.

Add this free function next to `classify_remote_cli_failure`:

```rust
/// Whether a failed remote command failed in the ssh TRANSPORT (connect,
/// resolve, auth, host key, or the ssh binary itself) rather than in the
/// remote `rupu`. ssh reports these itself on stderr (exit 255) before the
/// remote command ever runs; `RemoteOutput` carries no exit code, so the
/// markers are what tells "this host is down" from "this host's rupu is too
/// old to answer".
fn is_ssh_transport_failure(stderr: &str) -> bool {
    const MARKERS: &[&str] = &[
        "ssh: ", // "ssh: connect to host …", "ssh: Could not resolve hostname …"
        "ssh spawn failed",
        "connection closed by",
        "connection reset by",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "no route to host",
        "kex_exchange_identification",
        "permission denied (publickey",
        "host key verification failed",
    ];
    let lower = stderr.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}
```

In `list_runs`, change the `Err(e) => { … return Err(HostConnectorError::Unsupported(…)) }` arm so a transport failure passes through untouched. Insert at the top of that arm:

```rust
            Err(e) => {
                // A host that is DOWN is not a host that predates `run list`.
                if let HostConnectorError::Unreachable(msg) = &e {
                    if is_ssh_transport_failure(msg) {
                        return Err(e);
                    }
                }
                // … existing warn + Unsupported mapping, unchanged …
```

In `dashboard_summary`, apply the same rule to the `run list` call's `.map_err(…)`. Replace it with:

```rust
            .map_err(|e| {
                if let HostConnectorError::Unreachable(msg) = &e {
                    if is_ssh_transport_failure(msg) {
                        return e;
                    }
                }
                tracing::warn!(host_id = %self.host_id, error = %e, "dashboard_summary: run list failed");
                HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu run list`: {e}",
                    self.host_id
                ))
            })?;
```

No dashboard-handler change is needed. `api/dashboard.rs` already renders `Unsupported` as `unavailable` ("needs a newer rupu") and every other error as `offline` with the reason. With this step, a down SSH host now reaches that `offline` arm instead of being mislabelled "needs a newer rupu".

- [ ] **Step 5: Run the new tests and the full ssh suite**

Run: `cargo test -p rupu-cp --lib host::ssh 2>&1 | tail -8`
Expected: all pass, including the 9 new tests.

If an existing test asserts an exact command sequence that now has fewer `run list` calls, it was counting the redundant fetches this task removes. Update its expectation and note why in the test. If an existing test asserts the old `parse \`rupu session list\` output` message, update it to the new `parse \`rupu session list --format json\` output` text.

- [ ] **Step 6: Full lib suite, format, lint**

Run:

```
cargo test -p rupu-cp --lib 2>&1 | tail -5
rustfmt --edition 2021 crates/rupu-cp/src/host/ssh.rs
cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "^(warning|error)" | head
```

Expected: the baseline count plus all new tests, and no clippy output.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "$(cat <<'EOF'
perf(cp): serve SSH listings through a 5s single-flight cache

run list / autoflow history / transcript list / session list are
all-or-nothing remote commands, each a fresh ssh handshake. Paging an SSH
host, or the dashboard and Activity asking at once, re-ran them every
time. Listings now go through ListingCache; every mutating method
(approve/reject/cancel/pause/resume/archive/restore/delete, launches,
session send/archive/restore/delete) clears it on return.

Also: a down SSH host's `run list` failure was reported as Unsupported
("predates the command"), so it rendered as unavailable instead of
offline. ssh transport failures now pass through as Unreachable.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Per-host core (pure): types, watermark merge, splice

**Files:**
- Create: `crates/rupu-cp/web/src/lib/perHost/types.ts`
- Create: `crates/rupu-cp/web/src/lib/perHost/watermarkMerge.ts`
- Create: `crates/rupu-cp/web/src/lib/perHost/spliceHead.ts`
- Test: `crates/rupu-cp/web/src/lib/perHost/watermarkMerge.test.ts`, `crates/rupu-cp/web/src/lib/perHost/spliceHead.test.ts`

**Interfaces:**
- Produces, from `types.ts`:
  - `type HostState = 'loading' | 'ok' | 'offline' | 'unavailable'`
  - `interface HostTagged { host_id?: string }`
  - `interface HostSlice<T>` (spec §6.1 fields)
  - `interface RowAccess<T> { timeOf(row): number; keyOf(row, sliceHostId): string; idOf(row): string }`
  - `instantOf(v: unknown): number`
  - `rowAccess<T extends HostTagged>(timeField, idField): RowAccess<T>`
  - `interface HostSeed { id: string; name: string; transport_kind: string }`
  - `emptySlice<T>(h: HostSeed): HostSlice<T>`
- Produces, from `watermarkMerge.ts`:
  - `coveredOf`, `isGating`, `floorOf`
  - `compareNewestFirst(a: {t, key}, b: {t, key}): number`
  - `interface MergeResult<T> { visible: T[]; floor: number; hasMore: boolean; ended: boolean; gatingHosts: string[] }`
  - `watermarkMerge<T>(slices, access): MergeResult<T>`
  - `displacedCount<T>(visible, covered, timeOf): number`
- Produces, from `spliceHead.ts`:
  - `withHost<T extends HostTagged>(rows, hostId): T[]`
  - `sortSlice<T>(rows, access, hostId): T[]`
  - `appendPage<T>(old, page, access, hostId): T[]`
  - `interface SpliceResult<T> { rows: T[]; fullyListed: boolean }`
  - `spliceHead<T>(old, fresh, limit, access, hostId): SpliceResult<T>`

- [ ] **Step 1: Write `types.ts`**

```ts
// Per-host list loading — the slice model (spec
// docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md §6.1).
//
// One slice per host. A list page fires one request per host and merges the
// slices client-side (watermarkMerge.ts), so a slow or dead host never holds
// back anyone else's rows.

export type HostState = 'loading' | 'ok' | 'offline' | 'unavailable';

/** Every row this machinery merges may carry a server-tagged `host_id`. */
export interface HostTagged {
  host_id?: string;
}

export interface HostSlice<T> {
  hostId: string;
  name: string;
  transportKind: string;
  state: HostState;
  /** Newest-first by `timeOf`, de-duplicated by `keyOf`. */
  rows: T[];
  hasMore: boolean;
  /** A late host fetching up to the floor before it gates (spec §6.3). */
  catchingUp: boolean;
  /** A next-page or catch-up request for this host failed. */
  pagingFailed: boolean;
  /** Why the host is not `ok`, or why its last refresh failed (stale). */
  reason: string | null;
  /** Epoch ms of this host's last successful answer. */
  receivedAt: number | null;
}

export interface RowAccess<T> {
  /** Sort instant (epoch ms); `-Infinity` for a missing/unparseable stamp. */
  timeOf: (row: T) => number;
  /** Merge identity: the row's own `host_id` (server-tagged), else the slice's. */
  keyOf: (row: T, sliceHostId: string) => string;
  idOf: (row: T) => string;
}

/** Parse an RFC-3339 stamp to epoch ms. A missing or unparseable one sorts oldest. */
export function instantOf(v: unknown): number {
  if (typeof v !== 'string' || v === '') return -Infinity;
  const t = Date.parse(v);
  return Number.isNaN(t) ? -Infinity : t;
}

export function rowAccess<T extends HostTagged>(
  timeField: keyof T & string,
  idField: keyof T & string,
): RowAccess<T> {
  return {
    timeOf: (row) => instantOf(row[timeField]),
    keyOf: (row, sliceHostId) => `${row.host_id ?? sliceHostId}\u0000${String(row[idField])}`,
    idOf: (row) => String(row[idField]),
  };
}

/** The minimum a slice needs to know about its host. `RegisteredHostView` fits. */
export interface HostSeed {
  id: string;
  name: string;
  transport_kind: string;
}

export function emptySlice<T>(h: HostSeed): HostSlice<T> {
  return {
    hostId: h.id,
    name: h.name,
    transportKind: h.transport_kind,
    state: 'loading',
    rows: [],
    hasMore: true,
    catchingUp: false,
    pagingFailed: false,
    reason: null,
    receivedAt: null,
  };
}
```

- [ ] **Step 2: Write the failing tests `watermarkMerge.test.ts`**

```ts
import { describe, expect, it } from 'vitest';
import { emptySlice, rowAccess, type HostSlice } from './types';
import { displacedCount, floorOf, watermarkMerge } from './watermarkMerge';

interface Row { id: string; started_at: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const at = (minAgo: number) => new Date(T0 - minAgo * 60_000).toISOString();

function slice(hostId: string, mins: number[], over: Partial<HostSlice<Row>> = {}): HostSlice<Row> {
  return {
    ...emptySlice<Row>({ id: hostId, name: hostId, transport_kind: 'ssh' }),
    state: 'ok',
    rows: mins.map((m, i) => ({ id: `${hostId}-${i}`, started_at: at(m), host_id: hostId })),
    hasMore: false,
    ...over,
  };
}

describe('watermarkMerge', () => {
  it('orders by parsed instant across Z / +00:00 / fractional-second stamps', () => {
    const a = slice('a', []);
    a.rows = [{ id: 'z', started_at: '2026-09-30T11:00:00Z', host_id: 'a' }];
    const b = slice('b', []);
    b.rows = [
      { id: 'offset', started_at: '2026-09-30T11:30:00+00:00', host_id: 'b' },
      { id: 'frac', started_at: '2026-09-30T10:59:59.500Z', host_id: 'b' },
    ];
    expect(watermarkMerge([a, b], access).visible.map((r) => r.id)).toEqual(['offset', 'z', 'frac']);
  });

  it('sorts an unparseable stamp last', () => {
    const a = slice('a', [5]);
    a.rows.push({ id: 'bad', started_at: 'not-a-date', host_id: 'a' });
    expect(watermarkMerge([a], access).visible.map((r) => r.id)).toEqual(['a-0', 'bad']);
  });

  it('holds back rows older than the newest covered point of a host that still has more', () => {
    const local = slice('local', [1, 2, 3, 4], { hasMore: false });
    const remote = slice('remote', [0, 2.5], { hasMore: true }); // covered at 2.5 min ago
    const m = watermarkMerge([local, remote], access);
    expect(m.floor).toBe(T0 - 2.5 * 60_000);
    expect(m.visible.map((r) => r.id)).toEqual(['remote-0', 'local-0', 'local-1', 'remote-1']);
    expect(m.hasMore).toBe(true);
    expect(m.gatingHosts).toEqual(['remote']);
  });

  it('does not let loading, catching-up, offline or paging-failed hosts gate the floor', () => {
    const local = slice('local', [1, 9], { hasMore: true }); // covered at 9
    const gates = (over: Partial<HostSlice<Row>>) =>
      floorOf([local, slice('r', [0], { hasMore: true, ...over })], access.timeOf);
    expect(gates({ state: 'loading' })).toBe(T0 - 9 * 60_000);
    expect(gates({ catchingUp: true })).toBe(T0 - 9 * 60_000);
    expect(gates({ state: 'offline' })).toBe(T0 - 9 * 60_000);
    expect(gates({ pagingFailed: true })).toBe(T0 - 9 * 60_000);
    expect(gates({})).toBe(T0); // the healthy remote gates at its own covered point
  });

  it('de-duplicates by (host_id, id), the row\'s own tag winning over the slice', () => {
    const a = slice('local', [1]);
    const b = slice('host_prod', []);
    b.rows = [{ id: 'local-0', started_at: at(1), host_id: 'local' }]; // a mock answering with local rows
    expect(watermarkMerge([a, b], access).visible).toHaveLength(1);
  });

  it('is ended only when nothing gates and no host is loading or catching up', () => {
    expect(watermarkMerge([slice('a', [1])], access).ended).toBe(true);
    expect(watermarkMerge([slice('a', [1]), slice('b', [], { state: 'loading' })], access).ended).toBe(false);
    expect(watermarkMerge([slice('a', [1]), slice('b', [2], { catchingUp: true, hasMore: true })], access).ended).toBe(false);
    expect(watermarkMerge([slice('a', [1], { hasMore: true })], access).ended).toBe(false);
    expect(watermarkMerge([], access).ended).toBe(false);
  });

  it('counts the visible rows a host would push below the floor', () => {
    const visible = slice('local', [1, 2, 3, 4]).rows;
    expect(displacedCount(visible, T0 - 2.5 * 60_000, access.timeOf)).toBe(2);
    expect(displacedCount(visible, -Infinity, access.timeOf)).toBe(0);
  });
});
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/watermarkMerge.test.ts`
Expected: FAIL, `Cannot find module './watermarkMerge'`.

- [ ] **Step 4: Write `watermarkMerge.ts`**

```ts
// Watermark merge — the k-way merge behind per-host infinite scroll (spec §6.2).
//
// Each host is paged independently, so a row from one host can only be
// placed once every other host that still has more pages has loaded down to
// at least that row's time. The FLOOR is the newest of those "loaded down
// to" points. Rows below it are held back until the host(s) sitting at the
// floor load their next page.

import type { HostSlice, RowAccess } from './types';

/** Oldest instant this host has loaded, or `-Infinity` when it has nothing more to give. */
export function coveredOf<T>(s: HostSlice<T>, timeOf: (r: T) => number): number {
  if (!s.hasMore || s.rows.length === 0) return -Infinity;
  let min = Infinity;
  for (const r of s.rows) min = Math.min(min, timeOf(r));
  return min;
}

/** A host decides where the list currently ends only when it is healthy and has more. */
export function isGating<T>(s: HostSlice<T>): boolean {
  return s.state === 'ok' && s.hasMore && !s.catchingUp && !s.pagingFailed;
}

export function floorOf<T>(slices: readonly HostSlice<T>[], timeOf: (r: T) => number): number {
  let floor = -Infinity;
  for (const s of slices) if (isGating(s)) floor = Math.max(floor, coveredOf(s, timeOf));
  return floor;
}

/** Newest first; ties by merge key, i.e. (host_id, id). Equal `-Infinity` stamps tie too. */
export function compareNewestFirst(a: { t: number; key: string }, b: { t: number; key: string }): number {
  if (a.t !== b.t) return a.t > b.t ? -1 : 1;
  return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

export interface MergeResult<T> {
  /** Correctly-placed rows, newest first. */
  visible: T[];
  floor: number;
  /** Some host can still load older rows (drives the scroll sentinel). */
  hasMore: boolean;
  /** Nothing gates, and no host is still loading or catching up. */
  ended: boolean;
  /** The gating hosts sitting exactly at the floor: the next page comes from these only. */
  gatingHosts: string[];
}

export function watermarkMerge<T>(slices: readonly HostSlice<T>[], access: RowAccess<T>): MergeResult<T> {
  const floor = floorOf(slices, access.timeOf);
  const seen = new Set<string>();
  const entries: { row: T; t: number; key: string }[] = [];
  for (const s of slices) {
    for (const row of s.rows) {
      const t = access.timeOf(row);
      if (t < floor) continue;
      const key = access.keyOf(row, s.hostId);
      if (seen.has(key)) continue;
      seen.add(key);
      entries.push({ row, t, key });
    }
  }
  entries.sort(compareNewestFirst);
  const gating = slices.filter(isGating);
  return {
    visible: entries.map((e) => e.row),
    floor,
    hasMore: gating.length > 0,
    ended: slices.length > 0 && gating.length === 0 && !slices.some((s) => s.state === 'loading' || s.catchingUp),
    gatingHosts: gating.filter((s) => coveredOf(s, access.timeOf) === floor).map((s) => s.hostId),
  };
}

/** How many of `visible` would drop below the floor if a host covered to `covered` started gating. */
export function displacedCount<T>(visible: readonly T[], covered: number, timeOf: (r: T) => number): number {
  let n = 0;
  for (const r of visible) if (timeOf(r) < covered) n++;
  return n;
}
```

- [ ] **Step 5: Run the merge tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/watermarkMerge.test.ts`
Expected: 7 passed.

- [ ] **Step 6: Write the failing tests `spliceHead.test.ts`**

```ts
import { describe, expect, it } from 'vitest';
import { rowAccess } from './types';
import { appendPage, spliceHead, withHost } from './spliceHead';

interface Row { id: string; started_at: string; status?: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const r = (id: string, minAgo: number, status = 'running'): Row => ({
  id, status, host_id: 'h', started_at: new Date(T0 - minAgo * 60_000).toISOString(),
});
/** r1..rN, one minute apart, newest first. */
const seq = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => r(`r${from + i}`, from + i));
const ids = (rows: Row[]) => rows.map((x) => x.id);

describe('spliceHead', () => {
  it('keeps the boundary row when a new run arrives at the top (usePagedList dropped it)', () => {
    const old = seq(1, 40);
    const fresh = [r('n1', 0), ...seq(1, 19)]; // page 0 now ends at r19
    const { rows, fullyListed } = spliceHead(old, fresh, 20, access, 'h');
    expect(fullyListed).toBe(false);
    expect(ids(rows)).toEqual(['n1', ...ids(seq(1, 40))]);
  });

  it('drops a run that left the list instead of duplicating its neighbour', () => {
    const old = seq(1, 25);
    const fresh = seq(1, 21).filter((x) => x.id !== 'r3'); // r3 finished
    const { rows } = spliceHead(old, fresh, 20, access, 'h');
    expect(ids(rows)).toEqual(ids(seq(1, 25).filter((x) => x.id !== 'r3')));
    expect(new Set(ids(rows)).size).toBe(rows.length);
  });

  it('takes the fresh copy of a row present in both (status update lands)', () => {
    const old = seq(1, 20);
    const fresh = seq(1, 20).map((x) => (x.id === 'r2' ? { ...x, status: 'completed' } : x));
    expect(spliceHead(old, fresh, 20, access, 'h').rows.find((x) => x.id === 'r2')?.status).toBe('completed');
  });

  it('replaces the slice when page 0 is short (the host is fully listed)', () => {
    const { rows, fullyListed } = spliceHead(seq(1, 30), seq(1, 7), 20, access, 'h');
    expect(fullyListed).toBe(true);
    expect(ids(rows)).toEqual(ids(seq(1, 7)));
  });

  it('keeps a timestamp tie at the cutoff', () => {
    const tie = { ...r('tie', 20) };
    const old = [...seq(1, 20), tie];
    const fresh = seq(1, 20); // cutoff = r20's time = tie's time
    expect(ids(spliceHead(old, fresh, 20, access, 'h').rows)).toContain('tie');
  });
});

describe('appendPage / withHost', () => {
  it('appends with de-dup, the page copy winning', () => {
    const old = seq(1, 5);
    const page = [...seq(4, 8)].map((x) => (x.id === 'r4' ? { ...x, status: 'completed' } : x));
    const rows = appendPage(old, page, access, 'h');
    expect(ids(rows)).toEqual(ids(seq(1, 8)));
    expect(rows.find((x) => x.id === 'r4')?.status).toBe('completed');
  });

  it('fills host_id only when the server did not tag the row', () => {
    const rows = withHost<Row>([{ id: 'a', started_at: '' }, { id: 'b', started_at: '', host_id: 'x' }], 'h');
    expect(rows.map((x) => x.host_id)).toEqual(['h', 'x']);
  });
});
```

- [ ] **Step 7: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/spliceHead.test.ts`
Expected: FAIL, `Cannot find module './spliceHead'`.

- [ ] **Step 8: Write `spliceHead.ts`**

```ts
// Per-host slice maintenance: appending a page, and splicing a refreshed
// page 0 over a host's existing rows (spec §6.3 "Refresh merge rule").
//
// Replaces usePagedList's `[...page0, ...rows.slice(page0.length)]`, which
// dropped the boundary row when one new run arrived at the top, and
// duplicated a row when a run left the list (e.g. finished and left the
// Running tab).

import type { HostTagged, RowAccess } from './types';
import { compareNewestFirst } from './watermarkMerge';

/** Tag rows from an older server that does not set `host_id`. */
export function withHost<T extends HostTagged>(rows: readonly T[], hostId: string): T[] {
  return rows.map((r) => (r.host_id ? r : { ...r, host_id: hostId }));
}

/** Newest first, de-duplicated by key. The FIRST copy of a key wins. */
export function sortSlice<T>(rows: readonly T[], access: RowAccess<T>, hostId: string): T[] {
  const seen = new Set<string>();
  const entries: { row: T; t: number; key: string }[] = [];
  for (const row of rows) {
    const key = access.keyOf(row, hostId);
    if (seen.has(key)) continue;
    seen.add(key);
    entries.push({ row, t: access.timeOf(row), key });
  }
  entries.sort(compareNewestFirst);
  return entries.map((e) => e.row);
}

/** Add a fetched page. The page's copy of an overlapping row wins (fresher status). */
export function appendPage<T>(old: readonly T[], page: readonly T[], access: RowAccess<T>, hostId: string): T[] {
  return sortSlice([...page, ...old], access, hostId);
}

export interface SpliceResult<T> {
  rows: T[];
  /** Page 0 came back short: it is this host's whole list. */
  fullyListed: boolean;
}

/**
 * Splice a fresh page 0 (requested with `limit`) over a host's rows. Within
 * the span the page covers, it is authoritative: old rows newer than its
 * oldest row that it no longer contains have left the list. Older rows are
 * kept. A row exactly at the cutoff is kept so a timestamp tie is never lost.
 */
export function spliceHead<T>(
  old: readonly T[],
  fresh: readonly T[],
  limit: number,
  access: RowAccess<T>,
  hostId: string,
): SpliceResult<T> {
  if (fresh.length < limit) return { rows: sortSlice(fresh, access, hostId), fullyListed: true };
  let cutoff = Infinity;
  for (const r of fresh) cutoff = Math.min(cutoff, access.timeOf(r));
  const freshKeys = new Set(fresh.map((r) => access.keyOf(r, hostId)));
  const kept = old.filter((o) => access.timeOf(o) <= cutoff && !freshKeys.has(access.keyOf(o, hostId)));
  return { rows: sortSlice([...fresh, ...kept], access, hostId), fullyListed: false };
}
```

- [ ] **Step 9: Run both test files**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/`
Expected: 14 passed.

- [ ] **Step 10: Commit**

```bash
git add crates/rupu-cp/web/src/lib/perHost/
git commit -m "$(cat <<'EOF'
feat(cp-web): per-host slice model, watermark merge and head splice

Pure building blocks for per-host progressive lists: rows merge by
parsed instant (not raw string), a floor holds back rows another host
could still outrank, and a refreshed page 0 splices in without losing
the boundary row or duplicating one that left the list.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Status helpers

**Files:**
- Create: `crates/rupu-cp/web/src/lib/perHost/status.ts`
- Create: `crates/rupu-cp/web/src/lib/perHost/testUtils.ts`
- Test: `crates/rupu-cp/web/src/lib/perHost/status.test.ts`

**Interfaces:**
- Consumes: `HostSlice` (Task 4); `ApiError`, `apiErrorMessage` from `../api`; `HostFreshnessEntry` from `../../components/dashboard/HostFreshnessStrip`.
- Produces:
  - `type Failure = { kind: 'offline' | 'unavailable' | 'gone'; reason: string }`
  - `classifyFailure(e: unknown): Failure`
  - `waitingOn(slices): string[]` and `waitingLabel(slices): string | null`
  - `notIncluded(slices): string | null`
  - `pagingFailedHosts(slices): { hostId: string; name: string }[]`
  - `toFreshnessEntries(slices): HostFreshnessEntry[]`
  - from `testUtils.ts`: `deferred<T>()`, `REG_LOCAL`, `REG_PROD`, `callsFor(spy, host)`, `onlyHost(host, rows)`

- [ ] **Step 1: Write `testUtils.ts`.** It is a test-only helper; no production code imports it.

```ts
// Test-only helpers for per-host loading. Imported by *.test.ts(x) files only.
import type { RegisteredHostView } from '../api';

export const REG_LOCAL: RegisteredHostView = { id: 'local', name: 'Local', transport_kind: 'local' };
export const REG_PROD: RegisteredHostView = { id: 'host_prod', name: 'prod', transport_kind: 'http_cp' };

export function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** The calls a list spy received for `host` (the per-host loader passes `host` on every call). */
export function callsFor(spy: { mock: { calls: unknown[][] } }, host: string): unknown[][] {
  return spy.mock.calls.filter((c) => (c[0] as { host?: string } | undefined)?.host === host);
}

/** A list-mock implementation that answers `rows` for `host` and `[]` for every other host. */
export function onlyHost<R>(host: string, rows: R[]) {
  return (p?: { host?: string }) => Promise.resolve(p?.host === host ? rows : []);
}

/** Let pending promise callbacks run (real timers). */
export const flush = () => new Promise<void>((r) => setTimeout(r, 0));
```

- [ ] **Step 2: Write the failing tests `status.test.ts`**

```ts
import { describe, expect, it } from 'vitest';
import { ApiError } from '../api';
import { emptySlice, type HostSlice } from './types';
import { classifyFailure, notIncluded, pagingFailedHosts, toFreshnessEntries, waitingLabel } from './status';

const s = (id: string, over: Partial<HostSlice<unknown>> = {}): HostSlice<unknown> => ({
  ...emptySlice({ id, name: id, transport_kind: 'ssh' }),
  ...over,
});

describe('classifyFailure', () => {
  it('maps 501 → unavailable, 404 → gone, anything else → offline, with the server reason', () => {
    expect(classifyFailure(new ApiError(501, 'x', '{"error":"needs a newer rupu"}'))).toEqual({
      kind: 'unavailable',
      reason: 'needs a newer rupu',
    });
    expect(classifyFailure(new ApiError(404, 'x', '{"error":"host gone"}')).kind).toBe('gone');
    expect(classifyFailure(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}'))).toEqual({
      kind: 'offline',
      reason: 'host unreachable: timed out',
    });
    expect(classifyFailure(new TypeError('Failed to fetch')).kind).toBe('offline');
  });
});

describe('labels', () => {
  it('names hosts still loading or catching up', () => {
    expect(waitingLabel([s('local', { state: 'ok' }), s('mini'), s('kuki', { state: 'ok', catchingUp: true })])).toBe(
      'Waiting on mini, kuki…',
    );
    expect(waitingLabel([s('local', { state: 'ok' })])).toBeNull();
  });

  it('names hosts not included and why', () => {
    expect(notIncluded([s('a', { state: 'offline' }), s('b', { state: 'unavailable' }), s('c', { state: 'ok' })])).toBe(
      'a (offline), b (unavailable)',
    );
    expect(notIncluded([s('c', { state: 'ok' })])).toBeNull();
  });

  it('lists paging failures', () => {
    expect(pagingFailedHosts([s('a', { state: 'ok', pagingFailed: true }), s('b', { state: 'ok' })])).toEqual([
      { hostId: 'a', name: 'a' },
    ]);
  });
});

describe('toFreshnessEntries', () => {
  it('shows catching-up as loading and stamps ok hosts with their receipt time', () => {
    const [a, b, c] = toFreshnessEntries([
      s('a', { state: 'ok', receivedAt: Date.parse('2026-09-30T12:00:00Z') }),
      s('b', { state: 'ok', catchingUp: true, receivedAt: 1 }),
      s('c', { state: 'offline', reason: 'down' }),
    ]);
    expect(a).toMatchObject({ host_id: 'a', state: 'ok', captured_at: '2026-09-30T12:00:00.000Z' });
    expect(b.state).toBe('loading');
    expect(c).toMatchObject({ state: 'offline', captured_at: null, reason: 'down' });
  });
});
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/status.test.ts`
Expected: FAIL, `Cannot find module './status'`.

- [ ] **Step 4: Write `status.ts`**

```ts
// Honest per-host state for list pages (spec §8): failure classification,
// "waiting on …", "not included: …", and the freshness-strip mapping.

import { ApiError, apiErrorMessage } from '../api';
import type { HostFreshnessEntry } from '../../components/dashboard/HostFreshnessStrip';
import type { HostSlice } from './types';

export type Failure = { kind: 'offline' | 'unavailable' | 'gone'; reason: string };

/**
 * The single-host list paths answer 501 when a host cannot serve the
 * listing (e.g. an old remote rupu), 404 when the host is no longer
 * registered, and 502 when it gave no usable answer (spec §7.1). Anything
 * else, including a network error, is treated as offline.
 */
export function classifyFailure(e: unknown): Failure {
  const reason = apiErrorMessage(e);
  if (e instanceof ApiError) {
    if (e.status === 501) return { kind: 'unavailable', reason };
    if (e.status === 404) return { kind: 'gone', reason };
  }
  return { kind: 'offline', reason };
}

export function waitingOn<T>(slices: readonly HostSlice<T>[]): string[] {
  return slices.filter((s) => s.state === 'loading' || s.catchingUp).map((s) => s.name);
}

export function waitingLabel<T>(slices: readonly HostSlice<T>[]): string | null {
  const names = waitingOn(slices);
  return names.length ? `Waiting on ${names.join(', ')}…` : null;
}

export function notIncluded<T>(slices: readonly HostSlice<T>[]): string | null {
  const out = slices
    .filter((s) => s.state === 'offline' || s.state === 'unavailable')
    .map((s) => `${s.name} (${s.state})`);
  return out.length ? out.join(', ') : null;
}

export function pagingFailedHosts<T>(slices: readonly HostSlice<T>[]): { hostId: string; name: string }[] {
  return slices.filter((s) => s.pagingFailed).map((s) => ({ hostId: s.hostId, name: s.name }));
}

/**
 * Feed for `HostFreshnessStrip`. A catching-up host is still filling in, so
 * it shows as loading. Age is measured from when this browser received the
 * host's answer.
 */
export function toFreshnessEntries<T>(slices: readonly HostSlice<T>[]): HostFreshnessEntry[] {
  return slices.map((s) => ({
    host_id: s.hostId,
    name: s.name,
    transport_kind: s.transportKind,
    state: s.catchingUp ? 'loading' : s.state,
    captured_at: s.state === 'ok' && s.receivedAt != null ? new Date(s.receivedAt).toISOString() : null,
    reason: s.reason,
  }));
}
```

- [ ] **Step 5: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/status.test.ts`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/lib/perHost/status.ts crates/rupu-cp/web/src/lib/perHost/status.test.ts crates/rupu-cp/web/src/lib/perHost/testUtils.ts
git commit -m "$(cat <<'EOF'
feat(cp-web): per-host status helpers (failure kinds, waiting/excluded labels)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: `PerHostListEngine`

**Files:**
- Create: `crates/rupu-cp/web/src/lib/perHost/engine.ts`
- Test: `crates/rupu-cp/web/src/lib/perHost/engine.test.ts`

**Interfaces:**
- Consumes: Tasks 4–5.
- Produces:
  - `PAGE = 20`, `OVERLAP = 5`, `MAX_LIMIT = 200`
  - `interface PerHostFetchParams { host: string; offset: number; limit: number }`
  - `class PerHostListEngine<T extends HostTagged>`:
    - `constructor(fetchPage: (p: PerHostFetchParams) => Promise<T[]>, access: RowAccess<T>, onChange: (slices: HostSlice<T>[]) => void, allHosts: boolean)`
    - `current: HostSlice<T>[]`
    - `start(hosts: readonly HostSeed[]): void` and `reconcile(hosts: readonly HostSeed[]): void`
    - `scheduleHead(id): Promise<void>` and `refreshHost(id): void`
    - `loadMore(): Promise<void>`
    - `retryPaging(id): Promise<void>`
    - `removeRow(hostId, id): void`
    - `pollLocal(): void` and `pollRemote(): void`
    - `dispose(): void`

- [ ] **Step 1: Write the failing tests `engine.test.ts`**

```ts
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api';
import { PerHostListEngine, type PerHostFetchParams } from './engine';
import { rowAccess, type HostSlice } from './types';
import { watermarkMerge } from './watermarkMerge';
import { deferred, flush } from './testUtils';

interface Row { id: string; started_at: string; host_id?: string }
const access = rowAccess<Row>('started_at', 'id');
const T0 = Date.parse('2026-09-30T12:00:00Z');
const at = (minAgo: number) => new Date(T0 - minAgo * 60_000).toISOString();
/** `n` rows `${host}-0…`, `step` minutes apart, the first `from` minutes ago. */
function rows(host: string, n: number, step: number, from = 0): Row[] {
  return Array.from({ length: n }, (_, i) => ({ id: `${host}-${i}`, host_id: host, started_at: at(from + i * step) }));
}
const LOCAL = { id: 'local', name: 'Local', transport_kind: 'local' };
const REMOTE = { id: 'remote', name: 'remote', transport_kind: 'ssh' };

type Script = (p: PerHostFetchParams) => Promise<Row[]>;
function harness(script: Script) {
  const history: HostSlice<Row>[][] = [];
  const fetch = vi.fn(script);
  const engine = new PerHostListEngine<Row>(fetch, access, (s) => history.push(s), true);
  const visible = () => watermarkMerge(engine.current, access).visible;
  return { engine, fetch, history, visible };
}
/** Serve `all` (newest first) by offset/limit, like a real host. */
const pager = (all: Row[]) => (p: PerHostFetchParams) => Promise.resolve(all.slice(p.offset, p.offset + p.limit));

afterEach(() => vi.restoreAllMocks());

describe('PerHostListEngine', () => {
  it('paints local while a remote hangs, then merges the remote in', async () => {
    const remote = deferred<Row[]>();
    const { engine, visible } = harness((p) => (p.host === 'local' ? Promise.resolve(rows('local', 3, 10)) : remote.promise));
    engine.start([LOCAL, REMOTE]);
    await flush();
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'local-1', 'local-2']);
    expect(engine.current.find((s) => s.hostId === 'remote')?.state).toBe('loading');

    remote.resolve(rows('remote', 2, 10, 5));
    await flush();
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'remote-0', 'local-1', 'remote-1', 'local-2']);
  });

  it('classifies failures: 502 offline, 501 unavailable, 404 removes the host', async () => {
    const fail = (status: number) => Promise.reject(new ApiError(status, 'x', `{"error":"e${status}"}`));
    const { engine } = harness((p) =>
      p.host === 'a' ? fail(502) : p.host === 'b' ? fail(501) : p.host === 'c' ? fail(404) : Promise.resolve([]),
    );
    engine.start([LOCAL, { ...REMOTE, id: 'a' }, { ...REMOTE, id: 'b' }, { ...REMOTE, id: 'c' }]);
    await flush();
    const by = (id: string) => engine.current.find((s) => s.hostId === id);
    expect(by('a')).toMatchObject({ state: 'offline', reason: 'e502' });
    expect(by('b')).toMatchObject({ state: 'unavailable', reason: 'e501' });
    expect(by('c')).toBeUndefined();
  });

  it('keeps last-good rows when a refresh fails (stale-on-error)', async () => {
    let fail = false;
    const { engine, visible } = harness(() => (fail ? Promise.reject(new ApiError(502, 'x', '{"error":"down"}')) : Promise.resolve(rows('local', 2, 1))));
    engine.start([LOCAL]);
    await flush();
    fail = true;
    await engine.scheduleHead('local');
    expect(visible()).toHaveLength(2);
    expect(engine.current[0]).toMatchObject({ state: 'ok', reason: 'down' });
  });

  it('loads the next page only from the host sitting at the floor', async () => {
    const local = rows('local', 60, 1); // one per minute
    const remote = rows('remote', 60, 10); // one per 10 minutes
    const { engine, fetch } = harness((p) => pager(p.host === 'local' ? local : remote)(p));
    engine.start([LOCAL, REMOTE]);
    await flush();
    fetch.mockClear();
    await engine.loadMore();
    // local covers 19 min, remote 190 min → the floor is local's; only local pages.
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
    expect(fetch.mock.calls[0][0]).toEqual({ host: 'local', offset: 15, limit: 25 });
  });

  it('a late host catches up instead of shrinking the visible list', async () => {
    const local = rows('local', 60, 1);
    const remote = rows('remote', 200, 0.1); // busy: 10 rows a minute
    const remoteGate = deferred<void>();
    const { engine, history } = harness(async (p) => {
      if (p.host === 'remote') await remoteGate.promise;
      return pager(p.host === 'local' ? local : remote)(p);
    });
    engine.start([LOCAL, REMOTE]);
    await flush();
    const before = watermarkMerge(engine.current, access).visible.length;
    expect(before).toBe(20);

    remoteGate.resolve();
    for (let i = 0; i < 10; i++) await flush();
    const counts = history.map((h) => watermarkMerge(h, access).visible.length);
    const afterArrival = counts.slice(counts.findIndex((_, i) => history[i].some((s) => s.hostId === 'remote' && s.state === 'ok')));
    for (const n of afterArrival) expect(n).toBeGreaterThanOrEqual(before);
    expect(engine.current.find((s) => s.hostId === 'remote')?.catchingUp).toBe(false);
  });

  it('never has more than one request in flight per host, and coalesces refreshes', async () => {
    let inflight = 0;
    let peak = 0;
    const gate = deferred<void>();
    const { engine, fetch } = harness(async () => {
      inflight++;
      peak = Math.max(peak, inflight);
      await gate.promise;
      inflight--;
      return rows('local', 3, 1);
    });
    engine.start([LOCAL]);
    await flush(); // page 0 is now in flight (a schedule BEFORE it starts would coalesce into it)
    void engine.scheduleHead('local'); // queued behind it
    void engine.scheduleHead('local'); // coalesced into the queued one
    gate.resolve();
    for (let i = 0; i < 5; i++) await flush();
    expect(peak).toBe(1);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it('a paging failure stops the host gating; retry clears it', async () => {
    const local = rows('local', 60, 1);
    let failNext = true;
    const { engine } = harness((p) => {
      if (p.offset > 0 && failNext) return Promise.reject(new ApiError(502, 'x', '{"error":"down"}'));
      return pager(local)(p);
    });
    engine.start([LOCAL]);
    await flush();
    await engine.loadMore();
    expect(engine.current[0]).toMatchObject({ pagingFailed: true, reason: 'down' });
    expect(watermarkMerge(engine.current, access).hasMore).toBe(false);
    failNext = false;
    await engine.retryPaging('local');
    expect(engine.current[0].pagingFailed).toBe(false);
  });

  it('removeRow drops one row from its host', async () => {
    const { engine, visible } = harness(() => Promise.resolve(rows('local', 3, 1)));
    engine.start([LOCAL]);
    await flush();
    engine.removeRow('local', 'local-1');
    expect(visible().map((r) => r.id)).toEqual(['local-0', 'local-2']);
  });

  it('ignores answers after dispose', async () => {
    const d = deferred<Row[]>();
    const { engine, history } = harness(() => d.promise);
    engine.start([LOCAL]);
    const n = history.length;
    engine.dispose();
    d.resolve(rows('local', 1, 1));
    await flush();
    expect(history.length).toBe(n);
  });

  it('pollLocal refreshes only local; pollRemote skips unavailable hosts', async () => {
    const { engine, fetch } = harness((p) =>
      p.host === 'old' ? Promise.reject(new ApiError(501, 'x', '{"error":"old"}')) : Promise.resolve([]),
    );
    engine.start([LOCAL, REMOTE, { ...REMOTE, id: 'old' }]);
    await flush();
    fetch.mockClear();
    engine.pollLocal();
    await flush();
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
    fetch.mockClear();
    engine.pollRemote();
    await flush();
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['remote']);
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/engine.test.ts`
Expected: FAIL, `Cannot find module './engine'`.

- [ ] **Step 3: Write `engine.ts`**

```ts
// PerHostListEngine — per-host list state outside React (spec §6.3).
//
// - Serializes work per host: at most one request in flight per host, with
//   a refresh that comes due while one is in flight coalesced behind it.
// - A host that answers late (first page, or recovering) JOINS by catching
//   up to the floor before it gates, so the visible list never gets shorter.
// - Splices refreshes with spliceHead and pages with a 5-row overlap.
//
// The React hook (usePerHostPagedList.ts) owns one engine per filter
// generation and disposes it on change. A disposed engine ignores every
// late answer.

import { apiErrorMessage } from '../api';
import { emptySlice, type HostSeed, type HostSlice, type HostTagged, type RowAccess } from './types';
import { coveredOf, displacedCount, floorOf, isGating, watermarkMerge } from './watermarkMerge';
import { appendPage, sortSlice, spliceHead, withHost } from './spliceHead';
import { classifyFailure } from './status';

export const PAGE = 20;
export const OVERLAP = 5;
export const MAX_LIMIT = 200;

export interface PerHostFetchParams {
  host: string;
  offset: number;
  limit: number;
}

export class PerHostListEngine<T extends HostTagged> {
  private slices: HostSlice<T>[] = [];
  private readonly queues = new Map<string, Promise<void>>();
  private readonly headQueued = new Set<string>();
  private disposed = false;

  constructor(
    private readonly fetchPage: (p: PerHostFetchParams) => Promise<T[]>,
    private readonly access: RowAccess<T>,
    private readonly onChange: (slices: HostSlice<T>[]) => void,
    /** Listing every registered host (vs one picked host). Only then does a 404 drop a host. */
    private readonly allHosts: boolean,
  ) {}

  get current(): HostSlice<T>[] {
    return this.slices;
  }

  dispose(): void {
    this.disposed = true;
  }

  /** Seed one `loading` slice per host and fire every host's page 0 independently. */
  start(hosts: readonly HostSeed[]): void {
    this.commit(hosts.map((h) => emptySlice<T>(h)));
    for (const h of hosts) void this.scheduleHead(h.id);
  }

  /** Manual Refresh: add new hosts, drop removed ones, refresh every host (unavailable ones included). */
  reconcile(hosts: readonly HostSeed[]): void {
    const byId = new Map(this.slices.map((s) => [s.hostId, s]));
    this.commit(hosts.map((h) => byId.get(h.id) ?? emptySlice<T>(h)));
    for (const h of hosts) void this.scheduleHead(h.id);
  }

  /** Coalesced page-0 load: a first load, a refresh or a recovery. */
  scheduleHead(id: string): Promise<void> {
    if (this.headQueued.has(id)) return Promise.resolve();
    this.headQueued.add(id);
    return this.enqueue(id, async () => {
      this.headQueued.delete(id);
      await this.head(id);
    });
  }

  refreshHost(id: string): void {
    void this.scheduleHead(id);
  }

  /** Scroll: next page from each host sitting at the floor. */
  loadMore(): Promise<void> {
    const { gatingHosts } = watermarkMerge(this.slices, this.access);
    return Promise.all(
      gatingHosts.map((id) =>
        this.enqueue(id, async () => {
          const s = this.find(id);
          if (s && isGating(s)) await this.next(id, PAGE);
        }),
      ),
    ).then(() => undefined);
  }

  retryPaging(id: string): Promise<void> {
    return this.enqueue(id, () => this.join(id, { pagingFailed: false, reason: null }));
  }

  /** A row action (archive/restore/delete) took this row out of the list. */
  removeRow(hostId: string, rowId: string): void {
    this.patch(hostId, (s) => ({ ...s, rows: s.rows.filter((r) => this.access.idOf(r) !== rowId) }));
  }

  /** The local cadence (5 s on polling tables). */
  pollLocal(): void {
    for (const s of this.slices) if (s.hostId === 'local' && s.state !== 'unavailable') void this.scheduleHead(s.hostId);
  }

  /** The remote cadence (60 s visible, and on focus). Unavailable hosts wait for a manual Refresh. */
  pollRemote(): void {
    for (const s of this.slices) if (s.hostId !== 'local' && s.state !== 'unavailable') void this.scheduleHead(s.hostId);
  }

  // ── internals ──────────────────────────────────────────────────────────

  private commit(next: HostSlice<T>[]): void {
    if (this.disposed) return;
    this.slices = next;
    this.onChange(next);
  }

  private find(id: string): HostSlice<T> | undefined {
    return this.slices.find((s) => s.hostId === id);
  }

  private patch(id: string, f: (s: HostSlice<T>) => HostSlice<T>): void {
    this.commit(this.slices.map((s) => (s.hostId === id ? f(s) : s)));
  }

  private enqueue(id: string, job: () => Promise<void>): Promise<void> {
    const prev = this.queues.get(id) ?? Promise.resolve();
    const next = prev.then(() => (this.disposed ? undefined : job())).catch(() => undefined);
    this.queues.set(id, next);
    return next;
  }

  private async head(id: string): Promise<void> {
    if (!this.find(id)) return;
    let page: T[];
    try {
      page = withHost(await this.fetchPage({ host: id, offset: 0, limit: PAGE }), id);
    } catch (e) {
      this.fail(id, e);
      return;
    }
    const cur = this.find(id);
    if (this.disposed || !cur) return;
    if (cur.state !== 'ok') {
      // First answer or recovery: hold it out of gating until join() decides.
      const hasMore = page.length === PAGE;
      this.patch(id, (s) => ({
        ...s,
        state: 'ok',
        rows: sortSlice(page, this.access, id),
        hasMore,
        catchingUp: hasMore,
        pagingFailed: false,
        reason: null,
        receivedAt: Date.now(),
      }));
      await this.join(id);
      return;
    }
    const { rows, fullyListed } = spliceHead(cur.rows, page, PAGE, this.access, id);
    this.patch(id, (s) => ({ ...s, rows, hasMore: fullyListed ? false : s.hasMore, reason: null, receivedAt: Date.now() }));
  }

  /**
   * Bring a host into the merge without shortening the visible list: fetch
   * at least as many rows as it would push below the floor (the budget),
   * stopping early once it reaches the floor or runs out of rows. Then it
   * gates.
   */
  private async join(id: string, prePatch: Partial<HostSlice<T>> = {}): Promise<void> {
    const start = this.find(id);
    if (!start) return;
    this.patch(id, (s) => ({ ...s, ...prePatch, catchingUp: s.hasMore }));
    const others = () => this.slices.filter((o) => o.hostId !== id);
    const budget = displacedCount(
      watermarkMerge(others(), this.access).visible,
      coveredOf({ ...start, ...prePatch } as HostSlice<T>, this.access.timeOf),
      this.access.timeOf,
    );
    let added = 0;
    for (;;) {
      const s = this.find(id);
      if (this.disposed || !s) return;
      const floor = floorOf(others(), this.access.timeOf);
      if (!s.hasMore || coveredOf(s, this.access.timeOf) <= floor || added >= budget) break;
      const got = await this.next(id, Math.min(MAX_LIMIT - OVERLAP, Math.max(PAGE, budget - added)));
      if (got === null) return; // next() marked pagingFailed and cleared catchingUp
      if (got === 0) break;
      added += got;
    }
    this.patch(id, (s) => ({ ...s, catchingUp: false }));
  }

  /** Fetch `want` more rows (+ overlap). Returns the count of NEW rows, or null on failure. */
  private async next(id: string, want: number): Promise<number | null> {
    const cur = this.find(id);
    if (!cur) return 0;
    const limit = want + OVERLAP;
    let page: T[];
    try {
      page = withHost(
        await this.fetchPage({ host: id, offset: Math.max(0, cur.rows.length - OVERLAP), limit }),
        id,
      );
    } catch (e) {
      if (!this.disposed) {
        this.patch(id, (s) => ({ ...s, pagingFailed: true, catchingUp: false, reason: apiErrorMessage(e) }));
      }
      return null;
    }
    if (this.disposed) return null;
    let added = 0;
    this.patch(id, (s) => {
      const rows = appendPage(s.rows, page, this.access, id);
      added = rows.length - s.rows.length;
      // A full page of nothing new means the offsets drifted (many rows
      // inserted above). Stop rather than re-request the same offset forever.
      return { ...s, rows, hasMore: page.length === limit && added > 0 };
    });
    return added;
  }

  private fail(id: string, e: unknown): void {
    if (this.disposed) return;
    const f = classifyFailure(e);
    if (f.kind === 'gone' && this.allHosts && id !== 'local') {
      this.commit(this.slices.filter((s) => s.hostId !== id));
      return;
    }
    this.patch(id, (s) =>
      s.state === 'ok'
        ? { ...s, reason: f.reason } // stale-on-error: keep last-good rows
        : { ...s, state: f.kind === 'unavailable' ? 'unavailable' : 'offline', reason: f.reason, catchingUp: false },
    );
  }
}
```

- [ ] **Step 4: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/engine.test.ts`
Expected: 10 passed.

If "a late host catches up…" fails, print `counts` and fix the engine, not the test. The test encodes the spec §6.3 property.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/perHost/engine.ts crates/rupu-cp/web/src/lib/perHost/engine.test.ts
git commit -m "$(cat <<'EOF'
feat(cp-web): PerHostListEngine — per-host queues, catch-up, splice

One request in flight per host with coalesced refreshes; a late or
recovering host catches up to the floor before gating so the visible
list never shrinks; paging failures stop a host gating instead of
wedging the list; 404 drops a removed host; dispose() ignores late
answers.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `usePerHostPagedList` hook and `PerHostStatus` UI

**Files:**
- Create: `crates/rupu-cp/web/src/lib/perHost/usePerHostPagedList.ts`
- Create: `crates/rupu-cp/web/src/components/lists/PerHostStatus.tsx`
- Test: `crates/rupu-cp/web/src/lib/perHost/usePerHostPagedList.test.tsx`, `crates/rupu-cp/web/src/components/lists/PerHostStatus.test.tsx`

**Interfaces:**
- Consumes: the engine (Task 6), status helpers (Task 5), `api.getRegisteredHosts`, `useInfiniteScroll`.
- Produces:
  - `LOCAL_POLL_MS = 5_000`, `REMOTE_POLL_MS = 60_000`
  - `usePerHostPagedList<T extends HostTagged>(opts: { host: string | null; fetch: (p: PerHostFetchParams) => Promise<T[]>; timeField: keyof T & string; idField: keyof T & string; deps: unknown[]; poll?: boolean })`, returning `{ rows: T[]; slices: HostSlice<T>[]; loading: boolean; error: string | null; hasMore: boolean; ended: boolean; sentinelRef; refresh(): void; refreshHost(id: string): void; retryPaging(id: string): void; removeRow(hostId: string, id: string): void }`
  - Re-exports `PerHostFetchParams`
  - `<PerHostStrip slices />`
  - `perHostFooterText({ slices, loading, hasMore, ended, count }): string`
  - `<PagingFailures slices onRetry />`

- [ ] **Step 1: Write the failing hook tests `usePerHostPagedList.test.tsx`**

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api } from '../api';
import { LOCAL_POLL_MS, REMOTE_POLL_MS, usePerHostPagedList } from './usePerHostPagedList';
import { REG_LOCAL, REG_PROD, callsFor } from './testUtils';

interface Row { id: string; started_at: string; host_id?: string }

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function useList(fetch: (p: { host: string }) => Promise<Row[]>, host: string | null = null, poll = false) {
  return usePerHostPagedList<Row>({ host, fetch, timeField: 'started_at', idField: 'id', deps: [], poll });
}

describe('usePerHostPagedList', () => {
  it('fetches every registered host independently', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const fetch = vi.fn((p: { host: string }) =>
      Promise.resolve(p.host === 'local' ? [{ id: 'a', started_at: '2026-09-30T12:00:00Z' }] : []),
    );
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    expect(fetch.mock.calls.map((c) => c[0].host).sort()).toEqual(['host_prod', 'local']);
    expect(result.current.rows[0].host_id).toBe('local');
    expect(result.current.slices.map((s) => s.state)).toEqual(['ok', 'ok']);
  });

  it('single-host mode skips the registered-hosts read', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts');
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch, 'host_prod'));
    await waitFor(() => expect(fetch).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })));
    expect(reg).not.toHaveBeenCalled();
  });

  it('falls back to this host when the host list cannot be read, and says so', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.error).toMatch(/Could not list hosts/));
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
  });

  it('polls local every 5s and remotes every 60s, remotes only while visible', async () => {
    // Only intervals are faked: waitFor and the engine's promise chains run on real timers.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch, null, true));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));

    await act(() => vi.advanceTimersByTimeAsync(LOCAL_POLL_MS));
    await waitFor(() => expect(callsFor(fetch, 'local')).toHaveLength(2));
    expect(callsFor(fetch, 'host_prod')).toHaveLength(1);

    const vis = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    await new Promise((r) => setTimeout(r, 10));
    expect(callsFor(fetch, 'host_prod')).toHaveLength(1);

    vis.mockReturnValue('visible');
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    await waitFor(() => expect(callsFor(fetch, 'host_prod')).toHaveLength(2));

    const before = callsFor(fetch, 'host_prod').length;
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await waitFor(() => expect(callsFor(fetch, 'host_prod').length).toBe(before + 1));
  });

  it('does not poll when poll is false', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('manual refresh re-reads the host list and picks up a new host', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.slices).toHaveLength(1));
    reg.mockResolvedValue([REG_LOCAL, REG_PROD]);
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.slices.map((s) => s.hostId)).toEqual(['local', 'host_prod']));
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/usePerHostPagedList.test.tsx`
Expected: FAIL, module not found.

- [ ] **Step 3: Write `usePerHostPagedList.ts`**

```ts
// usePerHostPagedList — per-host progressive list loading for the Activity
// tables (spec docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md §6.3).
//
// `host: null` lists every registered host: `/api/hosts/registered` (a
// store read, no SSH), then one request per host, merged by the watermark
// rule. `host: '<id>'` is one slice, so a page has one code path whatever
// its filter. The engine (engine.ts) owns per-host state; this hook owns one
// engine per filter generation and the cadence timers:
//   local  — every 5 s on polling tables (unchanged)
//   remote — every 60 s while the tab is visible, plus on tab focus
//   manual — Refresh re-reads the host list and refreshes every host

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, apiErrorMessage } from '../api';
import { useInfiniteScroll } from '../useInfiniteScroll';
import { PerHostListEngine, type PerHostFetchParams } from './engine';
import { rowAccess, type HostSeed, type HostSlice, type HostTagged } from './types';
import { watermarkMerge } from './watermarkMerge';

export type { PerHostFetchParams } from './engine';

export const LOCAL_POLL_MS = 5_000;
export const REMOTE_POLL_MS = 60_000;

const LOCAL_SEED: HostSeed = { id: 'local', name: 'Local', transport_kind: 'local' };

export interface UsePerHostPagedListOptions<T> {
  /** One host id, or `null` for every registered host. */
  host: string | null;
  fetch: (p: PerHostFetchParams) => Promise<T[]>;
  timeField: keyof T & string;
  idField: keyof T & string;
  /** Reactive "start over" trigger, compared by index (same contract as usePagedList). */
  deps: unknown[];
  poll?: boolean;
}

export interface UsePerHostPagedListResult<T> {
  rows: T[];
  slices: HostSlice<T>[];
  /** True until the host list is known and at least one host has answered. */
  loading: boolean;
  /** Set when the host list could not be read, or when EVERY host failed. */
  error: string | null;
  hasMore: boolean;
  ended: boolean;
  sentinelRef: (el: HTMLDivElement | null) => void;
  refresh: () => void;
  refreshHost: (hostId: string) => void;
  retryPaging: (hostId: string) => void;
  removeRow: (hostId: string, id: string) => void;
}

const toSeed = <T>(s: HostSlice<T>): HostSeed => ({ id: s.hostId, name: s.name, transport_kind: s.transportKind });

export function usePerHostPagedList<T extends HostTagged>({
  host,
  fetch,
  timeField,
  idField,
  deps,
  poll = false,
}: UsePerHostPagedListOptions<T>): UsePerHostPagedListResult<T> {
  // `fetch` closes over live filter state and is a fresh identity every
  // render, so it is read through a ref, never listed as a dependency (the
  // same reasoning as usePagedList).
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;
  const hostRef = useRef(host);
  hostRef.current = host;
  const access = useMemo(() => rowAccess<T>(timeField, idField), [timeField, idField]);

  const allDeps = [host, ...deps];
  const prevDepsRef = useRef<unknown[]>([]);
  const genRef = useRef(0);
  if (allDeps.length !== prevDepsRef.current.length || allDeps.some((d, i) => !Object.is(d, prevDepsRef.current[i]))) {
    prevDepsRef.current = allDeps;
    genRef.current += 1;
  }
  const gen = genRef.current;

  const [slices, setSlices] = useState<HostSlice<T>[]>([]);
  const [hostsKnown, setHostsKnown] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const engineRef = useRef<PerHostListEngine<T> | null>(null);

  useEffect(() => {
    const engine = new PerHostListEngine<T>((p) => fetchRef.current(p), access, setSlices, hostRef.current === null);
    engineRef.current = engine;
    setSlices([]);
    setHostsKnown(false);
    setListError(null);
    const one = hostRef.current;
    if (one !== null) {
      engine.start([{ id: one, name: one === 'local' ? 'Local' : one, transport_kind: one === 'local' ? 'local' : '' }]);
      setHostsKnown(true);
    } else {
      api.getRegisteredHosts().then(
        (hs) => {
          if (engineRef.current !== engine) return;
          engine.start(hs);
          setHostsKnown(true);
        },
        (e: unknown) => {
          if (engineRef.current !== engine) return;
          setListError(`Could not list hosts (${apiErrorMessage(e)}); showing this host only.`);
          engine.start([LOCAL_SEED]);
          setHostsKnown(true);
        },
      );
    }
    return () => engine.dispose();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `gen` IS the deps-changed signal (see above)
  }, [gen, access]);

  useEffect(() => {
    if (!poll) return;
    const local = setInterval(() => engineRef.current?.pollLocal(), LOCAL_POLL_MS);
    const remote = setInterval(() => {
      if (document.visibilityState === 'visible') engineRef.current?.pollRemote();
    }, REMOTE_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState !== 'visible') return;
      engineRef.current?.pollLocal();
      engineRef.current?.pollRemote();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearInterval(local);
      clearInterval(remote);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [poll, gen]);

  const merged = useMemo(() => watermarkMerge(slices, access), [slices, access]);
  const loading = !hostsKnown || (slices.length > 0 && slices.every((s) => s.state === 'loading'));
  const allFailed = slices.length > 0 && slices.every((s) => s.state === 'offline' || s.state === 'unavailable');
  const error =
    listError ??
    (allFailed
      ? slices.length === 1
        ? slices[0].reason ?? 'request failed'
        : slices.map((s) => `${s.name}: ${s.reason ?? s.state}`).join(' · ')
      : null);

  const loadMore = useCallback(() => engineRef.current?.loadMore() ?? Promise.resolve(), []);
  const { sentinelRef } = useInfiniteScroll({ hasMore: merged.hasMore && !loading, loadMore });

  const refresh = useCallback(() => {
    const engine = engineRef.current;
    if (!engine || engine.current.length === 0) return;
    if (hostRef.current !== null) {
      engine.reconcile(engine.current.map(toSeed));
      return;
    }
    api.getRegisteredHosts().then(
      (hs) => {
        if (engineRef.current === engine) engine.reconcile(hs);
      },
      () => {
        if (engineRef.current === engine) engine.reconcile(engine.current.map(toSeed));
      },
    );
  }, []);
  const refreshHost = useCallback((id: string) => engineRef.current?.refreshHost(id), []);
  const retryPaging = useCallback((id: string) => void engineRef.current?.retryPaging(id), []);
  const removeRow = useCallback((hostId: string, id: string) => engineRef.current?.removeRow(hostId, id), []);

  return {
    rows: merged.visible,
    slices,
    loading,
    error,
    hasMore: merged.hasMore,
    ended: merged.ended,
    sentinelRef,
    refresh,
    refreshHost,
    retryPaging,
    removeRow,
  };
}
```

- [ ] **Step 4: Run the hook tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost/usePerHostPagedList.test.tsx`
Expected: 6 passed.

- [ ] **Step 5: Write the failing UI tests `PerHostStatus.test.tsx`**

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { emptySlice, type HostSlice } from '../../lib/perHost/types';
import { PagingFailures, PerHostStrip, perHostFooterText } from './PerHostStatus';

afterEach(cleanup);

const s = (id: string, over: Partial<HostSlice<unknown>> = {}): HostSlice<unknown> => ({
  ...emptySlice({ id, name: id, transport_kind: 'ssh' }),
  state: 'ok',
  ...over,
});

describe('PerHostStrip', () => {
  it('renders one chip per host, and nothing for a single host', () => {
    const { container, rerender } = render(<PerHostStrip slices={[s('local')]} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<PerHostStrip slices={[s('local'), s('mini', { state: 'loading' })]} />);
    expect(screen.getByText('mini')).toBeInTheDocument();
    expect(screen.getByText('loading…')).toBeInTheDocument();
  });
});

describe('perHostFooterText', () => {
  const base = { loading: false, hasMore: false, ended: true, count: 7 };
  it('ends honestly, naming hosts that were not included', () => {
    expect(perHostFooterText({ ...base, slices: [s('local')] })).toBe('— end of 7 —');
    expect(perHostFooterText({ ...base, slices: [s('local'), s('mini', { state: 'offline' })] })).toBe(
      '— end of 7 — · not included: mini (offline)',
    );
  });
  it('says who it is waiting on instead of claiming the end', () => {
    expect(perHostFooterText({ ...base, ended: false, slices: [s('local'), s('kuki', { state: 'loading' })] })).toBe(
      'waiting on kuki…',
    );
  });
  it('keeps the existing loading / scroll copy', () => {
    expect(perHostFooterText({ ...base, loading: true, slices: [] })).toBe('loading more…');
    expect(perHostFooterText({ ...base, hasMore: true, ended: false, slices: [] })).toBe('scroll for more');
  });
});

describe('PagingFailures', () => {
  it('offers a retry per failed host', () => {
    const onRetry = vi.fn();
    render(<PagingFailures slices={[s('mini', { pagingFailed: true })]} onRetry={onRetry} />);
    expect(screen.getByText(/older rows from mini couldn't load/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry mini' }));
    expect(onRetry).toHaveBeenCalledWith('mini');
  });
});
```

- [ ] **Step 6: Write `PerHostStatus.tsx`**

```tsx
// Per-host list chrome (spec §6.4 / §8): the freshness strip, the honest
// sentinel footer, and per-host "older rows couldn't load · Retry".

import { HostFreshnessStrip } from '../dashboard/HostFreshnessStrip';
import type { HostSlice } from '../../lib/perHost/types';
import { notIncluded, pagingFailedHosts, toFreshnessEntries, waitingOn } from '../../lib/perHost/status';

/** Per-host freshness for an All-hosts list. Nothing for a single host. */
export function PerHostStrip<T>({ slices }: { slices: HostSlice<T>[] }) {
  if (slices.length < 2) return null;
  return (
    <div className="mb-3">
      <HostFreshnessStrip hosts={toFreshnessEntries(slices)} />
    </div>
  );
}

/** The sentinel line under a per-host list (`count` = rows the page shows after its own filters). */
export function perHostFooterText<T>(p: {
  slices: HostSlice<T>[];
  loading: boolean;
  hasMore: boolean;
  ended: boolean;
  count: number;
}): string {
  if (p.loading) return 'loading more…';
  if (p.hasMore) return 'scroll for more';
  const waiting = waitingOn(p.slices);
  if (!p.ended && waiting.length) return `waiting on ${waiting.join(', ')}…`;
  const missing = notIncluded(p.slices);
  return `— end of ${p.count} —${missing ? ` · not included: ${missing}` : ''}`;
}

export function PagingFailures<T>({ slices, onRetry }: { slices: HostSlice<T>[]; onRetry: (hostId: string) => void }) {
  const failed = pagingFailedHosts(slices);
  if (failed.length === 0) return null;
  return (
    <div className="py-1 text-center text-note text-status-failed">
      {failed.map((h) => (
        <span key={h.hostId} className="mr-3">
          older rows from {h.name} couldn't load ·{' '}
          <button type="button" className="underline" aria-label={`Retry ${h.name}`} onClick={() => onRetry(h.hostId)}>
            Retry
          </button>
        </span>
      ))}
    </div>
  );
}
```

- [ ] **Step 7: Run both test files and typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/perHost src/components/lists/PerHostStatus.test.tsx && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/web/src/lib/perHost/usePerHostPagedList.ts crates/rupu-cp/web/src/lib/perHost/usePerHostPagedList.test.tsx crates/rupu-cp/web/src/components/lists/PerHostStatus.tsx crates/rupu-cp/web/src/components/lists/PerHostStatus.test.tsx
git commit -m "$(cat <<'EOF'
feat(cp-web): usePerHostPagedList hook + per-host list chrome

Registered hosts are read probe-free, each host is fetched on its own;
local polls every 5s, remotes every 60s while visible and on focus.
PerHostStrip / perHostFooterText / PagingFailures give the list pages
honest per-host state.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `HostSelect` (All-hosts dropdown) reads the probe-free host list

**Files:**
- Modify: `crates/rupu-cp/web/src/components/HostSelect.tsx`
- Test: `crates/rupu-cp/web/src/components/HostSelect.test.tsx`

**Interfaces:**
- Consumes: `api.getRegisteredHosts(): Promise<RegisteredHostView[]>`.
- Produces: the same component API. The `allowAll` branch now reads `/api/hosts/registered`; the launcher branch still reads `/api/hosts`.

- [ ] **Step 1: Update the `allowAll` tests.** In `HostSelect.test.tsx`, inside `describe('HostSelect — allowAll (fan-out variant)', …)`, replace every `vi.spyOn(api, 'getHosts')` with `vi.spyOn(api, 'getRegisteredHosts')` (the mocked values type-check: `HostView` has every `RegisteredHostView` field). Add this test to that describe:

```tsx
  it('does not ask for host health (no probe) — only the registered list', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL, REMOTE]);
    const health = vi.spyOn(api, 'getHosts');
    render(<HostSelect value="local" onChange={vi.fn()} allowAll />);
    await waitFor(() => expect(reg).toHaveBeenCalled());
    expect(health).not.toHaveBeenCalled();
  });
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/HostSelect.test.tsx`
Expected: the allowAll tests FAIL; the component still calls `getHosts`.

- [ ] **Step 3: Change `HostSelect.tsx`**

Replace the import line with `import { api, type HostView, type RegisteredHostView } from '../lib/api';`.

Replace the state + effect with:

```tsx
  // The All-hosts filter only needs ids and names — the probe-free
  // `/api/hosts/registered` (spec 2026-10-01 §6.4). The launcher variant keeps
  // `/api/hosts`: its "(offline)" suffix matters when choosing where to launch.
  const [hosts, setHosts] = useState<HostView[] | null>(null);
  const [registered, setRegistered] = useState<RegisteredHostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (allowAll) {
      api
        .getRegisteredHosts()
        .then((hs) => {
          if (!cancelled) setRegistered(hs);
        })
        .catch(() => {
          if (!cancelled) setRegistered([]);
        });
    } else {
      api
        .getHosts()
        .then((hs) => {
          if (!cancelled) setHosts(hs);
        })
        .catch(() => {
          if (!cancelled) setHosts([]);
        });
    }
    return () => {
      cancelled = true;
    };
  }, [allowAll]);
```

In the `allowAll` render, change `(hosts ?? [])` to `(registered ?? [])`. Update the file-header comment's first line to: `// HostSelect — a small dropdown of registered hosts. The allowAll (list-filter) variant reads the probe-free api.getRegisteredHosts(); the launcher variant reads api.getHosts() for status.`

- [ ] **Step 4: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/HostSelect.test.tsx`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/HostSelect.tsx crates/rupu-cp/web/src/components/HostSelect.test.tsx
git commit -m "$(cat <<'EOF'
perf(cp-web): All-hosts HostSelect reads the probe-free host list

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: WorkflowRuns — All hosts by default, per-host loading

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/runs/WorkflowRuns.tsx`
- Test: `crates/rupu-cp/web/src/pages/runs/WorkflowRuns.test.tsx`, `WorkflowRuns.columnOrder.test.tsx`, `WorkflowRuns.preventDefault.test.tsx`

**Interfaces:**
- Consumes: `usePerHostPagedList`, `PerHostFetchParams` (Task 7); `PerHostStrip`, `perHostFooterText`, `PagingFailures` (Task 7); `notIncluded`, `waitingLabel` (Task 5); `REG_*`, `callsFor`, `onlyHost` (Task 5).

- [ ] **Step 1: Re-stub the tests and rewrite the host-filter tests.** In all three WorkflowRuns test files, replace `vi.spyOn(api, 'getHosts')` with `vi.spyOn(api, 'getRegisteredHosts')` (HostSelect and the hook both read it now).

In `WorkflowRuns.test.tsx`, replace the whole `describe('WorkflowRuns host filter — server-driven', …)` block with:

```tsx
describe('WorkflowRuns host filter — per-host loading (spec 2026-10-01)', () => {
  it('defaults to All hosts and fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
    expect(screen.getByLabelText('Host filter')).toHaveValue('__all__');
  });

  it('renders This host, All hosts, then registered (non-local) hosts', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('paints local rows while a remote host is still loading, naming it in the strip', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([makeRun({ id: 'run_l' })]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());
    expect(screen.getByText('loading…')).toBeInTheDocument();
  });

  it('shows an offline remote honestly without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([makeRun({ id: 'run_l' })])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/not included: prod \(offline\)/)).toBeInTheDocument());
    expect(screen.getByText('deploy-prod')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('This host fetches only local', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    runsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(0);
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });
    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });
});
```

Replace the poll test (`'only the active/Running lifecycle polls every 5s (unchanged semantics)'`) with:

```tsx
  it('only the active/Running lifecycle polls local every 5s', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await vi.waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    await vi.advanceTimersByTimeAsync(5000);
    expect(callsFor(runsSpy, 'local').length).toBeGreaterThanOrEqual(2);
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1); // remotes wait for the 60s cadence
    vi.useRealTimers();
  });
```

Add the imports `import { ApiError } from '../../lib/api';` and `import { callsFor } from '../../lib/perHost/testUtils';`. If the stray-`alert` assertion doesn't match how `ErrorBanner` renders, assert instead that the banner text (`host unreachable`) is absent: `expect(screen.queryByText(/host unreachable/)).not.toBeInTheDocument()`.

Other tests in these files mock `getWorkflowRuns` with rows tagged `host_id: 'local'` for every call. Those rows dedupe by `(host_id, id)`, so they keep passing. If a test now renders a duplicate, switch its mock to `.mockImplementation(onlyHost('local', [...rows]))`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/WorkflowRuns`
Expected: the new host-filter tests FAIL (the default is still `local`).

- [ ] **Step 3: Migrate `WorkflowRuns.tsx`**

**3a.** Imports: replace `import { usePagedList } from '../../lib/usePagedList';` with:

```tsx
import { usePerHostPagedList, type PerHostFetchParams } from '../../lib/perHost/usePerHostPagedList';
import { PagingFailures, PerHostStrip, perHostFooterText } from '../../components/lists/PerHostStatus';
import { notIncluded, waitingLabel } from '../../lib/perHost/status';
```

Update the file header's "Fetch/paginate/poll is owned by `usePagedList`" paragraph to say `usePerHostPagedList` (per-host progressive loading, spec 2026-10-01), All hosts by default.

**3b.** Replace the default host state:

```tsx
  // All hosts by default: local paints at once, each remote merges in as it
  // answers (usePerHostPagedList). A picked host lists only that host.
  const [hostFilter, setHostFilter] = useState<string>(ALL_HOSTS);
```

**3c.** Replace `fetchRows` and the `usePagedList` call with:

```tsx
  const fetchRows = useCallback(
    ({ host, offset, limit }: PerHostFetchParams): Promise<RunListRow[]> => {
      if (archived) {
        // /api/runs/archived has no offset/limit — it's a single fetch.
        // Any page beyond the first returns empty so the host settles.
        return offset === 0 ? api.getArchivedRuns('workflow') : Promise.resolve([]);
      }
      return api.getWorkflowRuns({ lifecycle: tab, offset, limit, host });
    },
    [archived, tab],
  );

  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended } =
    usePerHostPagedList<RunListRow>({
      // Archived is a local-only endpoint (`/api/runs/archived`).
      host: archived ? 'local' : hostFilter === ALL_HOSTS ? null : hostFilter,
      fetch: fetchRows,
      timeField: 'started_at',
      idField: 'id',
      deps: [archived, tab],
      poll: !archived && tab === 'active',
    });
```

**3d.** Row actions. In `handleRowArchive`, `handleRowRestore` and `handleRowDelete`, replace the `refresh();` after success with:

```tsx
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
```

**3e.** Under the `FilterBar` wrapper `<div className="mb-5">…</div>`, add:

```tsx
      {!archived && <PerHostStrip slices={slices} />}
```

**3f.** Replace the loading/empty chain's head, from `{loading && rows.length === 0 ? (` through the first `EmptyState`, with:

```tsx
      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      ) : rows.length === 0 && waitingLabel(slices) ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label={waitingLabel(slices) ?? ''} />
        </div>
      ) : filtered.length === 0 ? (
        <EmptyState
          title={
            rows.length > 0
              ? 'No runs match this filter'
              : notIncluded(slices)
                ? 'No workflow runs on the hosts that answered'
                : 'No workflow runs yet'
          }
          hint={
            rows.length > 0
              ? 'Try selecting a different trigger or host filter above.'
              : notIncluded(slices)
                ? `Not included: ${notIncluded(slices)}.`
                : 'Workflow runs will appear here once you dispatch one from the CLI, the desktop app, or a scheduled trigger.'
          }
        />
```

**3g.** Make `SortableTable`'s `rowKey` host-qualified: `rowKey={(r) => `${r.host_id ?? 'local'}:${r.id}`}`.

**3h.** Replace the sentinel block. Change its mount condition from `{!archived && (loading || hasMore || ended) && (` to `{!archived && (`, set its text to the code below, and render `PagingFailures` after it:

```tsx
          {!archived && (
            <>
              <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
                {q
                  ? `${visible.length} matches of ${filtered.length} loaded`
                  : perHostFooterText({ slices, loading, hasMore, ended, count: filtered.length })}
              </div>
              <PagingFailures slices={slices} onRetry={retryPaging} />
            </>
          )}
```

- [ ] **Step 4: Run the WorkflowRuns tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/WorkflowRuns && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

For any remaining failure, read its assertion. If it encoded the old local default or a single call count, update it to the per-host contract (by `callsFor`). If it caught a real regression, fix the page.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/runs/
git commit -m "$(cat <<'EOF'
feat(cp-web): Workflow runs load per host, All hosts by default

Local rows paint at once; each remote merges in as it answers with its
own loading/offline chip; the footer names hosts not included; row
actions drop the row and re-sync only its host.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: AgentRuns — All hosts by default, per-host loading

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/runs/AgentRuns.tsx`
- Test: `AgentRuns.test.tsx`, `AgentRuns.actions.test.tsx`, `AgentRuns.columnOrder.test.tsx`

**Interfaces:**
- Consumes: same as Task 9.

- [ ] **Step 1: Re-stub the tests and rewrite the host-filter tests.** In all three AgentRuns test files, replace `vi.spyOn(api, 'getHosts')` with `vi.spyOn(api, 'getRegisteredHosts')`.

In `AgentRuns.test.tsx`, replace the first four tests of `describe('AgentRuns host filter — server-driven', …)` with the tests below. Leave its `'Host column renders host_id from the row'` test as is.

```tsx
  it('defaults to All hosts and fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
    expect(screen.getByLabelText('Host filter')).toHaveValue('__all__');
  });

  it('renders This host, All hosts, then registered (non-local) hosts', async () => {
    stubDeps();
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('This host fetches only local', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    runsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(0);
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });
    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });
```

Add `import { callsFor } from '../../lib/perHost/testUtils';`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/AgentRuns`
Expected: the new default test FAILs.

- [ ] **Step 3: Migrate `AgentRuns.tsx`**

**3a.** Imports: replace `import { usePagedList } from '../../lib/usePagedList';` with the three per-host imports from Task 9 step 3a. Change `import { useState } from 'react';` (or whatever React import exists) so it also imports `useCallback`.

**3b.** Default host: `const [hostFilter, setHostFilter] = useState<string>(ALL_HOSTS);` with the comment from Task 9 step 3b.

**3c.** Replace the `usePagedList` call with:

```tsx
  const fetchRows = useCallback(
    ({ host, offset, limit }: PerHostFetchParams) => api.getAgentRuns({ lifecycle: tab, offset, limit, host }),
    [tab],
  );
  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended } =
    usePerHostPagedList<AgentRunRow>({
      host: hostFilter === ALL_HOSTS ? null : hostFilter,
      fetch: fetchRows,
      timeField: 'started_at',
      idField: 'run_id',
      deps: [tab],
      poll: tab === 'active',
    });
```

**3d.** Row actions. Find each `refresh();` that follows a successful action call: the 5 handlers `handleSessionArchive`, `handleSessionRestore`, `handleSessionDelete`, `handleStandaloneArchive` and `handleStandaloneDelete`, including the post-confirm retry branches.
- **Standalone handlers** act on one row: replace with `removeRow(host ?? 'local', runId); refreshHost(host ?? 'local');`, using the handler's own id and host parameter names.
- **Session handlers** act on a whole session, which may be several rows: replace with `refreshHost(host ?? 'local');`.
- Leave the header's Refresh button calling `refresh()`.

**3e.** After the `<FilterBar … />` element, add `<PerHostStrip slices={slices} />` inside a `<div className="mt-3">…</div>`.

**3f.** Replace the loading/empty head with:

```tsx
        {loading && rows.length === 0 ? (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading agent runs…" />
          </div>
        ) : rows.length === 0 && waitingLabel(slices) ? (
          <div className="py-16 flex items-center justify-center">
            <Spinner label={waitingLabel(slices) ?? ''} />
          </div>
        ) : sorted.length === 0 ? (
          <EmptyState
            title={
              rows.length > 0
                ? 'No agent runs match this filter'
                : notIncluded(slices)
                  ? 'No agent runs on the hosts that answered'
                  : 'No agent runs yet'
            }
            hint={
              rows.length > 0
                ? 'Try a different lifecycle, source, or host filter above.'
                : notIncluded(slices)
                  ? `Not included: ${notIncluded(slices)}.`
                  : 'Standalone and session-bound agent invocations will appear here once they run.'
            }
          />
```

**3g.** `rowKey={(r) => `${r.host_id ?? 'local'}:${r.run_id}`}`.

**3h.** Sentinel text:

```tsx
              {q
                ? `${visible.length} matches of ${sorted.length} loaded`
                : perHostFooterText({ slices, loading, hasMore, ended, count: sorted.length })}
```

Then add `<PagingFailures slices={slices} onRetry={retryPaging} />` directly after the sentinel `div`.

- [ ] **Step 4: Run the AgentRuns tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/AgentRuns && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`. Triage any remaining failure the same way as Task 9 step 4.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/runs/AgentRuns*
git commit -m "$(cat <<'EOF'
feat(cp-web): Agent runs load per host, All hosts by default

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: AutoflowRuns (cycles + events) — All hosts by default, per-host loading

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`
- Test: `AutoflowRuns.test.tsx`, `AutoflowRuns.actions.test.tsx`, `AutoflowRuns.columnOrder.test.tsx`

**Interfaces:**
- Consumes: same as Task 9. Claims keeps `usePagedList`; its endpoint is local-only.

- [ ] **Step 1: Re-stub the tests and rewrite the host-filter tests.** In all three AutoflowRuns test files, replace `vi.spyOn(api, 'getHosts')` with `vi.spyOn(api, 'getRegisteredHosts')`.

In `AutoflowRuns.test.tsx`, inside `describe('AutoflowRuns host filter — server-driven (runs + cycles tabs)', …)`:
- Replace `'default fetch passes host: "local" to both events and runs'` with the first test below.
- Replace `'"All hosts" option fetches without a host param'` with the second.

```tsx
  it('defaults to All hosts: events and cycles are fetched per registered host', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    const eventsSpy = vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    const runsSpy = vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(eventsSpy, 'local')).toHaveLength(1));
    expect(callsFor(eventsSpy, 'host_prod')).toHaveLength(1);
    expect(callsFor(runsSpy, 'local')).toHaveLength(1);
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
  });

  it('This host fetches only local', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    const eventsSpy = vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());
    eventsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(eventsSpy, 'local')).toHaveLength(1));
    expect(callsFor(eventsSpy, 'host_prod')).toHaveLength(0);
  });
```

Add `import { callsFor } from '../../lib/perHost/testUtils';`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/AutoflowRuns`
Expected: the new default test FAILs.

- [ ] **Step 3: Migrate `AutoflowRuns.tsx`**

**3a.** Imports: keep `import { usePagedList } from '../../lib/usePagedList';` (Claims still uses it). Add the three per-host imports from Task 9 step 3a, and make sure `useCallback` is imported from React.

**3b.** `const [hostFilter, setHostFilter] = useState<string>(ALL_HOSTS);` with the Task 9 step 3b comment.

**3c.** Replace the `events` and `cycles` hooks. Update the comment above them to say they are per-host now: local polls every 5 s, remotes every 60 s while visible, and both share one SSH `autoflow history` listing via the server's listing cache.

```tsx
  const scopeHost = hostFilter === ALL_HOSTS ? null : hostFilter;
  const fetchEvents = useCallback(
    ({ host, offset, limit }: PerHostFetchParams) => api.getAutoflowEvents({ offset, limit, host }),
    [],
  );
  const fetchCycles = useCallback(
    ({ host, offset, limit }: PerHostFetchParams) => api.getAutoflowRuns({ offset, limit, host }),
    [],
  );
  const events = usePerHostPagedList<AutoflowEventRow>({
    host: scopeHost,
    fetch: fetchEvents,
    timeField: 'at',
    idField: 'event_id',
    deps: [],
    poll: true,
  });
  const cycles = usePerHostPagedList<AutoflowCycleRow>({
    host: scopeHost,
    fetch: fetchCycles,
    timeField: 'started_at',
    idField: 'cycle_id',
    deps: [],
    poll: true,
  });
```

In the `claims` `usePagedList` call, delete the `poll: false,` line (Task 13 removes the option).

**3d.** Event-row actions (`handleEventRunArchive` and the delete handler). Replace `events.refresh();` after success with `events.refreshHost(host ?? 'local');`. These are run actions on a launched run: the event row itself stays in the history, so do not `removeRow`.

**3e.** Under the FilterBar, on the runs and cycles tabs only (never claims):

```tsx
      {tab === 'runs' && <PerHostStrip slices={events.slices} />}
      {tab === 'cycles' && <PerHostStrip slices={cycles.slices} />}
```

**3f.** Events branch. Change its empty chain head:
- `events.loading && events.rows.length === 0 ? (…Spinner "Loading autoflow activity…"…)` stays as is.
- Insert a new branch after it: `: events.rows.length === 0 && waitingLabel(events.slices) ? (<div className="py-16 flex items-center justify-center"><Spinner label={waitingLabel(events.slices) ?? ''} /></div>)`.
- In the existing `events.rows.length === 0` `EmptyState`, append `notIncluded(events.slices) ? ` Not included: ${notIncluded(events.slices)}.` : ''` to its hint.

Its sentinel text becomes:

```tsx
                {q
                  ? `${visibleEvents.length} matches of ${events.rows.length} loaded`
                  : perHostFooterText({ slices: events.slices, loading: events.loading, hasMore: events.hasMore, ended: events.ended, count: events.rows.length })}
```

followed by `<PagingFailures slices={events.slices} onRetry={events.retryPaging} />`. Its row key becomes `rowKey={(e) => `${e.host_id ?? 'local'}:${e.event_id}`}`.

**3g.** Cycles branch: make exactly the 3f changes with `cycles` in place of `events` and `visibleCycles` in place of `visibleEvents`:
- the same waiting branch and EmptyState hint suffix, using `cycles.slices`
- the same sentinel text using `cycles.*`, then `<PagingFailures slices={cycles.slices} onRetry={cycles.retryPaging} />`
- `rowKey={(c) => `${c.host_id ?? 'local'}:${c.cycle_id}`}`

**3h.** The header Refresh button keeps calling `events.refresh(); cycles.refresh();`.

- [ ] **Step 4: Run the AutoflowRuns tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/runs/AutoflowRuns && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`. Triage any remaining failure the same way as Task 9 step 4.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/runs/AutoflowRuns*
git commit -m "$(cat <<'EOF'
feat(cp-web): Autoflow cycles + events load per host, All hosts by default

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Sessions — All hosts by default, per-host loading

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Sessions.tsx`
- Test: `Sessions.test.tsx`, `Sessions.archive.test.tsx`

**Interfaces:**
- Consumes: same as Task 9.

- [ ] **Step 1: Re-stub the tests and rewrite the host-filter tests.** In both Sessions test files, replace `vi.spyOn(api, 'getHosts')` with `vi.spyOn(api, 'getRegisteredHosts')`.

In `Sessions.test.tsx`, inside `describe('Sessions host filter — server-driven', …)`:
- Replace `'default fetch is called with host: "local" (fast path, not fan-out)'` with the first test below.
- Replace the `'__all__'` (All hosts) test with the second.

```tsx
  it('defaults to All hosts and fetches every registered host with its own host param', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(sessionsSpy, 'local')).toHaveLength(1));
    expect(callsFor(sessionsSpy, 'host_prod')).toHaveLength(1);
    expect(screen.getByLabelText('Host filter')).toHaveValue('__all__');
  });

  it('This host fetches only local', async () => {
    stubDeps();
    const sessionsSpy = vi.spyOn(api, 'getSessions').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    sessionsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(sessionsSpy, 'local')).toHaveLength(1));
    expect(callsFor(sessionsSpy, 'host_prod')).toHaveLength(0);
  });
```

Add `import { callsFor } from '../lib/perHost/testUtils';`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Sessions`
Expected: the new default test FAILs.

- [ ] **Step 3: Migrate `Sessions.tsx`**

**3a.** Imports: replace the `usePagedList` import with:

```tsx
import { usePerHostPagedList, type PerHostFetchParams } from '../lib/perHost/usePerHostPagedList';
import { PagingFailures, PerHostStrip, perHostFooterText } from '../components/lists/PerHostStatus';
import { notIncluded, waitingLabel } from '../lib/perHost/status';
```

Make sure `useCallback` is imported from React.

**3b.** `const [hostFilter, setHostFilter] = useState<string>(ALL_HOSTS);` with the Task 9 step 3b comment.

**3c.** Replace the hook call with:

```tsx
  const fetchRows = useCallback(
    ({ host, offset, limit }: PerHostFetchParams) => api.getSessions({ scope: tab, offset, limit, host }),
    [tab],
  );
  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended } =
    usePerHostPagedList<SessionSummary>({
      host: hostFilter === ALL_HOSTS ? null : hostFilter,
      fetch: fetchRows,
      timeField: 'updated_at',
      idField: 'session_id',
      deps: [tab],
      poll: tab === 'active',
    });
```

**3d.** Row actions (archive/restore/delete). Replace each `refresh();` after success with:

```tsx
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
```

Use the handler's own parameter names for the session id and host.

**3e.** Add `<PerHostStrip slices={slices} />` directly under the `FilterBar`.

**3f.** Empty chain:
- Insert the waiting branch after the `loading && rows.length === 0` spinner: `: rows.length === 0 && waitingLabel(slices) ? (<div className="py-16 flex items-center justify-center"><Spinner label={waitingLabel(slices) ?? ''} /></div>)`.
- In the no-sessions `EmptyState`, use the title `notIncluded(slices) ? 'No sessions on the hosts that answered' : <existing title>`, and append `notIncluded(slices) ? ` Not included: ${notIncluded(slices)}.` : ''` to its hint.

**3g.** `rowKey={(s) => `${s.host_id ?? 'local'}:${s.session_id}`}`.

**3h.** Sentinel text: `{q ? `${visible.length} matches of ${rows.length} loaded` : perHostFooterText({ slices, loading, hasMore, ended, count: rows.length })}`. Keep the existing `q` copy if it differs, then add `<PagingFailures slices={slices} onRetry={retryPaging} />` after the sentinel `div`. Change the sentinel's mount condition, if it has one, to always render.

- [ ] **Step 4: Run the Sessions tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Sessions && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Sessions*
git commit -m "$(cat <<'EOF'
feat(cp-web): Sessions load per host, All hosts by default

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Remove `usePagedList`'s poll and its buggy splice

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/usePagedList.ts`
- Test: `crates/rupu-cp/web/src/lib/usePagedList.test.tsx`

**Interfaces:**
- Produces: `usePagedList({ fetch, deps })`, with the `poll` option removed. The remaining callers (ProjectRunsTab, ProjectSessionsTab, AutoflowRuns Claims) never polled.

- [ ] **Step 1: Confirm no caller passes `poll` any more**

Run: `cd crates/rupu-cp/web && grep -rn "usePagedList" src --include=*.tsx --include=*.ts | grep -v test`, then check each hit for `poll`.
Expected: no `poll:` remains (Task 11 removed Claims' `poll: false`). If any remains, delete that line.

- [ ] **Step 2: Remove the poll tests.** In `usePagedList.test.tsx`, delete the two poll tests (`'polls page 0 every 5s and splices it back in when poll is true'` and `'never polls when poll is false (the default)'`) and the `poll` prop of `PollHarness`. Delete `PollHarness` too if it has no remaining users.

- [ ] **Step 3: Edit `usePagedList.ts`**
  - Delete the `poll?: boolean` option and its doc.
  - Delete the poll `useEffect` (the `if (!poll) return; … setInterval … 5000` block) and `poll = false` in the destructuring.
  - Update the header comment to read: `// usePagedList — the shared fetch/paginate state machine for single-source lists (ProjectRunsTab, ProjectSessionsTab, autoflow Claims). Page size 20, infinite-scroll sentinel via useInfiniteScroll, and an \`ended\` flag for the "— end of N —" footer. Polling, multi-host lists live in lib/perHost/usePerHostPagedList.ts; its poll splice was removed with them (spec 2026-10-01 §6.3: it dropped a row when a run arrived and duplicated one when a run left).`

- [ ] **Step 4: Run the tests and typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/usePagedList.test.tsx src/components/project && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/usePagedList.ts crates/rupu-cp/web/src/lib/usePagedList.test.tsx crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx
git commit -m "$(cat <<'EOF'
refactor(cp-web): drop usePagedList's poll — no polling caller remains

Its page-0 splice lost the boundary row when a run arrived and
duplicated a row when one left; the polling lists now use
usePerHostPagedList's spliceHead.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: `mergeUsage` (pure)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/usage/mergeUsage.ts`
- Test: `crates/rupu-cp/web/src/lib/usage/mergeUsage.test.ts`

**Interfaces:**
- Produces:
  - `rollupSummaries(list: UsageSummary[]): UsageSummary`, a port of Rust `usage::rollup`
  - `mergeUnpriced(gaps: UnpricedGap[]): UnpricedGap`, a port of `merge_unpriced`
  - `interface MergedUsage { summary: UsageSummary; unpriced: UnpricedGap }`
  - `mergeUsage(responses: Pick<UsageResponse, 'summary' | 'unpriced'>[]): MergedUsage`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it } from 'vitest';
import type { UsageSummary } from '../usage';
import { mergeUnpriced, mergeUsage, rollupSummaries } from './mergeUsage';

const sum = (over: Partial<UsageSummary> = {}): UsageSummary => ({
  input_tokens: 10, output_tokens: 5, cached_tokens: 2, cache_write_tokens: 1, total_tokens: 15,
  cost_usd: 1, priced: true, runs: 1, partial: false, ...over,
});

describe('rollupSummaries (port of usage::rollup)', () => {
  it('sums tokens and runs; total = input + output', () => {
    const r = rollupSummaries([sum(), sum({ input_tokens: 20, output_tokens: 10, runs: 3 })]);
    expect(r).toMatchObject({ input_tokens: 30, output_tokens: 15, cached_tokens: 4, cache_write_tokens: 2, total_tokens: 45, runs: 4 });
  });
  it('cost is null unless some host priced; priced ANDs; partial ORs', () => {
    expect(rollupSummaries([sum({ cost_usd: null }), sum({ cost_usd: null })]).cost_usd).toBeNull();
    expect(rollupSummaries([sum({ cost_usd: null }), sum({ cost_usd: 2.5 })]).cost_usd).toBe(2.5);
    expect(rollupSummaries([sum(), sum({ priced: false })]).priced).toBe(false);
    expect(rollupSummaries([sum(), sum({ partial: true })]).partial).toBe(true);
  });
  it('an empty rollup is priced, unpartial and costless (Rust parity)', () => {
    expect(rollupSummaries([])).toMatchObject({ priced: true, partial: false, cost_usd: null, runs: 0 });
  });
});

describe('mergeUnpriced (port of merge_unpriced)', () => {
  it('unions models (sorted, distinct) and sums rows', () => {
    expect(mergeUnpriced([{ models: ['b', 'a'], rows: 2 }, { models: ['a', 'c'], rows: 3 }])).toEqual({
      models: ['a', 'b', 'c'],
      rows: 5,
    });
  });
});

describe('mergeUsage', () => {
  it('merges summaries and gaps across hosts', () => {
    const m = mergeUsage([
      { summary: sum(), unpriced: { models: ['x'], rows: 1 } },
      { summary: sum(), unpriced: { models: [], rows: 0 } },
    ]);
    expect(m.summary.runs).toBe(2);
    expect(m.unpriced).toEqual({ models: ['x'], rows: 1 });
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/usage/mergeUsage.test.ts`
Expected: FAIL, module not found.

- [ ] **Step 3: Write `mergeUsage.ts`**

```ts
// Client-side merge of per-host `/api/usage` answers (spec 2026-10-01 §6.5).
// Ports of `rollup` (rupu-cp/src/usage.rs) and `merge_unpriced`
// (rupu-cp/src/api/usage.rs) — keep the two in step.

import type { UsageResponse } from '../api';
import type { UnpricedGap, UsageSummary } from '../usage';

export function rollupSummaries(list: readonly UsageSummary[]): UsageSummary {
  let input = 0;
  let output = 0;
  let cached = 0;
  let cacheWrite = 0;
  let runs = 0;
  let anyCost = false;
  let cost = 0;
  let priced = true;
  let partial = false;
  for (const s of list) {
    input += s.input_tokens;
    output += s.output_tokens;
    cached += s.cached_tokens;
    cacheWrite += s.cache_write_tokens ?? 0;
    runs += s.runs;
    if (s.cost_usd != null) {
      anyCost = true;
      cost += s.cost_usd;
    }
    if (!s.priced) priced = false;
    if (s.partial) partial = true;
  }
  return {
    input_tokens: input,
    output_tokens: output,
    cached_tokens: cached,
    cache_write_tokens: cacheWrite,
    total_tokens: input + output,
    cost_usd: anyCost ? cost : null,
    priced,
    runs,
    partial,
  };
}

export function mergeUnpriced(gaps: readonly UnpricedGap[]): UnpricedGap {
  const models = new Set<string>();
  let rows = 0;
  for (const g of gaps) {
    for (const m of g.models) models.add(m);
    rows += g.rows;
  }
  return { models: [...models].sort(), rows };
}

export interface MergedUsage {
  summary: UsageSummary;
  unpriced: UnpricedGap;
}

export function mergeUsage(responses: readonly Pick<UsageResponse, 'summary' | 'unpriced'>[]): MergedUsage {
  return {
    summary: rollupSummaries(responses.map((r) => r.summary)),
    unpriced: mergeUnpriced(responses.map((r) => r.unpriced)),
  };
}
```

- [ ] **Step 4: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/usage/mergeUsage.test.ts`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/usage/mergeUsage.ts crates/rupu-cp/web/src/lib/usage/mergeUsage.test.ts
git commit -m "$(cat <<'EOF'
feat(cp-web): mergeUsage — client port of usage rollup + unpriced merge

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 15: `useUsageData` — per-host `/api/usage`

**Files:**
- Create: `crates/rupu-cp/web/src/lib/usage/useUsageData.ts`
- Test: `crates/rupu-cp/web/src/lib/usage/useUsageData.test.ts`

**Interfaces:**
- Consumes: `mergeUsage` (Task 14), `classifyFailure` (Task 5), `api.getUsage(win, pivot, host)`, `api.getRegisteredHosts`.
- Produces:
  - `USAGE_REMOTE_POLL_MS = 60_000`
  - `useUsageData(usageWindow: UsageWindow, windowKey: string, windowSource: 'user' | 'tick')`, returning `{ data: (MergedUsage & { excluded: string[] }) | null; hosts: HostFreshnessEntry[]; error: Error | null }`
  - `windowKey` contract: a preset is `preset:<range>`; a custom window is `<since>|<until>`. Only hosts whose last answer matches the current key count toward the merge.

- [ ] **Step 1: Write the failing tests**

```ts
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, type UsageResponse, type UsageWindow } from '../api';
import { USAGE_REMOTE_POLL_MS, useUsageData } from './useUsageData';
import { REG_LOCAL, REG_PROD, callsFor } from '../perHost/testUtils';

const WIN: UsageWindow = { since: '2026-09-01T00:00:00.000Z', until: '2026-09-30T00:00:00.000Z' };

function resp(hostId: string, runs: number, state: 'ok' | 'offline' | 'unavailable' = 'ok'): UsageResponse {
  return {
    summary: { input_tokens: runs, output_tokens: 0, cached_tokens: 0, total_tokens: runs, cost_usd: runs, priced: true, runs },
    breakdown: [],
    unpriced: { models: [], rows: 0 },
    hosts: [{ host_id: hostId, name: hostId, transport_kind: 'ssh', state, captured_at: state === 'ok' ? '2026-09-30T00:00:00Z' : null, reason: state === 'ok' ? null : 'down' }],
  };
}
/** getUsage's 3rd arg is the host — adapt callsFor's object-param shape. */
const usageCallsFor = (host: string) =>
  callsFor({ mock: { calls: vi.mocked(api.getUsage).mock.calls.map((c) => [{ host: c[2] }]) } }, host);

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('useUsageData', () => {
  it('shows local as soon as it answers, while a remote is still loading', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      host === 'local' ? Promise.resolve(resp('local', 3)) : new Promise(() => {}),
    );
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(3));
    expect(result.current.data?.excluded).toEqual(['prod (loading)']);
    expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);
  });

  it("treats each response's own host entry as authoritative", async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      Promise.resolve(host === 'local' ? resp('local', 3) : resp('host_prod', 99, 'offline')),
    );
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.hosts[1]?.state).toBe('offline'));
    expect(result.current.data?.summary.runs).toBe(3);
    expect(result.current.data?.excluded).toEqual(['prod (offline)']);
  });

  it('pins group_by to model and passes the host', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const spy = vi.spyOn(api, 'getUsage').mockResolvedValue(resp('local', 1));
    renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(spy).toHaveBeenCalledWith(WIN, 'model', 'local'));
  });

  it('a tick refetches local only; remotes keep their 60s cadence', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => Promise.resolve(resp(host ?? 'local', 1)));
    const { rerender } = renderHook(({ w, src }) => useUsageData(w, 'preset:30d', src), {
      initialProps: { w: WIN, src: 'user' as 'user' | 'tick' },
    });
    await waitFor(() => expect(usageCallsFor('host_prod')).toHaveLength(1));
    rerender({ w: { ...WIN, until: '2026-09-30T00:00:30.000Z' }, src: 'tick' });
    await waitFor(() => expect(usageCallsFor('local')).toHaveLength(2));
    expect(usageCallsFor('host_prod')).toHaveLength(1);
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    await act(() => vi.advanceTimersByTimeAsync(USAGE_REMOTE_POLL_MS));
    await waitFor(() => expect(usageCallsFor('host_prod')).toHaveLength(2));
  });

  it('a user window change refetches every host and excludes answers for the old window', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    let remoteHangs = false;
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      host !== 'local' && remoteHangs ? new Promise(() => {}) : Promise.resolve(resp(host ?? 'local', host === 'local' ? 1 : 10)),
    );
    const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
      initialProps: { key: 'preset:30d' },
    });
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));
    remoteHangs = true;
    rerender({ key: 'preset:7d' });
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(1));
    expect(result.current.data?.excluded).toEqual(['prod (loading)']);
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/usage/useUsageData.test.ts`
Expected: FAIL, module not found.

- [ ] **Step 3: Write `useUsageData.ts`**

```ts
// useUsageData — the /usage headline, loaded PER HOST (spec 2026-10-01 §6.5).
//
// Same shape as useDashboardData: `/api/hosts/registered` seeds a
// `loading` entry per host, then each host's `/api/usage?host=<id>` fires
// independently. The headline is the merge (mergeUsage) over hosts whose
// last good answer is for the CURRENT window (`windowKey`), so a slow
// host's figure for an old window never mixes into a new one. Each
// response's own `hosts[0]` entry is authoritative: a 200 for a down host is
// not proof of health.
//
// `group_by` is pinned to `model` (the server default; every transport,
// SSH included, can answer it). The page never reads `breakdown`, so a pivot
// change no longer refetches, and SSH hosts stay in the headline under
// every pivot.
//
// Cadence: local follows the page's 30 s preset tick; remotes refetch
// every 60 s while visible, on focus, and immediately on a user window
// change.

import { useEffect, useMemo, useRef, useState } from 'react';
import { api, apiErrorMessage, type UsageResponse, type UsageWindow } from '../api';
import type { HostFreshnessEntry } from '../../components/dashboard/HostFreshnessStrip';
import type { HostSeed } from '../perHost/types';
import { classifyFailure } from '../perHost/status';
import { mergeUsage, type MergedUsage } from './mergeUsage';

export const USAGE_REMOTE_POLL_MS = 60_000;

interface HostUsage {
  hostId: string;
  name: string;
  transportKind: string;
  state: 'loading' | 'ok' | 'offline' | 'unavailable';
  response: UsageResponse | null;
  /** The window key `response` was fetched for. */
  windowKey: string | null;
  reason: string | null;
  receivedAt: number | null;
}

export interface UseUsageDataResult {
  data: (MergedUsage & { excluded: string[] }) | null;
  hosts: HostFreshnessEntry[];
  error: Error | null;
}

const seedOf = (h: HostSeed): HostUsage => ({
  hostId: h.id,
  name: h.name,
  transportKind: h.transport_kind,
  state: 'loading',
  response: null,
  windowKey: null,
  reason: null,
  receivedAt: null,
});

export function useUsageData(usageWindow: UsageWindow, windowKey: string, windowSource: 'user' | 'tick'): UseUsageDataResult {
  const [hosts, setHosts] = useState<HostUsage[]>([]);
  const [listError, setListError] = useState<Error | null>(null);
  const windowRef = useRef(usageWindow);
  windowRef.current = usageWindow;
  const keyRef = useRef(windowKey);
  keyRef.current = windowKey;
  const hostIdsRef = useRef<string[]>([]);
  const statesRef = useRef<HostUsage[]>([]);
  statesRef.current = hosts;
  /** Latest request id per host: an older, slower answer never overwrites a newer one. */
  const seqRef = useRef(new Map<string, number>());

  const fetchHost = useRef((hostId: string) => {
    const seq = (seqRef.current.get(hostId) ?? 0) + 1;
    seqRef.current.set(hostId, seq);
    const key = keyRef.current;
    const update = (f: (h: HostUsage) => HostUsage) =>
      setHosts((prev) => prev.map((h) => (h.hostId === hostId ? f(h) : h)));
    api.getUsage(windowRef.current, 'model', hostId).then(
      (resp) => {
        if (seqRef.current.get(hostId) !== seq) return;
        const wire = resp.hosts.find((h) => h.host_id === hostId);
        update((h) => {
          if (!wire) return { ...h, state: 'unavailable', response: null, reason: 'host missing from response' };
          if (wire.state !== 'ok') return { ...h, state: wire.state, response: null, reason: wire.reason };
          return { ...h, state: 'ok', response: resp, windowKey: key, reason: null, receivedAt: Date.now() };
        });
      },
      (e: unknown) => {
        if (seqRef.current.get(hostId) !== seq) return;
        const f = classifyFailure(e);
        update((h) =>
          h.response
            ? { ...h, reason: f.reason } // stale-on-error: keep last good
            : { ...h, state: f.kind === 'unavailable' ? 'unavailable' : 'offline', reason: f.reason },
        );
      },
    );
  }).current;

  const fetchWhere = (pred: (h: HostUsage) => boolean) => {
    for (const h of statesRef.current) if (pred(h)) fetchHost(h.hostId);
  };

  // Bootstrap once: the host list, then every host.
  useEffect(() => {
    let cancelled = false;
    api.getRegisteredHosts().then(
      (hs) => {
        if (cancelled) return;
        hostIdsRef.current = hs.map((h) => h.id);
        setHosts(hs.map(seedOf));
        for (const h of hs) fetchHost(h.id);
      },
      (e: unknown) => {
        if (cancelled) return;
        setListError(new Error(`Could not list hosts (${apiErrorMessage(e)}); showing this host only.`));
        hostIdsRef.current = ['local'];
        setHosts([seedOf({ id: 'local', name: 'Local', transport_kind: 'local' })]);
        fetchHost('local');
      },
    );
    return () => {
      cancelled = true;
    };
  }, [fetchHost]);

  // A user window change (preset button, drag-select, clear): every host, now.
  const firstKey = useRef(true);
  useEffect(() => {
    if (firstKey.current) {
      firstKey.current = false;
      return;
    }
    fetchWhere(() => true);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the window identity only
  }, [windowKey]);

  // The 30 s preset tick: local only.
  useEffect(() => {
    if (windowSource === 'tick') fetchWhere((h) => h.hostId === 'local');
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the tick-moved `until`
  }, [usageWindow.until, windowSource]);

  // Remote cadence: 60 s while visible, plus on focus (unavailable hosts wait for a reload).
  useEffect(() => {
    const remote = (h: HostUsage) => h.hostId !== 'local' && h.state !== 'unavailable';
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') fetchWhere(remote);
    }, USAGE_REMOTE_POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === 'visible') fetchWhere((h) => h.state !== 'unavailable');
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearInterval(t);
      document.removeEventListener('visibilitychange', onVisible);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- fetchWhere reads refs
  }, []);

  const current = (h: HostUsage) => h.state === 'ok' && h.response !== null && h.windowKey === windowKey;

  const data = useMemo(() => {
    const ok = hosts.filter(current);
    if (ok.length === 0) return null;
    const excluded = hosts
      .filter((h) => !current(h))
      .map((h) => `${h.name} (${h.state === 'ok' ? (h.reason ? 'stale' : 'loading') : h.state})`);
    return { ...mergeUsage(ok.map((h) => h.response as UsageResponse)), excluded };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `current` closes over windowKey
  }, [hosts, windowKey]);

  const entries: HostFreshnessEntry[] = hosts.map((h) => ({
    host_id: h.hostId,
    name: h.name,
    transport_kind: h.transportKind,
    state: h.state === 'ok' && h.windowKey !== windowKey ? 'loading' : h.state,
    captured_at: current(h) && h.receivedAt != null ? new Date(h.receivedAt).toISOString() : null,
    reason: h.reason,
  }));

  const allFailed = hosts.length > 0 && hosts.every((h) => h.state === 'offline' || h.state === 'unavailable');
  const error =
    listError ?? (allFailed ? new Error(hosts.map((h) => `${h.name}: ${h.reason ?? h.state}`).join(' · ')) : null);

  return { data, hosts: entries, error };
}
```

- [ ] **Step 4: Run the tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/usage/useUsageData.test.ts`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/usage/useUsageData.ts crates/rupu-cp/web/src/lib/usage/useUsageData.test.ts
git commit -m "$(cat <<'EOF'
feat(cp-web): useUsageData — /usage headline loaded per host

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 16: Usage page uses `useUsageData`

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Usage.tsx`
- Modify: `crates/rupu-cp/web/src/components/usage/UsageTimeline.tsx`, `crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.tsx`, `crates/rupu-cp/web/src/components/dashboard/ModelBreakdownTable.tsx`. Their `hosts` prop is only used for id→name mapping.
- Test: `Usage.test.tsx`, `Usage.refresh.test.tsx`, `Usage.selectRange.test.tsx`

**Interfaces:**
- Consumes: `useUsageData` (Task 15).
- Produces: `hosts?: { host_id: string; name: string }[]` on those three components (was `HostFreshness[]`).

- [ ] **Step 1: Widen the three `hosts` props**
  - In `UsageTimeline.tsx`, `UsageTimelineStacked.tsx` and `ModelBreakdownTable.tsx`, change `hosts?: HostFreshness[];` to `hosts?: { host_id: string; name: string }[];`.
  - Update each doc comment from "`data.hosts` from `/api/usage`" to "per-host name refs (from `useUsageData`)".
  - Remove a now-unused `HostFreshness` import.

- [ ] **Step 2: Update the Usage tests**

In all three Usage test files:
- Add `vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);` next to the existing `getUsage` spy setup (import `REG_LOCAL` from `../lib/perHost/testUtils`).
- Give the `usageResponse()` fixture a local host entry: `hosts: [{ host_id: 'local', name: 'Local', transport_kind: 'local', state: 'ok', captured_at: '2026-09-30T00:00:00Z', reason: null }]`.
- Replace every `toHaveBeenCalledWith(<win>, 'model')` and `toHaveBeenLastCalledWith(<win>, 'model')` with the same window plus a third argument `'local'`.

In `Usage.refresh.test.tsx`, the 30 s tick still refetches local `getUsage`, so its assertions on the local call count hold. If a test asserted that a **pivot** change refetches `getUsage`, invert it: a pivot change must not refetch.

Add one test to `Usage.test.tsx`:

```tsx
  it('a pivot change does not refetch the headline', async () => {
    stubAll();
    render(<Usage />);
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: /provider/i }));
    await new Promise((r) => setTimeout(r, 20));
    expect(api.getUsage).toHaveBeenCalledTimes(1);
  });
```

Use whatever setup helper the file already has in place of `stubAll()` (e.g. its `renderPage`/`stub` function), and the PivotPicker's actual accessible name for the provider option.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Usage`
Expected: FAIL. The page still calls `getUsage(win, pivot)` without a host.

- [ ] **Step 4: Migrate `Usage.tsx`**
  - Import `import { useUsageData } from '../lib/usage/useUsageData';`. Drop `type UsageResponse` from the api import if it becomes unused.
  - Delete `const [data, setData] = useState<UsageResponse | null>(null);`, `const [error, setError] = useState<Error | null>(null);` and the whole `/api/usage` `useEffect` (the one calling `api.getUsage(usageWindow, pivot)`), including its comment.
  - Add, after `windowSource` is declared:

```tsx
  // The fleet headline, loaded per host (spec 2026-10-01 §6.5). A preset's
  // identity is the preset (its `until` ticks every 30s without changing what
  // the operator is looking at); a custom window's is its exact bounds.
  const windowKey = isCustomWindow ? `${usageWindow.since}|${usageWindow.until}` : `preset:${range}`;
  const { data, hosts, error } = useUsageData(usageWindow, windowKey, windowSource);
```

  - Header strip: replace `{data && (<div className="mt-1"><HostFreshnessStrip hosts={data.hosts} /></div>)}` with `{hosts.length > 0 && (<div className="mt-1"><HostFreshnessStrip hosts={hosts} /></div>)}`.
  - Replace `hosts={data.hosts}` with `hosts={hosts}` on both `UsageTimeline` and `ModelBreakdownTable`.
  - Headline `subLabel`: append the excluded hosts:

```tsx
              subLabel: `${formatTokens(data.summary.total_tokens)} tokens · ${data.summary.runs} runs${
                !data.summary.priced ? ' · partial (see banner above)' : ''
              }${data.excluded.length ? ` · excludes ${data.excluded.join(', ')}` : ''}`,
```

  - In the file header, update the comment that says the headline / `UnpricedBanner` / `HostFreshnessStrip` come from `getUsage` "fleet-wide": they now come from `useUsageData`, loaded per host.

- [ ] **Step 5: Run the tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Usage src/components/usage src/components/dashboard && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Usage* crates/rupu-cp/web/src/components/usage/UsageTimeline.tsx crates/rupu-cp/web/src/components/dashboard/UsageTimelineStacked.tsx crates/rupu-cp/web/src/components/dashboard/ModelBreakdownTable.tsx
git commit -m "$(cat <<'EOF'
feat(cp-web): Usage headline loads per host; pivot no longer refetches

The page waited for every host's `rupu usage` over SSH, again on every
pivot click — for a breakdown it never read. Under pivot=project SSH
hosts also fell out of the headline. Now each host answers on its own,
the headline names hosts it excludes, and a pivot switch costs nothing.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 17: ⌘K palette — per-host runs; remote runs link with `?host=`

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/paletteSources.ts`
- Modify: `crates/rupu-cp/web/src/components/CommandPalette.tsx`
- Test: `crates/rupu-cp/web/src/lib/paletteSources.test.ts`, `crates/rupu-cp/web/src/components/CommandPalette.v2.test.tsx`

**Interfaces:**
- Produces: `runItems(rows)` links remote runs as `/runs/<id>?host=<host_id>`. The palette's runs merge in per host.

- [ ] **Step 1: Write the failing `runItems` test.** Add it to `paletteSources.test.ts`:

```ts
  it('runItems links a remote run with its host (and keeps the id unique per host)', () => {
    const base = rows[0];
    const [remote] = runItems([{ ...base, id: 'run_r', host_id: 'host_prod' }]);
    expect(remote.to).toBe('/runs/run_r?host=host_prod');
    const [local] = runItems([{ ...base, id: 'run_l', host_id: 'local' }]);
    expect(local.to).toBe('/runs/run_l');
  });
```

Use the existing `rows` fixture name from the `'runItems → /runs/:id'` test. If it is local to that test, build a `RunListRow` the same way.

- [ ] **Step 2: Run the test to see it fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/paletteSources.test.ts`
Expected: FAIL. `to` is `/runs/run_r`.

- [ ] **Step 3: Fix `runItems`**

```ts
export function runItems(rows: RunListRow[]): PaletteItem[] {
  return rows.map((r) => {
    const remote = r.host_id && r.host_id !== 'local' ? r.host_id : null;
    return {
      kind: 'run' as const,
      id: remote ? `${remote}:${r.id}` : r.id,
      title: r.workflow_name,
      subtitle: `${shortId(r.id)} · ${r.status}${remote ? ` · ${remote}` : ''}`,
      // A remote run's detail must name its host, or the CP falls back to
      // probing every host to find it.
      to: remote ? `/runs/${encodeURIComponent(r.id)}?host=${encodeURIComponent(remote)}` : `/runs/${r.id}`,
      keywords: r.id,
    };
  });
}
```

- [ ] **Step 4: Make the palette's runs source per-host.** In `CommandPalette.tsx`:
  - Add state `const [remoteRunItems, setRemoteRunItems] = useState<PaletteItem[]>([]);`.
  - In the on-open effect, reset it with `setRemoteRunItems([]);`.
  - In the `Promise.all` array, replace `api.getRuns({ limit: 200 }).then(runItems).catch(() => []),` with:

```tsx
      // Runs load per host (spec 2026-10-01 §6.6): local is part of the
      // initial batch; each remote's runs append when that host answers, so
      // a slow or dead host never holds the palette in a loading state.
      api.getRegisteredHosts()
        .catch(() => [{ id: 'local', name: 'Local', transport_kind: 'local' as const }])
        .then((hosts) => {
          for (const h of hosts) {
            if (h.id === 'local') continue;
            api
              .getRuns({ host: h.id, limit: 200 })
              .then(runItems)
              .then((items) => {
                if (!cancelled) setRemoteRunItems((prev) => [...prev, ...items]);
              })
              .catch(() => {});
          }
          return api.getRuns({ host: 'local', limit: 200 }).then(runItems).catch(() => []);
        }),
```

  - Change the `groups` memo's input from `items` to `[...items, ...remoteRunItems]`, adding `remoteRunItems` to its dependency array.

- [ ] **Step 5: Add a palette test.** Add it to `CommandPalette.v2.test.tsx`, using its existing open/render helpers:

```tsx
  it('lists local runs without waiting on a hung remote host', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([
      { id: 'local', name: 'Local', transport_kind: 'local' },
      { id: 'host_prod', name: 'prod', transport_kind: 'ssh' },
    ]);
    vi.spyOn(api, 'getRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([{ ...SAMPLE_RUN, id: 'run_local_1', workflow_name: 'local-wf', host_id: 'local' }])
        : new Promise(() => {}),
    );
    openPalette();
    await waitFor(() => expect(screen.getByText('local-wf')).toBeInTheDocument());
  });
```

Use that file's existing run fixture, or build a `RunListRow` inline in place of `SAMPLE_RUN`; likewise use its existing helper in place of `openPalette()`. If the other sources (agents, workflows, …) aren't stubbed in that file, stub them as its other tests do.

- [ ] **Step 6: Run the tests, then typecheck**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/paletteSources.test.ts src/components/CommandPalette && npx tsc -b && echo TSC_OK`
Expected: all pass, `TSC_OK`.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/lib/paletteSources.ts crates/rupu-cp/web/src/lib/paletteSources.test.ts crates/rupu-cp/web/src/components/CommandPalette.tsx crates/rupu-cp/web/src/components/CommandPalette.v2.test.tsx
git commit -m "$(cat <<'EOF'
feat(cp-web): ⌘K runs load per host; remote runs link with ?host=

The palette's run list waited on every host (server fan-out, 10k rows
per SSH host). Runs now append per host. runItems also dropped the
host, so a remote run opened without ?host= and the CP had to probe
every host to find it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 18: Full gates, live check against stub hosts, docs, PR

**Files:**
- Modify: `CLAUDE.md`
- Create, outside the repo (never committed): a stub-host script in the session scratchpad

- [ ] **Step 1: Full gates**

Run:

```
cargo test -p rupu-cp --lib 2>&1 | tail -5
cargo clippy -p rupu-cp --all-targets 2>&1 | grep -E "^(warning|error)" | head
cd crates/rupu-cp/web && npx vitest run 2>&1 | tail -8 && npx tsc -b && echo TSC_OK
```

Expected:
- Rust: the baseline counts plus the new tests, with no new failures.
- Clippy: clean.
- vitest: all pass. Compare against the Task 0 baseline; any new failure is yours.
- `TSC_OK` printed.

- [ ] **Step 2: Build the web bundle and the binary**

Run: `make cp-web` (rebuilds the embedded UI), then `cargo build -p rupu-cli`.
Expected: both succeed. The binary is at `target/debug/rupu`.

- [ ] **Step 3: Write the stub host.** Save it as `<scratchpad>/stubhost/stub_host.py`, where `<scratchpad>` is the session scratchpad directory. It must never be written inside the repo.

```python
#!/usr/bin/env python3
"""Stub "remote rupu-cp" for live-checking per-host loading. NEVER committed.

usage: stub_host.py <port> <ok|fail|hang> [delay_seconds]
"""
import json, sys, time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

PORT, MODE = int(sys.argv[1]), sys.argv[2]
DELAY = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0
NOW = datetime.now(timezone.utc)
NAME = f"stub{PORT}"

def iso(dt): return dt.strftime("%Y-%m-%dT%H:%M:%SZ")
def usage(): return {"input_tokens": 100, "output_tokens": 50, "cached_tokens": 0, "total_tokens": 150,
                     "cost_usd": 0.01, "priced": True, "runs": 1}

RUNS = [{"id": f"run_{NAME}_{i:03d}", "codename": f"stub-reef/heron#{i}", "codename_derived": False,
         "workflow_name": f"{NAME}-wf", "status": "running" if i % 10 == 0 else "completed",
         "started_at": iso(NOW - timedelta(minutes=7 * i + 3)), "finished_at": iso(NOW - timedelta(minutes=7 * i)),
         "trigger": "manual", "turns": 2, "duration_ms": 180000, "usage": usage()} for i in range(120)]
AGENTS = [{"codename": f"stub-reef/lynx#{i}", "codename_derived": False, "run_id": f"agent_{NAME}_{i:03d}",
           "source": "standalone", "agent": "reviewer", "status": "ok",
           "started_at": iso(NOW - timedelta(minutes=11 * i + 1)), "turns": 3, "usage": usage()} for i in range(40)]
CYCLES = [{"cycle_id": f"cyc_{NAME}_{i:03d}", "mode": "tick", "worker_name": NAME,
           "started_at": iso(NOW - timedelta(minutes=13 * i + 2)), "finished_at": iso(NOW - timedelta(minutes=13 * i + 1)),
           "workflow_count": 1, "ran_cycles": 1, "skipped_cycles": 0, "failed_cycles": 0, "run_ids": [], "usage": usage()}
          for i in range(40)]
EVENTS = [{"event_id": f"evt_{NAME}_{i:03d}", "cycle_id": f"cyc_{NAME}_{i:03d}", "at": iso(NOW - timedelta(minutes=13 * i + 2)),
           "kind": "run_launched", "workflow": f"{NAME}-wf", "run_id": f"run_{NAME}_{i:03d}", "status": "completed",
           "usage": usage(), "turns": 2, "duration_ms": 1000} for i in range(40)]
SESSIONS = [{"codename": f"stub-reef/owl#{i}", "codename_derived": False, "session_id": f"ses_{NAME}_{i:03d}",
             "agent_name": "assistant", "model": "stub-model", "status": "idle", "total_turns": 4,
             "created_at": iso(NOW - timedelta(hours=i + 1)), "updated_at": iso(NOW - timedelta(minutes=17 * i + 4)),
             "scope": "active"} for i in range(30)]

def page(rows, q):
    off, lim = int(q.get("offset", ["0"])[0]), int(q.get("limit", ["20"])[0])
    return rows[off:off + lim]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        u = urlparse(self.path); q = parse_qs(u.query); p = u.path
        if MODE == "hang": time.sleep(3600)
        time.sleep(DELAY)
        if MODE == "fail": return self.send(502, {"error": "stub: upstream down"})
        lc = q.get("lifecycle", [None])[0]
        runs = [r for r in RUNS if lc in (None, "") or (lc == "active") == (r["status"] == "running")]
        if p in ("/api/runs", "/api/runs/workflows"): return self.send(200, page(runs, q))
        if p == "/api/runs/agents": return self.send(200, page(AGENTS, q))
        if p == "/api/runs/autoflows": return self.send(200, page(CYCLES, q))
        if p == "/api/runs/autoflows/events": return self.send(200, page(EVENTS, q))
        if p == "/api/sessions": return self.send(200, page(SESSIONS, q))
        if p == "/api/usage":
            return self.send(200, {"summary": usage(), "breakdown": [], "unpriced": {"models": [], "rows": 0},
                                   "hosts": [{"host_id": "local", "name": NAME, "transport_kind": "local",
                                              "state": "ok", "captured_at": iso(datetime.now(timezone.utc)), "reason": None}]})
        return self.send(404, {"error": f"stub: no route {p}"})

ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
```

- [ ] **Step 4: Start the stubs and a throwaway CP.** This touches no real host. Run each long-running command with `run_in_background`:

```
python3 <scratchpad>/stubhost/stub_host.py 7601 ok 3      # slow but healthy
python3 <scratchpad>/stubhost/stub_host.py 7602 fail      # 502 on everything
python3 <scratchpad>/stubhost/stub_host.py 7603 hang      # never answers (connector times out)
```

Seed a temp `RUPU_HOME` with a few local runs. This copies only `run.json` files, read-only, into the scratchpad; they are never committed.

```
export TH=<scratchpad>/rupu-home && mkdir -p "$TH/runs"
for d in $(ls -t ~/.rupu/runs | head -40); do mkdir -p "$TH/runs/$d" && cp ~/.rupu/runs/$d/run.json "$TH/runs/$d/" 2>/dev/null; done
```

Then start the CP in the background: `RUPU_HOME=$TH target/debug/rupu cp serve --bind 127.0.0.1:7499`. Register the three stubs:

```
for p in 7601 7602 7603; do curl -s -X POST http://127.0.0.1:7499/api/hosts -H 'Content-Type: application/json' -d "{\"name\":\"stub$p\",\"base_url\":\"http://127.0.0.1:$p\"}"; echo; done
```

Expected: three JSON host views.

Before continuing, run `curl -s http://127.0.0.1:7499/api/hosts/registered` and confirm it lists only `local` + the three stubs. No SSH host may appear, since the temp home has none.

- [ ] **Step 5: Check it in the browser pane.** Open `http://127.0.0.1:7499` with `mcp__Claude_Browser__preview_start`, then go to the Activity page. For each tab (Workflow runs, Agent runs, Autoflow runs/cycles, Sessions), verify and record:
  1. Local rows appear before the 3 s stub, and the strip shows `stub7601 · loading…`.
  2. After ~3 s, stub7601's rows interleave in time order.
  3. stub7602 shows offline; the footer's "not included" names it.
  4. stub7603 stays loading, then turns offline when the HTTP connector times out (30 s). Local and stub7601 rows are unaffected the whole time.
  5. Scrolling: rows stay in newest-first order across hosts, with no jump-back above rows already scrolled past. The row being read stays in place when stub7601's rows insert above it.
  6. Refresh: no flash, and the strip ages reset.

  On Usage: the headline paints from local first, its sub-label says `excludes …` while stubs load or are down, and a pivot click issues no `/api/usage` request (check `read_network_requests`). On ⌘K: local runs list at once.

  Take one screenshot per page for the PR.

- [ ] **Step 6: Tear down**

Stop the CP and the three stubs (`TaskStop` / kill their background tasks). Then remove the temp home, guarding the path first (zsh never word-splits; a stray empty var must not hit `/`):

```
[ -n "$TH" ] && [ -d "$TH/runs" ] && rm -rf "$TH"
```

- [ ] **Step 7: Docs — `CLAUDE.md`**
  - Under "## Read first", add after the finding-reports entries:

    ```
    - CP progressive per-host loading spec + plan (Activity/Usage/⌘K load each host independently): `docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md`, `docs/superpowers/plans/2026-10-01-rupu-cp-progressive-per-host-loading.md`
    ```

  - In the `rupu-cp` crate bullet, append this sentence:

    ```
    Per-host list loading: the web loads Activity tables, the Usage headline and ⌘K runs one host at a time through the single-host `?host=<id>` paths (`web/src/lib/perHost/` — watermark merge + `PerHostListEngine`; `web/src/lib/usage/useUsageData.ts`), so those single-host list paths answer 501 (host can't serve it) / 502 (host down) instead of 500 (`api::runs::host_list_error`), and `SshHostConnector` serves its listing commands through a 5 s single-flight `host::listing_cache::ListingCache` cleared by every mutation.
    ```

- [ ] **Step 8: Commit the docs**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: point CLAUDE.md at the per-host loading spec, plan and seams

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 9: Rebase, push, and open the PR**
  - Refresh the base and rebase: `git fetch https://github.com/Section9Labs/rupu.git main:refs/remotes/origin/main && git rebase origin/main`. Resolve any conflicts and re-run Step 1's gates if anything moved.
  - Push with an explicit refspec over HTTPS (bare `git push` pushes every matching branch here, and SSH push can hang): `git push https://github.com/Section9Labs/rupu.git claude/cp-progressive-per-host-loading:claude/cp-progressive-per-host-loading`.
  - Verify the push landed: `git ls-remote https://github.com/Section9Labs/rupu.git claude/cp-progressive-per-host-loading`. Expected: the local HEAD sha.
  - `gh pr create --repo Section9Labs/rupu --base main --head claude/cp-progressive-per-host-loading`. Title: `CP: progressive per-host loading for Activity, Usage and ⌘K`. Body:
    - the problem
    - the approach, with links to the spec and plan
    - the SSH budget (48 → ~4 commands/min for an open All-hosts polling table on 4 SSH hosts)
    - the bugs fixed in passing (`usePagedList` splice, ⌘K remote links, `pivot=project` dropping SSH hosts)
    - the live-check results and screenshots from Step 5
    - **"Owed: one real-fleet look with the 4 SSH hosts (Activity + Usage) after release"**
    - end the body with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

---

## Self-Review

**Spec coverage**

| Spec | Task(s) |
|---|---|
| §2.4 pivot/`breakdown` | 16 |
| §2.5 SSH all-or-nothing | 2, 3 |
| §2.6 string ordering | 4 (`instantOf`/`compareNewestFirst`) |
| §2.7 `usePagedList` splice | 4 (`spliceHead`), 13 |
| §2.8 `HostSelect` | 8 |
| §6.1 slice model | 4 |
| §6.2 watermark merge | 4 |
| §6.3 hook, catch-up, scroll, single-flight per host, refresh rule, cadence, manual Refresh, `refreshHost`, `usePagedList` cleanup | 6, 7, 13 |
| §6.4 pages, footer | 7 (chrome), 8, 9–12 |
| §6.5 Usage | 14–16 |
| §6.6 ⌘K | 17 |
| §7.1 error statuses | 1 |
| §7.2 SSH listing cache | 2, 3 |
| §7.3 unchanged bare fan-out paths | no task, by design |
| §8 errors and honest state | 5, 6, 7, 9–12, 15 |
| §9 testing | each task's tests |
| §10 live verification | 18 |

**Placeholder scan.** Page-migration tasks name exact JSX replacements. Where existing test files have helpers whose names this plan can't see (Usage `stubAll`, CommandPalette `openPalette`/`SAMPLE_RUN`), the step says explicitly to use the file's existing helper. The behaviour asserted is fully specified.

**Type consistency.** These names match across tasks:
- `PerHostFetchParams { host, offset, limit }`
- `usePerHostPagedList({ host, fetch, timeField, idField, deps, poll })` → `{ rows, slices, loading, error, hasMore, ended, sentinelRef, refresh, refreshHost, retryPaging, removeRow }`
- `HostSlice` fields, `RowAccess { timeOf, keyOf, idOf }`, `HostSeed`
- `perHostFooterText({ slices, loading, hasMore, ended, count })`
- `useUsageData(usageWindow, windowKey, windowSource)` → `{ data: { summary, unpriced, excluded }, hosts, error }`
- `host_list_error`, `ListingCache::{get, clear, clear_on_drop}`, `cached_rows`, `exec_rupu_json`, `json_rows`

**Deviations from the spec, made explicit:**
- **§6.5:** the spec says "no `group_by` is sent". The plan pins `group_by=model`. That is the server default, so it is behaviourally identical, and it keeps `api.getUsage`'s signature unchanged.
- **§7.2:** the mutation list also includes `delete_session` and `start_session`, both mutating `SshHostConnector` methods the spec's list missed.
- **Task 3 Step 4b (added, a bug found while planning):** SSH `list_runs`/`dashboard_summary` turned every `run list` failure, including a down host, into `Unsupported`. Under §7.1 that would show a down SSH host as *unavailable* (retried on manual Refresh only), and the dashboard already shows it as "needs a newer rupu". ssh transport failures now pass through as `Unreachable` → offline.
