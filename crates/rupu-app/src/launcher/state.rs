//! LauncherState — populated in Task 2.
//!
//! The full struct + enums + validation logic land in the next task.
//! This stub provides the type names exported from `mod.rs` so the
//! crate compiles.

#[derive(Debug, Clone)]
pub enum LauncherMode {
    Ask,
}

#[derive(Debug, Clone)]
pub enum LauncherTarget {
    ThisWorkspace,
}

#[derive(Debug, Clone)]
pub enum CloneStatus {
    NotStarted,
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
}

pub struct LauncherState;
