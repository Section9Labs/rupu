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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardRange {
    Days7,
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

impl Default for DashboardRange {
    fn default() -> Self {
        Self::Days30
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

/// One bar in the live swimlane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRunBar {
    pub run_id: String,
    pub workflow_name: String,
    /// `RunStatus::as_str()` form.
    pub status: String,
    pub started_at: DateTime<Utc>,
    /// `"manual"` | `"cron"` | `"event"`.
    pub trigger: String,
    /// `None` for manual runs; set when the run belongs to an autoflow cycle.
    pub cycle_id: Option<String>,
}

/// One run inside a cycle.
///
/// Carries `status`, not just an id, because the `+N clean` pill needs to know
/// what folds. `AutoflowCycleRow` supplies only ids, so the status is joined
/// server-side in `build_summary` — which already holds every run. Making the
/// client fetch a run per id would turn one expanded cycle into N requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRun {
    pub run_id: String,
    /// `RunStatus::as_str()` form. `"unknown"` when the cycle references a run
    /// this host cannot resolve — never silently omitted, or the cycle's run
    /// count would disagree with its own row.
    pub status: String,
}

/// One autoflow cycle, collapsed. The activity feed's primary row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRollup {
    pub cycle_id: String,
    pub worker_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub ran: u64,
    pub skipped: u64,
    pub failed: u64,
    pub runs: Vec<CycleRun>,
}

/// A manual-trigger run. Never grouped — always rendered individually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRun {
    pub id: String,
    pub workflow_name: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub trigger: String,
}

/// One host's complete dashboard contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub active: ActiveCounts,
    pub terminal_buckets: Vec<TerminalBucket>,
    pub active_runs: Vec<ActiveRunBar>,
    pub cycles: Vec<CycleRollup>,
    pub recent_manual: Vec<RecentRun>,
    pub findings_open: u64,
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
    fn summary_serializes_captured_at_as_rfc3339() {
        let s = DashboardSummary {
            active: ActiveCounts::default(),
            terminal_buckets: vec![],
            active_runs: vec![],
            cycles: vec![],
            recent_manual: vec![],
            findings_open: 0,
            captured_at: chrono::Utc::now(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v["captured_at"].as_str().unwrap().contains('T'),
            "captured_at must be RFC-3339 — the freshness strip parses it"
        );
    }
}
