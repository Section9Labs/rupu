//! Workspace data layer — pure Rust, no GPUI.

pub mod discovery;
pub mod handle;
pub mod manifest;
pub mod recents;
pub mod storage;

pub use discovery::{Asset, AssetSet};
pub use handle::Workspace;
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
