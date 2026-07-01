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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CpConfig {
    /// Max bytes for a workspace-sync payload/delta. `None` ⇒ the CP's
    /// `MAX_WORKSPACE_BYTES` default.
    pub max_workspace_bytes: Option<u64>,
}
