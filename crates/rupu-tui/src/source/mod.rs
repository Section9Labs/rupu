use std::time::Duration;

use rupu_orchestrator::RunRecord;
use rupu_transcript::Event;

mod jsonl_tail;
pub use jsonl_tail::JsonlTailSource;

mod replay;
pub use replay::ReplaySource;

#[derive(Debug, Clone)]
pub enum SourceEvent {
    StepEvent { step_id: String, event: Event },
    RunUpdate(Box<RunRecord>),
    Tick,
}

pub trait EventSource: Send {
    fn poll(&mut self) -> Vec<SourceEvent>;
    fn wait(&mut self, dur: Duration) -> Option<SourceEvent>;
    /// Replay sources signal completion via this; live sources return
    /// false. Default is `false` so existing impls don't break.
    fn is_drained(&self) -> bool {
        false
    }
}
