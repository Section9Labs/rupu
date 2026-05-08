//! rupu-workspace — workspace discovery and on-disk records.
//!
//! A "workspace" is a working directory rupu has been invoked from,
//! plus the metadata it discovered (canonical path, git remote,
//! default branch). The record store lives at `~/.rupu/workspaces/`;
//! one TOML file per workspace, keyed by ULID id. The agent runtime
//! upserts the record on each `rupu run` so transcripts always carry
//! a stable `workspace_id`.

pub mod autoflow_claim;
pub mod autoflow_claim_store;
pub mod record;
pub mod repo_record;
pub mod repo_store;
pub mod worktree_layout;

// `discover` and the upsert logic land in Tasks 10-11; the modules
// exist here so the lib's public surface is stable from the skeleton
// stage. The function signatures of `discover` and `upsert` will not
// change between skeleton and implementation.
pub mod discover;
pub mod store;

pub use autoflow_claim::{AutoflowClaimRecord, ClaimStatus, PendingDispatch};
pub use autoflow_claim_store::{
    ActiveLockRecord, AutoflowClaimStore, ClaimLockGuard, ClaimStoreError,
};
pub use discover::{discover, DiscoverError, Discovery};
pub use record::{new_id, Workspace};
pub use repo_record::TrackedRepo;
pub use repo_store::{repo_ref_key, RepoRegistryStore, RepoStoreError};
pub use store::{upsert, StoreError, WorkspaceStore};
pub use worktree_layout::{issue_dir_name, issue_worktree_path, repo_dir_name};
