//! DTOs for [`HostConnector::dashboard_summary`].
//!
//! One host's entire contribution to the dashboard, fetched in ONE round-trip.
//! Deliberately coarse: SSH hosts pay a full ssh handshake per call (no
//! ControlMaster multiplexing — see `host/ssh.rs` `RemoteExec::run`), so this
//! must not decompose into per-panel calls.

#![deny(clippy::all)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The dashboard's time window. Mirrors the UI's segmented control.
///
/// Deliberately NOT `Serialize`/`Deserialize`. The wire vocabulary is `"7d"` /
/// `"30d"` / `"all"`, produced and consumed by `as_str()` and `parse()`. A serde
/// derive would emit `"days7"` / `"days30"` instead — a second, disagreeing
/// representation of the same value. Route every conversion through
/// `parse()` / `as_str()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashboardRange {
    Days7,
    #[default]
    Days30,
    All,
}

impl DashboardRange {
    /// Parse the wire form (`"7d"` / `"30d"` / `"all"`). `None` on anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "7d" => Some(Self::Days7),
            "30d" => Some(Self::Days30),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// The cutoff instant, or `None` for [`DashboardRange::All`].
    pub fn since(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::Days7 => Some(now - chrono::Duration::days(7)),
            Self::Days30 => Some(now - chrono::Duration::days(30)),
            Self::All => None,
        }
    }

    /// CLI flag form, for shelling to a remote host.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::All => "all",
        }
    }
}

/// Live, non-terminal run counts. These are the states that answer
/// "is anything stuck right now".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActiveCounts {
    pub running: u64,
    pub awaiting_approval: u64,
    pub paused: u64,
    pub pending: u64,
}

/// One time bucket of terminal outcomes, for the trend area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalBucket {
    pub ts: DateTime<Utc>,
    pub completed: u64,
    pub failed: u64,
    pub rejected: u64,
    pub cancelled: u64,
}

/// Runs STARTED in a bucket, split by trigger. Same day-key alignment as
/// [`TerminalBucket`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputBucket {
    pub ts: DateTime<Utc>,
    pub manual: u64,
    pub cron: u64,
    pub event: u64,
}

/// Scalar cycle summary — the one line of cycle numbers (spec §5.5). NOT a
/// row array.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CycleCounts {
    pub total: u64,
    /// `None` when the host cannot report the ran/failed breakdown (SSH).
    /// Never fabricate 0.
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
    /// Deliberately distinct from [`DashboardSummary::captured_at`], which
    /// stamps the run-store read — the inventory is minutes-stale by design
    /// while the run data is seconds-stale. Plans 2 and 3.
    pub inventory_captured_at: Option<DateTime<Utc>>,
}

/// One host's complete dashboard contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub active: ActiveCounts,
    /// The single longest-running run, or None if nothing is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_longest: Option<ActiveLongest>,
    pub terminal_buckets: Vec<TerminalBucket>,
    pub throughput_buckets: Vec<ThroughputBucket>,
    pub cycles: CycleCounts,
    /// `None` when this host does not report open-findings data at all (e.g.
    /// SSH — the CLI has no findings surface). `Some(0)` is a genuine zero;
    /// `None` must never be summed as one at the aggregation layer
    /// (`api::dashboard` sums only `Some` values and flags the aggregate as
    /// partial when any reporting host contributed `None`).
    pub findings_open: Option<u64>,
    /// This host's inventory contribution. `#[serde(default)]` is
    /// load-bearing: `HttpHostConnector` parses a remote CP's body as a bare
    /// `DashboardSummary`, and a remote running an older rupu emits no
    /// `fleet` key at all. Without the default that host would fail to parse
    /// and drop off the dashboard entirely instead of degrading to an
    /// all-`None` strip contribution.
    #[serde(default)]
    pub fleet: FleetCounts,
    /// When this host's data was actually read. Drives the per-host freshness
    /// strip — a host 30s stale must not render as "live". Never synthesized
    /// at the aggregation layer; always set by the connector that read it.
    pub captured_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parses_from_wire_strings() {
        assert_eq!(DashboardRange::parse("7d"), Some(DashboardRange::Days7));
        assert_eq!(DashboardRange::parse("30d"), Some(DashboardRange::Days30));
        assert_eq!(DashboardRange::parse("all"), Some(DashboardRange::All));
        assert_eq!(DashboardRange::parse("bogus"), None);
    }

    #[test]
    fn active_counts_default_to_zero() {
        let a = ActiveCounts::default();
        assert_eq!(a.running, 0);
        assert_eq!(a.awaiting_approval, 0);
        assert_eq!(a.paused, 0);
        assert_eq!(a.pending, 0);
    }

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

    #[test]
    fn throughput_bucket_serializes_with_expected_field_names() {
        let b = ThroughputBucket {
            ts: chrono::Utc::now(),
            manual: 3,
            cron: 5,
            event: 1,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert!(
            v["ts"].as_str().unwrap().contains('T'),
            "ts must be RFC-3339"
        );
        assert_eq!(v["manual"], 3);
        assert_eq!(v["cron"], 5);
        assert_eq!(v["event"], 1);
    }

    #[test]
    fn cycle_counts_default_is_zero_total_and_none_breakdown() {
        let c = CycleCounts::default();
        assert_eq!(c.total, 0);
        assert_eq!(c.clean, None);
        assert_eq!(c.with_failures, None);
    }

    #[test]
    fn cycle_counts_clean_none_never_serializes_as_zero() {
        let c = CycleCounts {
            total: 5,
            clean: None,
            with_failures: Some(2),
        };
        let v = serde_json::to_value(&c).unwrap();
        // A host that cannot report the breakdown must serialize `clean` as
        // null (or omit it) — never fabricate a 0.
        assert!(
            v.get("clean").is_none() || v["clean"].is_null(),
            "clean: None must serialize as absent/null, never 0; got {v:?}"
        );
        assert_ne!(
            v.get("clean").cloned().unwrap_or(serde_json::Value::Null),
            serde_json::json!(0),
            "clean: None must never be presented as a genuine zero"
        );
        assert_eq!(v["with_failures"], 2);
        assert_eq!(v["total"], 5);
    }

    #[test]
    fn cycle_counts_round_trips_through_json() {
        let c = CycleCounts {
            total: 10,
            clean: Some(7),
            with_failures: Some(3),
        };
        let v = serde_json::to_value(&c).unwrap();
        let back: CycleCounts = serde_json::from_value(v).unwrap();
        assert_eq!(back.total, 10);
        assert_eq!(back.clean, Some(7));
        assert_eq!(back.with_failures, Some(3));
    }

    #[test]
    fn active_longest_round_trips_with_expected_field_names() {
        let a = ActiveLongest {
            run_id: "run-123".to_string(),
            workflow_name: "nightly-scan".to_string(),
            age_ms: 45_000,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["run_id"], "run-123");
        assert_eq!(v["workflow_name"], "nightly-scan");
        assert_eq!(v["age_ms"], 45_000);
        let back: ActiveLongest = serde_json::from_value(v).unwrap();
        assert_eq!(back.run_id, "run-123");
        assert_eq!(back.workflow_name, "nightly-scan");
        assert_eq!(back.age_ms, 45_000);
    }

    #[test]
    fn summary_serializes_captured_at_as_rfc3339() {
        let s = DashboardSummary {
            active: ActiveCounts::default(),
            active_longest: None,
            terminal_buckets: vec![],
            throughput_buckets: vec![],
            cycles: CycleCounts::default(),
            findings_open: Some(0),
            fleet: FleetCounts::default(),
            captured_at: chrono::Utc::now(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v["captured_at"].as_str().unwrap().contains('T'),
            "captured_at must be RFC-3339 — the freshness strip parses it"
        );
    }

    #[test]
    fn summary_active_longest_none_is_omitted_from_wire() {
        let s = DashboardSummary {
            active: ActiveCounts::default(),
            active_longest: None,
            terminal_buckets: vec![],
            throughput_buckets: vec![],
            cycles: CycleCounts::default(),
            findings_open: None,
            fleet: FleetCounts::default(),
            captured_at: chrono::Utc::now(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v.get("active_longest").is_none(),
            "active_longest: None must be omitted (skip_serializing_if), not null"
        );
        // findings_open has no such attribute in the spec — None still
        // serializes, just as null.
        assert!(v.get("findings_open").is_some());
        assert!(v["findings_open"].is_null());
    }

    #[test]
    fn summary_round_trips_through_json_with_active_longest_present() {
        let s = DashboardSummary {
            active: ActiveCounts {
                running: 2,
                awaiting_approval: 1,
                paused: 0,
                pending: 0,
            },
            active_longest: Some(ActiveLongest {
                run_id: "run-abc".to_string(),
                workflow_name: "deploy".to_string(),
                age_ms: 120_000,
            }),
            terminal_buckets: vec![TerminalBucket {
                ts: chrono::Utc::now(),
                completed: 4,
                failed: 1,
                rejected: 0,
                cancelled: 0,
            }],
            throughput_buckets: vec![ThroughputBucket {
                ts: chrono::Utc::now(),
                manual: 2,
                cron: 3,
                event: 0,
            }],
            cycles: CycleCounts {
                total: 6,
                clean: Some(4),
                with_failures: Some(2),
            },
            findings_open: Some(3),
            fleet: FleetCounts::default(),
            captured_at: chrono::Utc::now(),
        };
        let v = serde_json::to_value(&s).unwrap();
        let back: DashboardSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back.active.running, 2);
        assert_eq!(back.active_longest.unwrap().run_id, "run-abc");
        assert_eq!(back.terminal_buckets.len(), 1);
        assert_eq!(back.throughput_buckets.len(), 1);
        assert_eq!(back.cycles.total, 6);
        assert_eq!(back.findings_open, Some(3));
    }
}
