//! Status of one DAG node. Lifted from rupu-tui::state::NodeStatus
//! with one addition: this version returns RGB tuples for status
//! colors so the consuming GPUI layer (rupu-app) can convert to
//! `gpui::Rgba` without pulling GPUI into this crate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    #[default]
    Waiting,
    Active,
    Working,
    Complete,
    Failed,
    SoftFailed,
    Awaiting,
    Retrying,
    Skipped,
}

impl NodeStatus {
    /// Single-glyph identifier matching the line-stream printer's
    /// vocabulary: `○ ● ◐ ✓ ✗ ⏸ ⊘ ↻`.
    pub fn glyph(self) -> char {
        match self {
            Self::Waiting => '○',
            Self::Active => '●',
            Self::Working => '◐',
            Self::Complete => '✓',
            Self::Failed => '✗',
            Self::SoftFailed => '✗',
            Self::Awaiting => '⏸',
            Self::Retrying => '↻',
            Self::Skipped => '⊘',
        }
    }

    /// Foreground color as a 24-bit RGB tuple. Mirrors the Okesu
    /// palette in rupu-cli/output/palette.rs + rupu-app/src/palette.rs.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Waiting => (82, 82, 91),       // slate-500 (dim)
            Self::Active => (59, 130, 246),      // blue-500
            Self::Working => (59, 130, 246),     // blue-500
            Self::Complete => (34, 197, 94),     // green-500
            Self::Failed => (239, 68, 68),       // red-500
            Self::SoftFailed => (202, 138, 4),   // yellow-600
            Self::Awaiting => (251, 191, 36),    // amber-400
            Self::Retrying => (124, 58, 237),    // brand-500
            Self::Skipped => (203, 213, 225),    // slate-300
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_glyph_is_checkmark() {
        assert_eq!(NodeStatus::Complete.glyph(), '✓');
    }

    #[test]
    fn failed_color_is_red_500() {
        assert_eq!(NodeStatus::Failed.rgb(), (239, 68, 68));
    }

    #[test]
    fn default_is_waiting() {
        assert_eq!(NodeStatus::default(), NodeStatus::Waiting);
    }

    #[test]
    fn waiting_glyph_is_hollow_circle() {
        // The empty/pending state is `○` (hollow), so an unstarted
        // workflow doesn't look like every node is running.
        // Active/Working share `●`/`◐` once data flows.
        assert_eq!(NodeStatus::Waiting.glyph(), '○');
    }
}
