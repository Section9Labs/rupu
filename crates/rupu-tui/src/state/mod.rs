mod node;
pub use node::{LastAction, NodeState, NodeStatus, TokenCounters};

use std::collections::BTreeMap;

use rupu_orchestrator::RunRecord;
use rupu_transcript::Event;

mod edges;
mod projection;
pub use edges::derive_edges;

#[derive(Debug, Clone, Default)]
pub struct RunModel {
    pub nodes: BTreeMap<String, NodeState>,
    pub run_record: Option<RunRecord>,
}

impl RunModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_node(&mut self, step_id: &str, agent: &str) -> &mut NodeState {
        self.nodes
            .entry(step_id.to_string())
            .or_insert_with(|| NodeState::new(step_id, agent))
    }

    pub fn node(&self, step_id: &str) -> Option<&NodeState> {
        self.nodes.get(step_id)
    }

    pub fn apply_event(&mut self, step_id: &str, ev: &Event) {
        let node = self.upsert_node(step_id, "");
        projection::apply(node, ev);
    }

    pub fn apply_run_update(&mut self, rec: RunRecord) {
        self.run_record = Some(rec);
    }
}
