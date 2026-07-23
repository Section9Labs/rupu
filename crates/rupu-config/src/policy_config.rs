//! Enforcement-policy and CP-runtime config sections.

use serde::{Deserialize, Serialize};

/// Global enforcement policy. Keys named here (dotted paths, e.g.
/// `"permission_mode"`, `"autoflow.max_active"`) are LOCKED: their GLOBAL value
/// overrides project + env at resolution. Only read from the global layer — a
/// project cannot declare its own locks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    pub lock: Vec<String>,
}

/// CP-runtime settings persistable in config (the `[cp]` section). Absent
/// fields fall back to the CP's compiled defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CpConfig {
    /// Max bytes for a workspace-sync payload/delta. `None` ⇒ the CP's
    /// `MAX_WORKSPACE_BYTES` default.
    pub max_workspace_bytes: Option<u64>,
    /// Whether `rupu cp serve` runs the autoflow reconcile loop (issue +
    /// PR entity autoflows) in-process on a timer. Defaults to `true` so
    /// autoflows fire without a separate `rupu autoflow serve`/cron.
    #[serde(default = "CpConfig::default_true")]
    pub autoflow_reconcile_enabled: bool,
    /// Seconds between autoflow reconcile passes when the loop above is
    /// enabled. Defaults to 60.
    #[serde(default = "CpConfig::default_background_interval_secs")]
    pub autoflow_reconcile_interval_secs: u64,
    /// Whether `rupu cp serve` runs the cron/event-trigger tick loop
    /// (`rupu cron tick`'s core) in-process on a timer. Defaults to
    /// `true` so cron- and event-triggered workflows fire without an
    /// external `cron` entry.
    #[serde(default = "CpConfig::default_true")]
    pub cron_tick_enabled: bool,
    /// Seconds between cron tick passes when the loop above is enabled.
    /// Defaults to 60.
    #[serde(default = "CpConfig::default_background_interval_secs")]
    pub cron_tick_interval_secs: u64,
    /// Whether `rupu cp serve` runs the gate sweep loop (Plan 4) in-process
    /// on a timer. The sweep enforces gate `on_timeout` routing, runs the
    /// `on_reject` cleanup chain for web-initiated timeout-rejects, and reaps
    /// orphaned local runs whose runner process died. Defaults to `true` so
    /// timed-out gates and dead-runner runs never wedge Live Events.
    #[serde(default = "CpConfig::default_true")]
    pub gate_sweep_enabled: bool,
    /// Seconds between gate sweep passes when the loop above is enabled.
    /// Defaults to 60.
    #[serde(default = "CpConfig::default_background_interval_secs")]
    pub gate_sweep_interval_secs: u64,
    /// Which agent-authoring UI the CP web app renders: "classic" (raw editor)
    /// or "next" (the card-based Agent Builder). Defaults to classic so the new
    /// UI is opt-in behind this flag.
    #[serde(default = "CpConfig::default_agent_authoring_ui")]
    pub agent_authoring_ui: String,
    /// Which workflow-editor UI the CP web app renders: "classic" (raw editor)
    /// or "next" (the visual branch-authoring UI). Defaults to classic so the new
    /// UI is opt-in behind this flag.
    #[serde(default = "CpConfig::default_workflow_editor_ui")]
    pub workflow_editor_ui: String,
}

impl CpConfig {
    fn default_true() -> bool {
        true
    }

    fn default_background_interval_secs() -> u64 {
        60
    }

    fn default_agent_authoring_ui() -> String {
        "classic".to_string()
    }

    fn default_workflow_editor_ui() -> String {
        "classic".to_string()
    }
}

impl Default for CpConfig {
    fn default() -> Self {
        Self {
            max_workspace_bytes: None,
            autoflow_reconcile_enabled: Self::default_true(),
            autoflow_reconcile_interval_secs: Self::default_background_interval_secs(),
            cron_tick_enabled: Self::default_true(),
            cron_tick_interval_secs: Self::default_background_interval_secs(),
            gate_sweep_enabled: Self::default_true(),
            gate_sweep_interval_secs: Self::default_background_interval_secs(),
            agent_authoring_ui: Self::default_agent_authoring_ui(),
            workflow_editor_ui: Self::default_workflow_editor_ui(),
        }
    }
}
