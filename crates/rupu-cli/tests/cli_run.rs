use assert_fs::prelude::*;
use std::process::Command;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "Hello from the mock provider.", "stop": "end_turn" } }
]
"#;

/// Shared by both the parent and child agent in
/// `rupu_run_bare_dispatches_child_agent_via_dispatch_agent_tool`. Each
/// process-side `MockProvider` instance is built fresh from this same
/// `RUPU_MOCK_PROVIDER_SCRIPT` JSON (one for the parent's own agent
/// loop, one for the child's, built independently inside
/// `CliAgentDispatcher::dispatch`) — so both turn sequences start at
/// index 0 of this same two-turn script:
///   turn 0: a `dispatch_agent` tool_use targeting `child`.
///   turn 1: a final assistant text.
/// For the parent this drives a real dispatch. For the child, the
/// same turn-0 tool_use is scripted again, but the child agent's own
/// frontmatter declares no `dispatchableAgents`, so `dispatch_agent`'s
/// own allowlist check rejects the self-referential call with a
/// (non-fatal) tool_result error rather than recursing — the loop
/// then serves turn 1 and the child finishes normally.
const DISPATCH_MOCK_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "dispatch_agent", "tool_input": {"agent": "child", "prompt": "please do the subtask"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "All done.", "stop": "end_turn" } }
]
"#;

fn init_git_checkout(path: &std::path::Path, origin_url: &str) {
    let status = Command::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", "origin", origin_url])
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_writes_transcript_under_mock_provider() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str(
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 1\n---\nyou echo.",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    // Force PWD to project so workspace discovery uses it.
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu run should exit 0 when the mock provider succeeds"
    );

    // Find the transcript file written under <global>/transcripts/<run_id>.jsonl
    let transcripts = global.child("transcripts");
    let entries: Vec<_> = std::fs::read_dir(transcripts.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one transcript file");
    let summary = rupu_transcript::JsonlReader::summary(entries[0].path()).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_unknown_agent_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let exit = rupu_cli::run(vec!["rupu".into(), "run".into(), "nonexistent".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "unknown agent should not exit 0"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_auto_tracks_current_checkout() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str(
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 1\n---\nyou echo.",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    init_git_checkout(project.path(), "git@github.com:Section9Labs/rupu.git");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(exit, std::process::ExitCode::from(0));

    let store = rupu_workspace::RepoRegistryStore {
        root: global.path().join("repos"),
    };
    let tracked = store
        .load("github:Section9Labs/rupu")
        .unwrap()
        .expect("repo should be auto-tracked");
    assert_eq!(tracked.repo_ref, "github:Section9Labs/rupu");
    assert_eq!(tracked.known_paths.len(), 1);
    assert_eq!(
        tracked.preferred_path,
        project.path().canonicalize().unwrap().display().to_string()
    );

    let transcript_path = std::fs::read_dir(global.child("transcripts").path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .expect("standalone transcript should exist");
    let summary = rupu_transcript::JsonlReader::summary(&transcript_path).unwrap();
    let metadata_path = rupu_cli::standalone_run_metadata::metadata_path_for_run(
        global.child("transcripts").path(),
        &summary.run_id,
    );
    let metadata = rupu_cli::standalone_run_metadata::read_metadata(&metadata_path).unwrap();
    assert_eq!(metadata.run_id, summary.run_id);
    assert_eq!(
        metadata.repo_ref.as_deref(),
        Some("github:Section9Labs/rupu")
    );
    assert_eq!(metadata.backend_id, "local_checkout");
    assert_eq!(metadata.trigger_source, "run_cli");
    assert_eq!(
        metadata.workspace_strategy.as_deref(),
        Some("direct_checkout")
    );
    assert!(metadata.target.is_none());
    assert_eq!(
        metadata.workspace_path,
        project.path().canonicalize().unwrap()
    );
    assert!(metadata
        .worker_id
        .as_deref()
        .is_some_and(|value| value.starts_with("worker_local_")));
}

/// Proves the wiring this change adds: a bare `rupu run` (no workflow
/// involved) of an agent with `tools: [dispatch_agent]` +
/// `dispatchableAgents:` actually dispatches a child sub-run, exactly
/// like `rupu workflow run` already does per-step. Before this change
/// `run.rs` built `ToolContext { dispatcher: None, .. }` for bare runs,
/// so `dispatch_agent` always returned `dispatcher_not_configured`; the
/// tool call below would produce that error string in `child` position
/// and no `sub/` directory would ever be created.
#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_bare_dispatches_child_agent_via_dispatch_agent_tool() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/parent.md")
        .write_str(
            "---\nname: parent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n\
             maxTurns: 4\ntools: [dispatch_agent]\ndispatchableAgents: [child]\n---\n\
             you dispatch a child agent to do the subtask.",
        )
        .unwrap();
    global
        .child("agents/child.md")
        .write_str(
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\n\
             maxTurns: 4\n---\nyou are the child agent.",
        )
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", DISPATCH_MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let run_id = "run_bare_dispatch_test_1";
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "parent".into(),
        "--mode".into(),
        "bypass".into(),
        "--run-id".into(),
        run_id.into(),
        "go".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu run of a dispatching agent should still exit 0"
    );

    // Parent transcript completed OK.
    let parent_transcript = global.child("transcripts").child(format!("{run_id}.jsonl"));
    let parent_summary = rupu_transcript::JsonlReader::summary(parent_transcript.path()).unwrap();
    assert_eq!(parent_summary.status, rupu_transcript::RunStatus::Ok);

    // The dispatch created a child sub-run under
    // `<runs_root>/<run_id>/sub/<sub_run_id>/transcript.jsonl` —
    // exactly the layout `CliAgentDispatcher::dispatch` and
    // `RunStore::create_sub_run` document (spec § 5.1). This directory
    // only exists if `ToolContext.dispatcher` was actually `Some(..)`
    // for the bare run — the thing this change wires up.
    let sub_dir = global.child("runs").child(run_id).child("sub");
    assert!(
        sub_dir.path().is_dir(),
        "expected a sub/ directory under the parent run once dispatch_agent fired"
    );
    let sub_run_entries: Vec<_> = std::fs::read_dir(sub_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        sub_run_entries.len(),
        1,
        "expected exactly one child sub-run directory"
    );
    let child_transcript = sub_run_entries[0].path().join("transcript.jsonl");
    assert!(
        child_transcript.is_file(),
        "expected the child sub-run's transcript.jsonl to exist"
    );
    let child_summary = rupu_transcript::JsonlReader::summary(&child_transcript).unwrap();
    assert_eq!(
        child_summary.status,
        rupu_transcript::RunStatus::Ok,
        "the dispatched child agent should also complete OK against the mock provider"
    );
}
