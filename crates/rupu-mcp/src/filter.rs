//! Which tools a server lists. Task 1 keeps everything; the grammar
//! (`parse`) arrives with the serve flags.

/// A selection over tool names.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    _private: (),
}

impl ToolFilter {
    /// Keeps every tool.
    pub fn all() -> Self {
        Self::default()
    }

    /// Whether `name` is kept.
    pub fn apply(&self, _name: &str) -> bool {
        true
    }
}
