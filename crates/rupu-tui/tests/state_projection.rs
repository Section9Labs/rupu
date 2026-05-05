use rupu_tui::state::{NodeState, NodeStatus, TokenCounters};

#[test]
fn fresh_node_is_waiting_with_empty_tokens() {
    let n = NodeState::new("step-1", "spec-agent");
    assert_eq!(n.status, NodeStatus::Waiting);
    assert_eq!(n.tokens, TokenCounters::default());
    assert_eq!(n.tools_used.len(), 0);
    assert!(n.transcript_tail.is_empty());
}
