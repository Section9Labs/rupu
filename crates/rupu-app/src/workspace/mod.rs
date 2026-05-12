//! Workspace data layer — pure Rust, no GPUI.

pub mod discovery;
pub mod manifest;
pub mod storage;
// recents, handle added in later tasks.

pub use discovery::{Asset, AssetSet};
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
