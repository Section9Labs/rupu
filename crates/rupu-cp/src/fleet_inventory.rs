//! `FleetInventory` port — the SCM- and provider-backed half of the fleet
//! strip.
//!
//! rupu-cp deliberately does not depend on rupu-providers and holds no SCM
//! credentials, so both live behind this port exactly as repo listing lives
//! behind [`crate::repos::RepoLister`]. `rupu cp serve` installs the adapter.
//!
//! [`FleetInventory::snapshot`] is SYNCHRONOUS and must never block: it reads
//! a cache the adapter refreshes on its own schedule. Building a dashboard
//! summary must never wait on a network round-trip — one hung provider would
//! otherwise stall every dashboard render.

#![deny(clippy::all)]

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
/// One snapshot type covers both the provider probe and the SCM inventory:
/// they cross the same boundary, live on the same cache, and share a staleness
/// stamp, so a second port would duplicate the crossing for no added
/// capability.
#[derive(Debug, Clone, Default)]
pub struct InventorySnapshot {
    pub providers: Vec<ProviderProbeRow>,
    pub repos: Option<u64>,
    pub issues_pending: Option<u64>,
    pub issues_open: Option<u64>,
    /// True when a repo hit the per-repo issue fetch cap, making
    /// `issues_open` a floor rather than a total.
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

    /// Providers whose last probe failed. [`ProbeState::NeverProbed`] is
    /// excluded deliberately: it is an absence of information, and counting it
    /// either way would assert something no probe has established.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, state: ProbeState) -> ProviderProbeRow {
        ProviderProbeRow {
            provider: name.into(),
            state,
            probed_at: None,
        }
    }

    #[test]
    fn never_probed_counts_as_neither_healthy_nor_unhealthy() {
        let snap = InventorySnapshot {
            providers: vec![
                row("anthropic", ProbeState::Ok),
                row("openai", ProbeState::NeverProbed),
                row(
                    "google",
                    ProbeState::AuthFailed {
                        detail: "401".into(),
                    },
                ),
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
                row(
                    "a",
                    ProbeState::Unreachable {
                        detail: "dns".into(),
                    },
                ),
                row(
                    "b",
                    ProbeState::AuthFailed {
                        detail: "401".into(),
                    },
                ),
            ],
            ..InventorySnapshot::default()
        };
        assert_eq!(snap.providers_unhealthy(), 2);
    }

    #[test]
    fn an_empty_snapshot_reports_zero_providers_and_no_stamp() {
        let snap = InventorySnapshot::default();
        assert_eq!(snap.providers_configured(), 0);
        assert_eq!(snap.providers_unhealthy(), 0);
        assert!(snap.captured_at.is_none());
        assert!(!snap.issues_capped);
    }
}
