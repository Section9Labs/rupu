//! `rupu run` wires the agent's `fallbacks:` chain and a real hop builder into
//! the runner: a refused turn hops to the chain's model and the run finishes.

use crate::ENV_LOCK;
use assert_fs::prelude::*;

/// The origin provider (`claude-sonnet-4-6`) refuses; the hop's provider,
/// built for `mock-2`, replays its own script and answers.
const MOCK_SCRIPT: &str = r#"
{
  "turns": [
    { "Reply": {
        "content": [{ "type": "text", "text": "I won't do that." }],
        "stop": { "reason": "refusal", "wire": { "provider": "anthropic", "value": "refusal" } }
    } }
  ],
  "models": {
    "mock-2": [
      { "AssistantText": { "text": "Done on the fallback model.", "stop": "end_turn" } }
    ]
  }
}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_falls_back_to_the_agents_chain_after_a_refusal() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/guarded.md")
        .write_str(
            "---\nname: guarded\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n\
             fallbacks:\n  - model: mock-2\n---\nyou answer.",
        )
        .unwrap();
    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "guarded".into(),
        "--mode".into(),
        "bypass".into(),
        "do the thing".into(),
    ])
    .await;

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "a refusal with a fallback chain should finish on the hop"
    );

    let entries: Vec<_> = std::fs::read_dir(global.child("transcripts").path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one transcript");
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(entries[0].path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let fell_back = events.iter().find_map(|e| match e {
        rupu_transcript::Event::Recovery {
            action: rupu_transcript::RecoveryAction::FellBack,
            model,
            ..
        } => Some(model.clone()),
        _ => None,
    });
    assert_eq!(
        fell_back,
        Some(Some("mock-2".to_string())),
        "the transcript should record the hop to mock-2: {events:#?}"
    );
    let summary = rupu_transcript::JsonlReader::summary(entries[0].path()).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}
