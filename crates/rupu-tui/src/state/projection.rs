use rupu_transcript::{Event, RunStatus as TranscriptRunStatus};

use super::node::{LastAction, NodeState, NodeStatus};

/// How many leading lines of an AssistantMessage to keep in the
/// node's transcript_tail. The focused-node panel renders this many.
const TRANSCRIPT_LINES_PER_ASSISTANT_MESSAGE: usize = 3;

/// How many characters of a tool's input to keep in `LastAction.summary`.
/// Matches the focused-node panel's display width budget.
const TOOL_INPUT_SUMMARY_LEN: usize = 60;

pub(super) fn apply(node: &mut NodeState, ev: &Event) {
    match ev {
        Event::RunStart { agent, .. } => {
            if node.agent.is_empty() {
                node.agent = agent.clone();
            }
            node.status = NodeStatus::Active;
        }
        Event::TurnStart { .. } => {
            node.status = NodeStatus::Working;
            node.turn_idx = node.turn_idx.saturating_add(1);
        }
        Event::AssistantMessage { content, .. } => {
            for line in content.lines().take(TRANSCRIPT_LINES_PER_ASSISTANT_MESSAGE) {
                node.push_transcript_line(line.to_string());
            }
        }
        Event::ToolCall { tool, input, .. } => {
            *node.tools_used.entry(tool.clone()).or_insert(0) += 1;
            node.last_action = Some(LastAction {
                tool: tool.clone(),
                summary: summarize_input(input),
                duration_ms: None,
            });
        }
        Event::ToolResult { duration_ms, .. } => {
            if let Some(la) = node.last_action.as_mut() {
                la.duration_ms = Some(*duration_ms);
            }
        }
        Event::FileEdit { path, .. } => {
            *node.tools_used.entry("edit".into()).or_insert(0) += 1;
            node.last_action = Some(LastAction {
                tool: "edit".into(),
                summary: path.clone(),
                duration_ms: None,
            });
        }
        Event::CommandRun { argv, exit_code, .. } => {
            *node.tools_used.entry("bash".into()).or_insert(0) += 1;
            node.last_action = Some(LastAction {
                tool: "bash".into(),
                summary: format!("{} (exit {})", argv.join(" "), exit_code),
                duration_ms: None,
            });
        }
        Event::ActionEmitted { kind, allowed, .. } => {
            node.actions_emitted = node.actions_emitted.saturating_add(1);
            if !*allowed {
                node.denied_actions.push(kind.clone());
            }
        }
        Event::GateRequested { prompt, .. } => {
            node.status = NodeStatus::Awaiting;
            node.gate_prompt = Some(prompt.clone());
        }
        Event::TurnEnd { tokens_in, tokens_out, .. } => {
            if let Some(t) = tokens_in {
                node.tokens.input = node.tokens.input.saturating_add(*t);
            }
            if let Some(t) = tokens_out {
                node.tokens.output = node.tokens.output.saturating_add(*t);
            }
        }
        Event::Usage { input_tokens, output_tokens, cached_tokens, .. } => {
            node.tokens.input = node.tokens.input.saturating_add(u64::from(*input_tokens));
            node.tokens.output = node.tokens.output.saturating_add(u64::from(*output_tokens));
            node.tokens.cached = node.tokens.cached.saturating_add(u64::from(*cached_tokens));
        }
        Event::RunComplete { status, .. } => {
            node.status = match status {
                TranscriptRunStatus::Ok => NodeStatus::Complete,
                TranscriptRunStatus::Error | TranscriptRunStatus::Aborted => NodeStatus::Failed,
            };
        }
    }
}

fn summarize_input(v: &serde_json::Value) -> String {
    if let Some(cmd) = v.get("command").and_then(|x| x.as_str()) {
        return cmd.chars().take(TOOL_INPUT_SUMMARY_LEN).collect();
    }
    if let Some(path) = v.get("path").and_then(|x| x.as_str()) {
        return path.to_string();
    }
    v.to_string().chars().take(TOOL_INPUT_SUMMARY_LEN).collect()
}
