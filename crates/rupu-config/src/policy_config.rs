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
    /// **Deprecated, inert, and never read.** Used to select between "classic"
    /// and "next" agent-authoring UIs in the CP web app; the `next` UI is now
    /// the only UI and the classic renderer has been deleted (ISSUES.md I-28).
    ///
    /// The field survives the deletion ONLY as a migration shim. `CpConfig` is
    /// `deny_unknown_fields`, so an existing `config.toml` carrying this key
    /// would otherwise fail to deserialize — and several call paths load
    /// config with `.unwrap_or_default()`, which converts that parse error
    /// into the *silent loss of every other key the user set*. Accepting the
    /// key as an opaque no-op keeps the rest of the config intact; the
    /// warning in [`crate::Config::warn_deprecated_keys`] tells the user to
    /// delete it.
    ///
    /// `skip_serializing` keeps it out of `/api/config` and out of anything
    /// that round-trips `CpConfig` back to TOML, so the key disappears the
    /// first time a config is rewritten.
    #[serde(default, skip_serializing)]
    pub agent_authoring_ui: Option<toml::Value>,
    /// **Deprecated, inert, and never read.** Used to select between "classic"
    /// and "next" workflow-editor UIs in the CP web app; the `next` UI is now
    /// the only UI and the classic renderer has been deleted (ISSUES.md I-29).
    /// See `agent_authoring_ui` above for why the field survives as a shim.
    #[serde(default, skip_serializing)]
    pub workflow_editor_ui: Option<toml::Value>,
}

impl CpConfig {
    fn default_true() -> bool {
        true
    }

    fn default_background_interval_secs() -> u64 {
        60
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
            agent_authoring_ui: None,
            workflow_editor_ui: None,
        }
    }
}

/// `[workflow]` config block. Gates the `run:` step kind, which executes
/// declared commands and is therefore opt-in per workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowConfig {
    /// Whether `run:` steps may execute. Default `false`.
    ///
    /// A workflow containing a `run:` step FAILS when this is off — it
    /// does not silently skip the step. A benchmark that quietly omitted
    /// its scoring step would report a plausible-looking but meaningless
    /// result.
    pub run_step_enabled: bool,
    /// Optional allowlist of permitted executables, matched on basename.
    /// Empty ⇒ any executable is permitted (when `run_step_enabled`).
    pub run_step_allowlist: Vec<String>,
}

impl WorkflowConfig {
    /// True if `cmd` may be executed. Matches on basename so an absolute
    /// path cannot bypass the list (`/bin/bash` and `bash` gate alike).
    pub fn allows(&self, cmd: &str) -> bool {
        if self.run_step_allowlist.is_empty() {
            return true;
        }
        let base = std::path::Path::new(cmd)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd);
        self.run_step_allowlist.iter().any(|a| a == base)
    }
}

#[cfg(test)]
mod workflow_config_tests {
    use super::*;

    #[test]
    fn run_step_is_disabled_by_default() {
        assert!(
            !WorkflowConfig::default().run_step_enabled,
            "run: executes arbitrary commands; it must be opt-in"
        );
    }

    #[test]
    fn empty_allowlist_permits_any_executable_when_enabled() {
        let c = WorkflowConfig {
            run_step_enabled: true,
            run_step_allowlist: vec![],
        };
        assert!(c.allows("python3"));
    }

    #[test]
    fn allowlist_matches_on_basename_not_path() {
        // Otherwise the allowlist is trivially bypassed by absolute path.
        let c = WorkflowConfig {
            run_step_enabled: true,
            run_step_allowlist: vec!["python3".into()],
        };
        assert!(c.allows("python3"));
        assert!(c.allows("/usr/bin/python3"));
        assert!(!c.allows("bash"));
        assert!(!c.allows("/bin/bash"));
    }

    #[test]
    fn round_trips_through_toml() {
        let c = WorkflowConfig {
            run_step_enabled: true,
            run_step_allowlist: vec!["python3".into(), "make".into()],
        };
        let s = toml::to_string(&c).unwrap();
        assert_eq!(toml::from_str::<WorkflowConfig>(&s).unwrap(), c);
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(
            toml::from_str::<WorkflowConfig>("run_step_enabled = true\nrun_step_shell = true\n")
                .is_err(),
            "deny_unknown_fields must reject a typo rather than ignore it"
        );
    }
}
