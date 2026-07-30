//! Collects this host's [`FleetCounts`] from `<global_dir>`.
//!
//! Split out of `local.rs` (already long) so the collection can be exercised
//! against a tempdir without standing up a `LocalHostConnector`. Every reader
//! here degrades a failure to `None` + a `tracing::warn!` — never to a zero,
//! which would make a broken store indistinguishable from an empty one.

#![deny(clippy::all)]

use crate::api::repo_scope::{distinct_repo_workspaces, ScopeKind};
use crate::fleet_inventory::InventorySnapshot;
use crate::host::dashboard_summary::FleetCounts;
use rupu_workspace::worker_store::WorkerStore;
use rupu_workspace::{AutoflowClaimStore, ClaimStatus, RepoRegistryStore, WorkspaceStore};

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

/// Fold an inventory snapshot into the disk-sourced counts.
///
/// Only fields the snapshot owns are touched; everything read from
/// `<global_dir>` passes through untouched. An empty snapshot (no providers
/// known) leaves the provider fields `None` — "the cache has not filled yet"
/// must not render as "you have zero providers", which is why this keys off
/// `providers.is_empty()` rather than unconditionally calling the counters.
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

    /// The snapshot fills ONLY the fields it owns. A base count sourced from
    /// disk must survive untouched.
    #[test]
    fn apply_inventory_fills_provider_fields_without_clobbering_local_counts() {
        use crate::fleet_inventory::{ProbeState, ProviderProbeRow};

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
        let out = apply_inventory(FleetCounts::default(), &InventorySnapshot::default());
        assert_eq!(out.providers_configured, None);
        assert_eq!(out.providers_unhealthy, None);
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
