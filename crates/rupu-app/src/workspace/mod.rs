//! Workspace data layer — pure Rust, no GPUI.

pub mod manifest;
pub mod storage;
// discovery, recents, handle added in later tasks.

pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
