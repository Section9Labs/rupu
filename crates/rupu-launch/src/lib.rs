//! rupu-launch — the one way a run starts and watches its children (spec
//! 2026-10-07 W7): [`RunLauncher`] implements `rupu_tools::Launcher` (and
//! `WorkflowGenerator`) for every child kind — an in-process `subagent`, an
//! `agent` process, a `workflow` process — under one policy (allowlist,
//! depth, permission ceiling, concurrency), one codename scheme, one usage
//! rollup and one children ledger ([`ledger`], `<parent run dir>/
//! children.jsonl`). [`reaper`] stops the process children of a parent that
//! died.

pub mod factory;
pub mod generate;
pub mod identity;
pub mod launcher;
pub mod ledger;
pub mod proc;
pub mod process;
pub mod reaper;
pub mod testing;

pub use factory::Launchers;
pub use generate::{Generation, GenerationProviderFactory};
pub use launcher::{LaunchEnv, RunLauncher, DEFAULT_MAX_CONCURRENT_CHILDREN, TERM_GRACE};
pub use process::{
    ProcOutcome, ProcState, ProcessBackend, ProcessLaunch, SpawnedProcess, Stop, Subprocess,
};
