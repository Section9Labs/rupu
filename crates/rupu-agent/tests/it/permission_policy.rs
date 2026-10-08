//! The agent loop's permission path (W1): every call is decided by the
//! `PermissionPolicy` from the tool's effect, legacy names still resolve, and
//! ask mode without an operator says so once.

use crate::findings_without_coverage::opts_for;
use rupu_agent::run_agent;
use rupu_agent::runner::ScriptedTurn;
use rupu_providers::types::StopReason;
use rupu_tools::{PermissionMode, PermissionPolicy, PromptAnswer, PromptRequest, Prompter};
use rupu_transcript::{Event, JsonlReader};
use std::sync::{Arc, Mutex};

/// Answers every prompt with `answer`, recording the tools it was asked about.
struct Recording {
    answer: PromptAnswer,
    asked: Mutex<Vec<String>>,
}

impl Prompter for Recording {
    fn ask(&self, req: &PromptRequest<'_>) -> PromptAnswer {
        self.asked.lock().unwrap().push(req.tool.to_string());
        self.answer
    }
}

fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: name.into(),
        tool_input: input,
        stop: StopReason::ToolUse,
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "done".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

fn events(path: &std::path::Path) -> Vec<Event> {
    JsonlReader::iter(path)
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

fn tool_result_errors(evs: &[Event]) -> Vec<Option<String>> {
    evs.iter()
        .filter_map(|e| match e {
            Event::ToolResult { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn ask_prompts_for_mcp_write() {
    // T3: under ask with an operator, an External MCP tool reaches the
    // prompter; before W1 it ran unprompted.
    let tmp = tempfile::TempDir::new().unwrap();
    let prompter = Arc::new(Recording {
        answer: PromptAnswer::Deny,
        asked: Mutex::new(Vec::new()),
    });
    let mut opts = opts_for(
        tmp.path(),
        None,
        vec![
            tool_use(
                "c1",
                "issues.create",
                serde_json::json!({"repo": "acme/widgets", "title": "t", "body": "b"}),
            ),
            done(),
        ],
    );
    opts.permission = PermissionPolicy::new(PermissionMode::Ask, Some(prompter.clone()));
    opts.tool_context.services.scm = Some(Arc::new(rupu_scm::Registry::default()));
    // The registry adds the connector tools: resolve the grant over it.
    let opts = rupu_agent::grant::with_grant(opts, None, &[]).expect("grant");
    let transcript = opts.transcript_path.clone();
    run_agent(opts)
        .await
        .expect("a denied call does not fail the run");

    assert_eq!(*prompter.asked.lock().unwrap(), ["issues.create"]);
    assert_eq!(
        tool_result_errors(&events(&transcript)),
        [Some("permission_denied".to_string())]
    );
}

#[tokio::test]
async fn degraded_notice_once() {
    // D5: ask with no operator allows writes and says so exactly once.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut opts = opts_for(
        tmp.path(),
        None,
        vec![
            tool_use(
                "w1",
                "write_file",
                serde_json::json!({"path": "a.txt", "content": "a"}),
            ),
            tool_use(
                "w2",
                "write_file",
                serde_json::json!({"path": "b.txt", "content": "b"}),
            ),
            done(),
        ],
    );
    opts.permission = PermissionPolicy::unattended(PermissionMode::Ask);
    let transcript = opts.transcript_path.clone();
    run_agent(opts).await.expect("run succeeds");

    let evs = events(&transcript);
    let notices = evs
        .iter()
        .filter(|e| matches!(e, Event::Notice { kind, .. } if kind == "permission_mode_degraded"))
        .count();
    assert_eq!(notices, 1, "{evs:?}");
    assert!(tmp.path().join("a.txt").exists() && tmp.path().join("b.txt").exists());
}

#[tokio::test]
async fn readonly_still_allows_record_tools() {
    // D4: readonly reviewers must still report findings.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut opts = opts_for(
        tmp.path(),
        Some(vec![
            "findings.report".to_string(),
            "write_file".to_string(),
        ]),
        vec![
            tool_use(
                "f1",
                "findings.report",
                serde_json::json!({
                    "scope": "repo",
                    "summary": "s",
                    "severity": "low",
                    "evidence": {"rationale": "r"}
                }),
            ),
            tool_use(
                "w1",
                "write_file",
                serde_json::json!({"path": "a.txt", "content": "a"}),
            ),
            done(),
        ],
    );
    opts.permission = PermissionPolicy::unattended(PermissionMode::Readonly);
    let transcript = opts.transcript_path.clone();
    run_agent(opts).await.expect("run succeeds");

    let errs = tool_result_errors(&events(&transcript));
    assert_eq!(errs[0], None, "findings.report is Record: allowed");
    assert_eq!(errs[1].as_deref(), Some("permission_denied"));
    assert!(!tmp.path().join("a.txt").exists());
}

#[tokio::test]
async fn alias_call_resolves() {
    // A model calling the legacy `report_finding` executes `findings.report`;
    // the transcript keeps the name the model used.
    let tmp = tempfile::TempDir::new().unwrap();
    let opts = opts_for(
        tmp.path(),
        Some(vec!["report_finding".to_string()]),
        vec![
            tool_use(
                "f1",
                "report_finding",
                serde_json::json!({
                    "scope": "repo",
                    "summary": "s",
                    "severity": "low",
                    "evidence": {"rationale": "r"}
                }),
            ),
            done(),
        ],
    );
    let transcript = opts.transcript_path.clone();
    run_agent(opts).await.expect("run succeeds");

    let evs = events(&transcript);
    let called: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            Event::ToolCall { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(called, ["report_finding"]);
    let outputs: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            Event::ToolResult {
                output,
                error: None,
                ..
            } => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        outputs.iter().any(|o| o.starts_with("finding_id: ")),
        "{evs:?}"
    );
}
