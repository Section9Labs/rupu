//! Executor — the live-run surface for `rupu.app` (Slice D Plan 3).
//!
//! `WorkflowExecutor` is the trait. `InProcessExecutor` runs workflows
//! in a tokio task and fans events through any number of `EventSink`s
//! (`InMemorySink` for live broadcast, `JsonlSink` for on-disk
//! `events.jsonl`). `FileTailRunSource` consumes `events.jsonl` for
//! runs the executor didn't start (CLI, cron, MCP).

pub mod errors;
pub mod event;
pub mod sink;
pub mod jsonl_sink;
pub mod in_memory_sink;
pub mod in_process;
pub mod file_tail;

pub use errors::ExecutorError;
