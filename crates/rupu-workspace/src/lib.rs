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
pub mod autoflow_worktree;
pub mod record;
pub mod repo_record;
pub mod repo_store;
pub mod worktree_layout;

// `discover` and the upsert logic land in Tasks 10-11; the modules
// exist here so the lib's public surface is stable from the skeleton
// stage. The function signatures of `discover` and `upsert` will not
// change between skeleton and implementation.
pub mod discover;
pub mod host_store;
pub mod store;
pub mod worker_store;
pub mod workspace_sync;

pub use autoflow_claim::{AutoflowClaimRecord, AutoflowContender, ClaimStatus, PendingDispatch};
pub use autoflow_claim_store::{
    ActiveLockRecord, AutoflowClaimStore, ClaimLockGuard, ClaimStoreError,
};
pub use autoflow_worktree::{
    ensure_issue_worktree, remove_issue_worktree, AutoflowWorktree, AutoflowWorktreeError,
};
pub use discover::{discover, DiscoverError, Discovery};
pub use host_store::{
    add_bucket_host, add_ssh_host, delete_host_token, enroll_node, get_host_token, set_host_token,
    verify_node_token, Host, HostStatus, HostStore, HostStoreError, HostTransport,
};
pub use record::{new_id, Workspace};
pub use repo_record::TrackedRepo;
pub use repo_store::{repo_ref_key, RepoRegistryStore, RepoStoreError};
pub use rupu_runtime::{WorkerCapabilities, WorkerKind, WorkerRecord};
pub use store::{upsert, StoreError, WorkspaceStore};
pub use worker_store::{WorkerStore, WorkerStoreError};
pub use workspace_sync::{
    apply_deltas, collect_delta, detect_mode, pack, stage, Baseline, Delta, Payload, SyncError,
    SyncMode,
};
pub use worktree_layout::{issue_dir_name, issue_worktree_path, repo_dir_name};
