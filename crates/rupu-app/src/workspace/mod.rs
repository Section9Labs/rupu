//! Workspace data layer — pure Rust, no GPUI.

pub mod manifest;
// storage, discovery, recents added in later tasks.

pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
