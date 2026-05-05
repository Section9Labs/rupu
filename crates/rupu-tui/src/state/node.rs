use std::collections::BTreeMap;
use std::collections::VecDeque;

const TRANSCRIPT_TAIL_LEN: usize = 5;

/// Status of one DAG node, projected from transcript events + RunRecord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeStatus {
    #[default]
    Waiting,
    Active,
    Working,
    Complete,
    Failed,
    SoftFailed,
    Awaiting,
    Retrying,
    Skipped,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounters {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

#[derive(Debug, Clone)]
pub struct LastAction {
    pub tool: String,
    pub summary: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct NodeState {
    pub step_id: String,
    pub agent: String,
    pub status: NodeStatus,
    pub turn_idx: u32,
    pub tokens: TokenCounters,
    pub tools_used: BTreeMap<String, u32>,
    pub last_action: Option<LastAction>,
    pub transcript_tail: VecDeque<String>,
    pub gate_prompt: Option<String>,
    pub actions_emitted: u32,
    pub denied_actions: Vec<String>,
}

impl NodeState {
    pub fn new(step_id: impl Into<String>, agent: impl Into<String>) -> Self {
        Self {
            step_id: step_id.into(),
            agent: agent.into(),
            ..Self::default()
        }
    }

    pub fn push_transcript_line(&mut self, line: String) {
        if self.transcript_tail.len() == TRANSCRIPT_TAIL_LEN {
            self.transcript_tail.pop_front();
        }
        self.transcript_tail.push_back(line);
    }
}
