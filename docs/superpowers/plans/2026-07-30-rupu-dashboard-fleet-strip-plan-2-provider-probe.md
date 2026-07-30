# Dashboard Fleet Strip — Plan 2: provider probe

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill `FleetCounts::providers_configured` and `providers_unhealthy` from a real, cached authentication probe — never from config presence.

**Architecture:** `LlmProvider` gains a `probe()` method whose default is `NotImplemented`, so a provider without a real probe reports "never probed" rather than a fabricated OK. rupu-cp defines a `FleetInventory` **port** (it must: rupu-cp does not depend on rupu-providers, and architecture rule 1 keeps it that way). `rupu cp serve` installs the adapter, which owns a `ProviderRegistry`, refreshes a TTL cache on a background task, and answers from the cache only. `LocalHostConnector` holds an optional `Arc<dyn FleetInventory>` and folds its snapshot into the `FleetCounts` Plan 1 built.

**Tech Stack:** Rust 2021 (async-trait, tokio, chrono, reqwest via the existing provider clients).

## Spec

`docs/superpowers/specs/2026-07-30-rupu-dashboard-fleet-strip-design.md` §3 (ProviderProbeCache).

## Prerequisite

**Plan 1 must be merged.** This plan fills two fields on the `FleetCounts` struct Plan 1 introduces and extends the collector Plan 1 creates.

## Global Constraints

- Workspace deps only — versions pinned in the root `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden.
- **rupu-cp must not gain a dependency on rupu-providers.** The probe crosses that boundary as a port, exactly like `RepoLister` (`crates/rupu-cp/src/repos.rs`) and its adapter (`crates/rupu-cli/src/cp_repos.rs`).
- `rupu-cli` stays thin — but a port *adapter* is not business logic; `cp_repos.rs` is the precedent for where it lives.
- Errors: `thiserror` in libraries, `anyhow` in the CLI binary.
- **Never run package-wide `cargo fmt`** — format only the files you touched.
- **The dashboard must never block on a probe.** Building a summary reads the cache; the network happens only on the background refresh task.
- A provider that has never been probed is neither healthy nor unhealthy.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/rupu-providers/src/provider.rs` | **Modify.** Add `LlmProvider::probe()` with a `NotImplemented` default. |
| `crates/rupu-providers/src/anthropic.rs` | **Modify.** Real `probe()` impl. |
| `crates/rupu-cp/src/fleet_inventory.rs` | **Create.** The `FleetInventory` port + `InventorySnapshot` DTO. |
| `crates/rupu-cp/src/lib.rs` | **Modify.** Register the module. |
| `crates/rupu-cp/src/host/local.rs` | **Modify.** Hold an optional inventory port; fold its snapshot in. |
| `crates/rupu-cp/src/host/fleet_counts.rs` | **Modify.** `apply_inventory` merges a snapshot into `FleetCounts`. |
| `crates/rupu-cli/src/cp_inventory.rs` | **Create.** The adapter: owns `ProviderRegistry`, TTL cache, background refresh. |
| `crates/rupu-cli/src/cmd/cp.rs` | **Modify.** Build the adapter, install it, spawn the refresh task. |

---

### Task 1: `LlmProvider::probe()`

**Files:**
- Modify: `crates/rupu-providers/src/provider.rs:33-56`
- Modify: `crates/rupu-providers/src/anthropic.rs` (near the existing `list_models` impl, ~line 1962)

**Interfaces:**
- Produces: `async fn probe(&self) -> Result<(), ProviderError>` on `LlmProvider`. `Ok(())` means the provider answered an authenticated request. The default returns `ProviderError::NotImplemented { provider }`, which callers map to "never probed" — never to healthy.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/rupu-providers/src/provider.rs`:

```rust
    /// The default must NOT be derived from `list_models()`. That method
    /// returns a bare `Vec` and swallows every error into an empty result, so
    /// deriving from it would report a 401 as "healthy, zero models". A
    /// provider with no real probe must be indistinguishable from one that has
    /// never been probed.
    #[tokio::test]
    async fn probe_default_is_not_implemented() {
        let p = MockProvider {
            response: crate::provider::tests::mock_response(),
        };
        let err = p.probe().await.expect_err("default must not report success");
        assert!(
            matches!(err, ProviderError::NotImplemented { .. }),
            "expected NotImplemented, got {err:?}"
        );
    }
```

If the existing `MockProvider` fixture has no shared constructor, build it inline the way the surrounding tests in this module already do rather than adding `mock_response`.

Append to `mod tests` in `crates/rupu-providers/src/anthropic.rs`, following the pattern of the existing `list_models_returns_empty_on_non_2xx` test (same mock-server harness):

```rust
    #[tokio::test]
    async fn probe_maps_401_to_unauthorized() {
        // Reuse this module's existing mock-server harness; return 401 for the
        // models endpoint.
        let server = mock_server_returning(401, r#"{"error":{"message":"invalid x-api-key"}}"#).await;
        let client = client_pointing_at(&server);

        let err = <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect_err("401 must not be reported as healthy");

        assert!(
            matches!(
                err,
                ProviderError::Unauthorized { .. } | ProviderError::Api { status: 401, .. }
            ),
            "a 401 must surface as an auth failure, got {err:?}"
        );
    }

    #[tokio::test]
    async fn probe_succeeds_on_2xx() {
        let server = mock_server_returning(200, r#"{"data":[]}"#).await;
        let client = client_pointing_at(&server);
        <AnthropicClient as crate::provider::LlmProvider>::probe(&client)
            .await
            .expect("a 2xx must probe clean");
    }
```

Adapt `mock_server_returning` / `client_pointing_at` to whatever the existing tests in that module actually call — do not introduce a second mocking harness.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers probe_`
Expected: FAIL — no method named `probe`.

- [ ] **Step 3: Add the trait method**

In `crates/rupu-providers/src/provider.rs`, inside `pub trait LlmProvider`:

```rust
    /// Liveness + authorization probe: does this provider answer an
    /// authenticated request right now?
    ///
    /// `Ok(())` means an authenticated call succeeded. Errors carry WHY, which
    /// is the point — an operator needs to tell "the key is wrong" from "the
    /// host is unreachable".
    ///
    /// The default is `NotImplemented`, NOT a `list_models()`-derived guess.
    /// `list_models` returns a bare `Vec` and folds every failure into an
    /// empty result, so a 401 would be indistinguishable from a provider with
    /// no models — reporting that as healthy is precisely the false green
    /// light this whole feature exists to avoid. A provider without a real
    /// probe reports "never probed" instead.
    async fn probe(&self) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented {
            provider: self.provider_id().to_string(),
        })
    }
```

In `crates/rupu-providers/src/anthropic.rs`, add a real impl next to `list_models`. Issue the same authenticated `GET /v1/models` request `list_models` already builds, but propagate the outcome instead of swallowing it:

```rust
    async fn probe(&self) -> Result<(), crate::error::ProviderError> {
        // Same authenticated request `list_models` makes — but the status is
        // the answer here, so nothing is swallowed. A 2xx (even with zero
        // models) means the credential works.
        let resp = self.models_request().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(crate::error::ProviderError::Api {
            status: status.as_u16(),
            message: body,
        })
    }
```

Factor the request-building half of `list_models` into a `models_request()` helper returning `Result<reqwest::Response, ProviderError>` and have both call it, so the probe and the listing can never drift apart in auth handling. Leave `list_models`'s public signature and error-swallowing behaviour unchanged — other callers depend on it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-providers probe_`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-providers/src/provider.rs crates/rupu-providers/src/anthropic.rs
git add crates/rupu-providers/src/provider.rs crates/rupu-providers/src/anthropic.rs
git commit -m "feat(providers): LlmProvider::probe with a NotImplemented default"
```

---

### Task 2: The `FleetInventory` port

**Files:**
- Create: `crates/rupu-cp/src/fleet_inventory.rs`
- Modify: `crates/rupu-cp/src/lib.rs`
- Modify: `crates/rupu-cp/src/host/fleet_counts.rs`

**Interfaces:**
- Produces:
  - `pub enum ProbeState { Ok, AuthFailed { detail: String }, Unreachable { detail: String }, NeverProbed }`
  - `pub struct ProviderProbeRow { pub provider: String, pub state: ProbeState, pub probed_at: Option<DateTime<Utc>> }`
  - `pub struct InventorySnapshot { pub providers: Vec<ProviderProbeRow>, pub repos: Option<u64>, pub issues_pending: Option<u64>, pub issues_open: Option<u64>, pub issues_capped: bool, pub captured_at: Option<DateTime<Utc>> }` — the repo/issue fields land unused here and are filled by Plan 3.
  - `pub trait FleetInventory: Send + Sync { fn snapshot(&self) -> InventorySnapshot; }` — **synchronous and non-blocking**: it reads an already-refreshed cache.
  - `pub fn apply_inventory(base: FleetCounts, snap: &InventorySnapshot) -> FleetCounts` in `host::fleet_counts`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cp/src/fleet_inventory.rs` with the module doc and tests only:

```rust
//! `FleetInventory` port — the SCM- and provider-backed half of the fleet
//! strip.
//!
//! rupu-cp deliberately does not depend on rupu-providers or hold SCM
//! credentials, so both live behind this port exactly as repo listing lives
//! behind [`crate::repos::RepoLister`]. `rupu cp serve` installs the adapter.
//!
//! [`FleetInventory::snapshot`] is SYNCHRONOUS and must never block: it reads
//! a cache the adapter refreshes on its own schedule. Building a dashboard
//! summary must never wait on a network round-trip.

#![deny(clippy::all)]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_probed_counts_as_neither_healthy_nor_unhealthy() {
        let snap = InventorySnapshot {
            providers: vec![
                row("anthropic", ProbeState::Ok),
                row("openai", ProbeState::NeverProbed),
                row("google", ProbeState::AuthFailed { detail: "401".into() }),
            ],
            ..InventorySnapshot::default()
        };
        assert_eq!(snap.providers_configured(), 3);
        assert_eq!(
            snap.providers_unhealthy(),
            1,
            "NeverProbed is an absence of information — it must not be counted \
             as unhealthy, and must not be counted as healthy either"
        );
    }

    #[test]
    fn unreachable_counts_as_unhealthy() {
        let snap = InventorySnapshot {
            providers: vec![
                row("a", ProbeState::Unreachable { detail: "dns".into() }),
                row("b", ProbeState::AuthFailed { detail: "401".into() }),
            ],
            ..InventorySnapshot::default()
        };
        assert_eq!(snap.providers_unhealthy(), 2);
    }

    fn row(name: &str, state: ProbeState) -> ProviderProbeRow {
        ProviderProbeRow {
            provider: name.into(),
            state,
            probed_at: None,
        }
    }
}
```

And in `crates/rupu-cp/src/host/fleet_counts.rs`, append to its `mod tests`:

```rust
    /// The snapshot fills ONLY the fields it owns. A base count Plan 1
    /// sourced from disk must survive untouched.
    #[test]
    fn apply_inventory_fills_provider_fields_without_clobbering_local_counts() {
        use crate::fleet_inventory::{InventorySnapshot, ProbeState, ProviderProbeRow};

        let base = FleetCounts {
            workers: Some(3),
            claims_active: Some(9),
            ..FleetCounts::default()
        };
        let stamp = chrono::Utc::now();
        let snap = InventorySnapshot {
            providers: vec![
                ProviderProbeRow {
                    provider: "anthropic".into(),
                    state: ProbeState::Ok,
                    probed_at: Some(stamp),
                },
                ProviderProbeRow {
                    provider: "google".into(),
                    state: ProbeState::AuthFailed {
                        detail: "401".into(),
                    },
                    probed_at: Some(stamp),
                },
            ],
            captured_at: Some(stamp),
            ..InventorySnapshot::default()
        };

        let out = apply_inventory(base, &snap);

        assert_eq!(out.providers_configured, Some(2));
        assert_eq!(out.providers_unhealthy, Some(1));
        assert_eq!(out.workers, Some(3), "local counts must survive");
        assert_eq!(out.claims_active, Some(9));
        assert_eq!(out.inventory_captured_at, Some(stamp));
    }

    /// With no providers known at all, report nothing rather than `Some(0)` —
    /// "the adapter has not filled its cache yet" is not "you have no
    /// providers".
    #[test]
    fn apply_inventory_with_an_empty_snapshot_reports_no_provider_counts() {
        let out = apply_inventory(FleetCounts::default(), &Default::default());
        assert_eq!(out.providers_configured, None);
        assert_eq!(out.providers_unhealthy, None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp fleet_inventory apply_inventory`
Expected: FAIL — module not registered, types absent.

- [ ] **Step 3: Implement the port**

Register in `crates/rupu-cp/src/lib.rs` beside the other port modules (`pub mod repos;`):

```rust
pub mod fleet_inventory;
```

Add above the test module in `crates/rupu-cp/src/fleet_inventory.rs`:

```rust
use chrono::{DateTime, Utc};

/// Outcome of one provider's most recent probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeState {
    /// An authenticated request succeeded.
    Ok,
    /// The provider answered, and rejected our credentials.
    AuthFailed { detail: String },
    /// The provider did not answer (DNS, connect, timeout, 5xx).
    Unreachable { detail: String },
    /// No probe has run — either the adapter has not reached this provider
    /// yet, or the provider has no `probe()` implementation. NOT a health
    /// verdict in either direction.
    NeverProbed,
}

/// One provider's row in the probe cache.
#[derive(Debug, Clone)]
pub struct ProviderProbeRow {
    /// Provider id as a display string, e.g. `"anthropic"`.
    pub provider: String,
    pub state: ProbeState,
    pub probed_at: Option<DateTime<Utc>>,
}

/// Everything the fleet strip needs that rupu-cp cannot read itself.
///
/// The repo/issue fields are declared here but left `None` until Plan 3 —
/// keeping one snapshot type avoids a second port and a second cache for
/// data with the same lifetime and the same staleness stamp.
#[derive(Debug, Clone, Default)]
pub struct InventorySnapshot {
    pub providers: Vec<ProviderProbeRow>,
    pub repos: Option<u64>,
    pub issues_pending: Option<u64>,
    pub issues_open: Option<u64>,
    pub issues_capped: bool,
    /// When the underlying caches were filled. `None` = never filled.
    pub captured_at: Option<DateTime<Utc>>,
}

impl InventorySnapshot {
    /// Providers rupu holds usable credentials for — every row in the cache,
    /// whatever its probe state.
    pub fn providers_configured(&self) -> u64 {
        self.providers.len() as u64
    }

    /// Providers whose last probe failed. `NeverProbed` is excluded
    /// deliberately: it is an absence of information, and counting it either
    /// way would assert something no probe has established.
    pub fn providers_unhealthy(&self) -> u64 {
        self.providers
            .iter()
            .filter(|p| {
                matches!(
                    p.state,
                    ProbeState::AuthFailed { .. } | ProbeState::Unreachable { .. }
                )
            })
            .count() as u64
    }
}

/// Read-only access to the adapter's cache.
///
/// Synchronous by design — see the module doc. An implementation that performs
/// I/O here would block every dashboard render on the slowest provider.
pub trait FleetInventory: Send + Sync {
    fn snapshot(&self) -> InventorySnapshot;
}
```

Add to `crates/rupu-cp/src/host/fleet_counts.rs`:

```rust
use crate::fleet_inventory::InventorySnapshot;

/// Fold an inventory snapshot into the disk-sourced counts.
///
/// Only fields the snapshot owns are touched; everything Plan 1 read from
/// `<global_dir>` passes through untouched. An empty snapshot (no providers
/// known) leaves the provider fields `None` — "the cache has not filled yet"
/// must not render as "you have zero providers".
pub fn apply_inventory(base: FleetCounts, snap: &InventorySnapshot) -> FleetCounts {
    let providers_known = !snap.providers.is_empty();
    FleetCounts {
        providers_configured: providers_known.then(|| snap.providers_configured()),
        providers_unhealthy: providers_known.then(|| snap.providers_unhealthy()),
        repos: snap.repos,
        issues_pending: snap.issues_pending,
        issues_open: snap.issues_open,
        issues_capped: snap.issues_capped,
        inventory_captured_at: snap.captured_at,
        ..base
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp fleet_inventory apply_inventory`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/fleet_inventory.rs crates/rupu-cp/src/host/fleet_counts.rs crates/rupu-cp/src/lib.rs
git add crates/rupu-cp/src/fleet_inventory.rs crates/rupu-cp/src/host/fleet_counts.rs crates/rupu-cp/src/lib.rs
git commit -m "feat(cp): FleetInventory port and snapshot folding"
```

---

### Task 3: Wire the port into the local connector

**Files:**
- Modify: `crates/rupu-cp/src/host/local.rs` (struct, builder, `dashboard_summary`)

**Interfaces:**
- Consumes: `FleetInventory`, `apply_inventory` (Task 2).
- Produces: `LocalHostConnector::with_inventory(self, inv: Option<Arc<dyn FleetInventory>>) -> Self`, following the existing `with_pricing` builder shape.

- [ ] **Step 1: Write the failing test**

Append to `mod dashboard_summary_tests` in `crates/rupu-cp/src/host/local.rs`:

```rust
    /// With an inventory installed, its provider counts reach the summary.
    /// Without one, the provider fields stay `None` — a read-only `rupu cp`
    /// (no `cp serve`, so no adapter) must claim nothing about provider health.
    #[tokio::test]
    async fn dashboard_summary_folds_in_the_inventory_when_installed() {
        use crate::fleet_inventory::{
            FleetInventory, InventorySnapshot, ProbeState, ProviderProbeRow,
        };

        struct Fake;
        impl FleetInventory for Fake {
            fn snapshot(&self) -> InventorySnapshot {
                InventorySnapshot {
                    providers: vec![
                        ProviderProbeRow {
                            provider: "anthropic".into(),
                            state: ProbeState::Ok,
                            probed_at: None,
                        },
                        ProviderProbeRow {
                            provider: "google".into(),
                            state: ProbeState::Unreachable { detail: "dns".into() },
                            probed_at: None,
                        },
                    ],
                    ..InventorySnapshot::default()
                }
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let conn = local_connector_for(tmp.path()).with_inventory(Some(std::sync::Arc::new(Fake)));
        let sum = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::All)
            .await
            .expect("summary");

        assert_eq!(sum.fleet.providers_configured, Some(2));
        assert_eq!(sum.fleet.providers_unhealthy, Some(1));
    }

    #[tokio::test]
    async fn dashboard_summary_reports_no_provider_counts_without_an_inventory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let conn = local_connector_for(tmp.path());
        let sum = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::All)
            .await
            .expect("summary");

        assert_eq!(sum.fleet.providers_configured, None);
        assert_eq!(
            sum.fleet.providers_unhealthy, None,
            "no adapter means no probe has run; the strip must claim nothing"
        );
    }
```

Reuse this module's existing helper for constructing a `LocalHostConnector` against a tempdir; if it has no such helper, add `local_connector_for(root: &Path) -> LocalHostConnector` alongside the existing fixtures rather than inlining the six-argument `new` twice.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp dashboard_summary_folds_in_the_inventory dashboard_summary_reports_no_provider_counts`
Expected: FAIL — no method `with_inventory`.

- [ ] **Step 3: Implement**

Add the field to `struct LocalHostConnector`:

```rust
    /// Optional inventory port. `None` in a read-only `rupu cp` (no adapter is
    /// installed without `cp serve`), which is why every field it feeds is
    /// `Option` — absent means unreported, not zero.
    inventory: Option<std::sync::Arc<dyn crate::fleet_inventory::FleetInventory>>,
```

Initialize it to `None` in `LocalHostConnector::new` and add the builder next to `with_pricing`:

```rust
    /// Install the fleet-inventory port (or clear it with `None`). `cp serve`
    /// calls this; a read-only `rupu cp` does not.
    pub fn with_inventory(
        mut self,
        inventory: Option<std::sync::Arc<dyn crate::fleet_inventory::FleetInventory>>,
    ) -> Self {
        self.inventory = inventory;
        self
    }
```

In `dashboard_summary`, replace the `let fleet = ...` line Plan 1 added:

```rust
        // Disk-sourced counts first, then the port's snapshot folded on top.
        // `snapshot()` reads an already-refreshed cache — no network here, so
        // a dead provider can never slow the dashboard down.
        let mut fleet = crate::host::fleet_counts::collect_fleet_counts(&self.global_dir);
        if let Some(inv) = &self.inventory {
            fleet = crate::host::fleet_counts::apply_inventory(fleet, &inv.snapshot());
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp`
Expected: PASS. Fix any `LocalHostConnector::new` call sites the new field breaks.

- [ ] **Step 5: Commit**

```bash
cargo fmt -- crates/rupu-cp/src/host/local.rs
git add crates/rupu-cp/src/host/local.rs
git commit -m "feat(cp): fold the fleet inventory into the local host summary"
```

---

### Task 4: The `cp serve` adapter

**Files:**
- Create: `crates/rupu-cli/src/cp_inventory.rs`
- Modify: `crates/rupu-cli/src/cmd/cp.rs`

**Interfaces:**
- Consumes: `FleetInventory`, `InventorySnapshot`, `ProbeState`, `ProviderProbeRow` (Task 2); `LlmProvider::probe` (Task 1); `ProviderRegistry`.
- Produces: `pub struct CpFleetInventory` implementing `FleetInventory`, plus `pub async fn refresh(&self)` for the background task and `pub fn classify(err: &ProviderError) -> ProbeState` for the mapping.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cli/src/cp_inventory.rs` with the module doc and tests only:

```rust
//! `cp serve` adapter for rupu-cp's `FleetInventory` port.
//!
//! Owns a `ProviderRegistry`, probes every credentialled provider on a TTL,
//! and serves the result from an in-memory cache. `snapshot()` never performs
//! I/O — the dashboard reads the cache, and only the background refresh task
//! touches the network.

#![deny(clippy::all)]

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_providers::error::ProviderError;

    #[test]
    fn auth_shaped_errors_classify_as_auth_failed() {
        for err in [
            ProviderError::Unauthorized {
                provider: "anthropic".into(),
                auth_mode: Default::default(),
                hint: "check your key".into(),
            },
            ProviderError::MissingAuth {
                provider: "anthropic".into(),
                env_hint: "ANTHROPIC_API_KEY".into(),
            },
            ProviderError::TokenRefreshFailed("expired".into()),
            ProviderError::Api {
                status: 401,
                message: "nope".into(),
            },
            ProviderError::Api {
                status: 403,
                message: "nope".into(),
            },
        ] {
            assert!(
                matches!(classify(&err), ProbeState::AuthFailed { .. }),
                "{err:?} must classify as AuthFailed"
            );
        }
    }

    #[test]
    fn transport_and_server_errors_classify_as_unreachable() {
        for err in [
            ProviderError::Http("connection refused".into()),
            ProviderError::Api {
                status: 503,
                message: "down".into(),
            },
        ] {
            assert!(
                matches!(classify(&err), ProbeState::Unreachable { .. }),
                "{err:?} must classify as Unreachable"
            );
        }
    }

    /// A provider with no `probe()` impl must report NeverProbed — the whole
    /// point of the NotImplemented default. Classifying it as Unreachable
    /// would put a red count on a provider that may be perfectly fine.
    #[test]
    fn not_implemented_classifies_as_never_probed() {
        let err = ProviderError::NotImplemented {
            provider: "google-gemini-cli".into(),
        };
        assert!(matches!(classify(&err), ProbeState::NeverProbed));
    }

    /// Before the first refresh the cache is empty, so the strip reports
    /// nothing about providers rather than "0 configured".
    #[test]
    fn snapshot_before_any_refresh_is_empty() {
        let inv = CpFleetInventory::new_for_test();
        assert!(inv.snapshot().providers.is_empty());
        assert!(inv.snapshot().captured_at.is_none());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli cp_inventory`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement the adapter**

Declare the module in `crates/rupu-cli/src/main.rs` (or `lib.rs`, matching where `mod cp_repos;` is declared):

```rust
mod cp_inventory;
```

Add above the test module in `crates/rupu-cli/src/cp_inventory.rs`:

```rust
use chrono::Utc;
use rupu_cp::fleet_inventory::{
    FleetInventory, InventorySnapshot, ProbeState, ProviderProbeRow,
};
use rupu_providers::{error::ProviderError, registry::ProviderRegistry};
use std::sync::{Arc, RwLock};

/// How long a probe result stays authoritative. Long enough that a fleet of
/// providers costs a handful of requests an hour; short enough that a revoked
/// key turns the strip red within a coffee break.
pub const PROBE_TTL_SECS: u64 = 300;

pub struct CpFleetInventory {
    registry: Option<Arc<ProviderRegistry>>,
    cache: RwLock<InventorySnapshot>,
}

/// Map a probe error to a cache state.
///
/// `NotImplemented` is NOT a failure — it means this provider has no probe, so
/// nothing has been established about it. Everything auth-shaped is
/// `AuthFailed`; everything transport- or server-shaped is `Unreachable`.
pub fn classify(err: &ProviderError) -> ProbeState {
    match err {
        ProviderError::NotImplemented { .. } => ProbeState::NeverProbed,
        ProviderError::Unauthorized { .. }
        | ProviderError::MissingAuth { .. }
        | ProviderError::TokenRefreshFailed(_)
        | ProviderError::AuthConfig(_) => ProbeState::AuthFailed {
            detail: err.to_string(),
        },
        ProviderError::Api { status, .. } if *status == 401 || *status == 403 => {
            ProbeState::AuthFailed {
                detail: err.to_string(),
            }
        }
        _ => ProbeState::Unreachable {
            detail: err.to_string(),
        },
    }
}

impl CpFleetInventory {
    pub fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self {
            registry: Some(registry),
            cache: RwLock::new(InventorySnapshot::default()),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            registry: None,
            cache: RwLock::new(InventorySnapshot::default()),
        }
    }

    /// Probe every credentialled provider and replace the cache.
    ///
    /// Errors are recorded as states, never propagated: one dead provider must
    /// not blank the other rows. Rate limiting is deliberately NOT a health
    /// failure — a 429 means the credential works.
    pub async fn refresh(&self) {
        let Some(registry) = &self.registry else {
            return;
        };
        let mut rows = Vec::new();
        for id in registry.available_providers() {
            let state = match registry.create_provider(id) {
                Ok(p) => match p.probe().await {
                    Ok(()) => ProbeState::Ok,
                    Err(ProviderError::RateLimited { .. }) => ProbeState::Ok,
                    Err(e) => classify(&e),
                },
                // The credential store listed it but a client could not be
                // built — that is an auth/config problem, and `classify`
                // already knows how to name it.
                Err(e) => classify(&e),
            };
            rows.push(ProviderProbeRow {
                provider: id.to_string(),
                state,
                probed_at: Some(Utc::now()),
            });
        }

        if let Ok(mut c) = self.cache.write() {
            c.providers = rows;
            c.captured_at = Some(Utc::now());
        }
    }
}

impl FleetInventory for CpFleetInventory {
    fn snapshot(&self) -> InventorySnapshot {
        self.cache
            .read()
            .map(|c| c.clone())
            .unwrap_or_default()
    }
}
```

`ProviderRegistry::create_provider` returns `Box<dyn LlmProvider>`; `probe(&self)` takes `&self`, so no `mut` binding is needed. If `ProviderId` does not implement `Display`, use its existing `auth_key()` for the row's `provider` string.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli cp_inventory`
Expected: 4 passed.

- [ ] **Step 5: Install it in `cp serve`**

In `crates/rupu-cli/src/cmd/cp.rs`, where `CpRepoLister` is built and installed via `with_repos`, build the inventory the same way, install it on the `LocalHostConnector` used by the fully-wired host registry (`AppState::with_hosts`), and spawn the refresher:

```rust
    // Fleet inventory: probe providers on a TTL so the dashboard reads a cache
    // instead of the network. One immediate refresh so the strip is populated
    // by the time the operator's first page load lands.
    let inventory = std::sync::Arc::new(crate::cp_inventory::CpFleetInventory::new(
        std::sync::Arc::clone(&provider_registry),
    ));
    {
        let inv = std::sync::Arc::clone(&inventory);
        tokio::spawn(async move {
            loop {
                inv.refresh().await;
                tokio::time::sleep(std::time::Duration::from_secs(
                    crate::cp_inventory::PROBE_TTL_SECS,
                ))
                .await;
            }
        });
    }
```

Pass `.with_inventory(Some(inventory))` to the `LocalHostConnector` builder chain in the same place `.with_pricing(...)` is already applied. If `cp serve` has no `ProviderRegistry` in scope, construct one from the same credential store `rupu run` uses — do not build a second credential-resolution path.

- [ ] **Step 6: Verify the wiring compiles and the suite is green**

Run: `cargo build -p rupu-cli && cargo test -p rupu-cli && cargo test -p rupu-cp`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
cargo fmt -- crates/rupu-cli/src/cp_inventory.rs crates/rupu-cli/src/cmd/cp.rs
git add crates/rupu-cli/src/cp_inventory.rs crates/rupu-cli/src/cmd/cp.rs crates/rupu-cli/src/main.rs
git commit -m "feat(cli): cp serve provider probe cache behind the FleetInventory port"
```

---

## Verification

- [ ] `cargo clippy -p rupu-cp -p rupu-cli -p rupu-providers -- -D warnings` clean.
- [ ] Start `rupu cp serve` with a valid Anthropic credential: within one refresh the strip shows `N providers` with **no** unhealthy clause.
- [ ] Break the credential (edit the key to garbage) and restart: the strip shows `(1 unhealthy)` in the fault colour, and the segment's `data-fault` is `true`.
- [ ] Kill network access and restart: the failure is reported as unreachable, not as auth-failed.
- [ ] Run read-only `rupu cp` (no `serve`): the providers segment shows an em-dash and no health clause.
- [ ] Confirm a page load never blocks on a probe — with a hung provider, the dashboard still paints (the probe runs only on the background task).
