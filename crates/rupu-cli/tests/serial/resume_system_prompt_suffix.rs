//! W3, R8: a resumed workflow run keeps its `## Run target` system-prompt
//! section. The launch records it on the run (`RunRecord.system_prompt_suffix`)
//! and both resume paths — `rupu workflow approve` (through
//! `resume::rebuild_opts_from_disk`) and `rupu workflow resume`
//! (`cmd::workflow::resume_run`) — hand it back to the step factory. Before,
//! every resumed step ran without it.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_orchestrator::{RunStatus, RunStore};

const WORKFLOW_GATE_THEN_STEP: &str = r#"name: target-wf
steps:
  - id: gate
    approval:
      prompt: "Go?"
  - id: after
    agent: echo
    actions: []
    prompt: carry on
"#;

const MOCK_SCRIPT: &str = r#"[{ "AssistantText": { "text": "done", "stop": "end_turn" } }]"#;

/// What a launch with a target records (the parked run is given it directly:
/// a `--target` launch would clone or fetch over the network).
const TARGET: &str = "Repository: acme/widgets (github)";

struct Fixture {
    _tmp: assert_fs::TempDir,
    store: RunStore,
    transcripts: std::path::PathBuf,
}

async fn parked_with_a_target(run_id: &str) -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global
        .child("workflows/target-wf.yaml")
        .write_str(WORKFLOW_GATE_THEN_STEP)
        .unwrap();
    let project = tmp.child("proj");
    project.create_dir_all().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    rupu(&[
        "workflow",
        "run",
        "target-wf",
        "--mode",
        "bypass",
        "--run-id",
        run_id,
        "--plain",
    ])
    .await;

    let store = RunStore::new(global.path().join("runs"));
    let mut rec = store.load(run_id).expect("the launch created the run");
    assert_eq!(rec.status, RunStatus::AwaitingApproval);
    assert_eq!(
        rec.system_prompt_suffix, None,
        "no target, nothing recorded"
    );
    rec.system_prompt_suffix = Some(TARGET.into());
    store.update(&rec).unwrap();
    Fixture {
        transcripts: rec.transcript_dir.clone(),
        _tmp: tmp,
        store,
    }
}

async fn rupu(args: &[&str]) {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let _exit = rupu_cli::run(argv).await;
}

/// The resumed step's recorded system prompt carries the run's target.
fn assert_step_saw_the_target(fx: &Fixture, run_id: &str) {
    let rec = fx.store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed, "{:?}", rec.error_message);
    let mut prompts = Vec::new();
    for entry in std::fs::read_dir(&fx.transcripts).unwrap().flatten() {
        let path = entry.path();
        let Ok(iter) = rupu_transcript::JsonlReader::iter(&path) else {
            continue;
        };
        for ev in iter.flatten() {
            if let rupu_transcript::Event::RunStart {
                system_prompt: Some(p),
                ..
            } = ev
            {
                prompts.push(p);
            }
        }
    }
    assert!(
        prompts
            .iter()
            .any(|p| p.contains(&format!("## Run target\n\n{TARGET}"))),
        "the resumed step's system prompt must keep the run target: {prompts:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn approve_resume_keeps_system_prompt_suffix() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_suffix_approve";
    let fx = parked_with_a_target(run_id).await;
    rupu(&["workflow", "approve", run_id, "--mode", "bypass"]).await;
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    assert_step_saw_the_target(&fx, run_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_keeps_system_prompt_suffix() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_suffix_resume";
    let fx = parked_with_a_target(run_id).await;
    fx.store.set_pause_marker(run_id).unwrap();
    rupu(&["workflow", "approve", run_id, "--mode", "bypass"]).await;
    assert_eq!(fx.store.load(run_id).unwrap().status, RunStatus::Paused);
    rupu(&["workflow", "resume", run_id, "--mode", "bypass", "--plain"]).await;
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    assert_step_saw_the_target(&fx, run_id);
}
