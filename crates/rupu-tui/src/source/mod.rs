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
    RunUpdate(RunRecord),
    Tick,
}

pub trait EventSource: Send {
    fn poll(&mut self) -> Vec<SourceEvent>;
    fn wait(&mut self, dur: Duration) -> Option<SourceEvent>;
}
