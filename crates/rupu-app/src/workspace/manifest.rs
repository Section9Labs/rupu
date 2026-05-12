//! Per-user workspace manifest. Persists to
//! `~/Library/Application Support/rupu.app/workspaces/<id>.toml`
//! (path resolution lives in `storage.rs`).
//!
//! The split between manifest (per-user) and project-dir `.rupu/`
//! (per-project) is deliberate: the project directory stays clean
//! and shareable; per-user UI state survives `git clean -fdx`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Top-level workspace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// ULID prefixed with `ws_` (matches existing `run_*` convention).
    pub id: String,
    /// User-chosen display name (defaults to directory's basename on create).
    pub name: String,
    /// User-chosen accent color (sidebar chip + titlebar chip).
    pub color: WorkspaceColor,
    /// Absolute path to the workspace directory.
    pub path: String,
    /// Last time the workspace was opened — drives "Open Recent" ordering.
    pub opened_at: DateTime<Utc>,
    /// Repo refs attached to this workspace (rendered in sidebar `repos`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoBinding>,
    /// rupu execution hosts. Always at least one (Local). v2 adds Mcp.
    #[serde(default = "default_attached_hosts")]
    pub attached_hosts: Vec<AttachedHost>,
    /// Persisted UI state (open tabs, collapsed sidebar sections, etc.).
    #[serde(default)]
    pub ui: UiState,
}

fn default_attached_hosts() -> Vec<AttachedHost> {
    vec![AttachedHost::Local]
}

/// One of five preset accent colors. Stored as a lowercase string so
/// the toml stays human-editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceColor {
    Purple,
    Blue,
    Green,
    Amber,
    Pink,
}

impl WorkspaceColor {
    pub fn to_rgba(self) -> gpui::Rgba {
        use crate::palette::*;
        match self {
            Self::Purple => CHIP_PURPLE,
            Self::Blue => CHIP_BLUE,
            Self::Green => CHIP_GREEN,
            Self::Amber => CHIP_AMBER,
            Self::Pink => CHIP_PINK,
        }
    }
}

/// One repo attached to the workspace. The string format mirrors
/// rupu-scm's `RepoRef::parse` input: `<platform>:<owner>/<repo>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoBinding {
    #[serde(rename = "ref")]
    pub r#ref: String,
}

/// rupu execution host. v1 only supports Local (in-process orchestrator).
/// v2 will add `Mcp { url, auth_key }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AttachedHost {
    Local,
    // v2:
    // Mcp { url: String, auth_key: String },
}

/// UI state persisted per workspace. Empty by default; populated as
/// later sub-slices add tabs / view picker / etc.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    /// Tab identifiers in left-to-right order. Format: `<kind>:<id>` —
    /// `workflow:review`, `file:src/lib.rs`, `run:run_01ABC...`, etc.
    /// Resolved by later sub-slices; for D-1 this is always empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_open_tabs: Vec<String>,
    /// Sidebar sections the user has collapsed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_collapsed_sections: Vec<String>,
    /// Active view per tab — populated by D-2+ as views land.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_view_per_tab: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_through_toml() {
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "rupu".into(),
            color: WorkspaceColor::Purple,
            path: "/Users/matt/Code/Oracle/rupu".into(),
            opened_at: chrono::DateTime::parse_from_rfc3339("2026-05-11T15:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            repos: vec![RepoBinding { r#ref: "github:Section9Labs/rupu".into() }],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        let parsed: WorkspaceManifest = toml::from_str(&s).expect("deserialize");
        assert_eq!(parsed, m);
    }

    #[test]
    fn manifest_omits_empty_ui_fields() {
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "minimal".into(),
            color: WorkspaceColor::Blue,
            path: "/tmp/x".into(),
            opened_at: chrono::Utc::now(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        assert!(!s.contains("last_open_tabs"), "empty tabs should be omitted: {s}");
        assert!(!s.contains("sidebar_collapsed_sections"), "empty collapsed should be omitted: {s}");
        assert!(!s.contains("repos"), "empty repos should be omitted: {s}");
    }
}
