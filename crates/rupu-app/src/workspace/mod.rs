//! Workspace data layer — pure Rust, no GPUI.

pub mod discovery;
pub mod manifest;
pub mod recents;
pub mod storage;
// handle added in next task.

pub use discovery::{Asset, AssetSet};
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
