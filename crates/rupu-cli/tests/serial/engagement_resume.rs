//! A workflow run launched under `--engagement-profile` keeps that
//! engagement across a resume. The selection is recorded on the run
//! (`RunRecord.engagement_profiles`) and every resume path re-resolves it:
//! `rupu workflow approve` and the cp-serve worker's `workflow resume` for a
//! web approve (both through `resume::rebuild_opts_from_disk`), and
//! `rupu workflow resume` of a paused run (`cmd::workflow::resume_run`).
//! Before, a resumed step ran on the native `code` path.
//!
//! The binding effect: the step after the gate calls `asset_mark`, which the
//! engine grants only under an active engagement and which writes the
//! engagement asset ledger. With the engagement restored, the asset lands at
//! the marked depth; without it, the tool is not offered and the ledger
//! stays empty.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_orchestrator::{RunStatus, RunStore};

/// The gate comes first, so the launch parks without a provider call; only
/// the resumed `mark` step reaches the model.
const WORKFLOW_GATE_THEN_MARK: &str = r#"name: eng-wf
steps:
  - id: gate
    approval:
      prompt: "Go?"
  - id: mark
    agent: marker
    actions: []
    prompt: mark the service
"#;

/// The `mark` step marks one `network:service` asset `tested`, then ends.
const MOCK_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "asset_mark", "tool_input": {
      "kind": "network:service",
      "coordinates": [ { "t": "host", "v": "10.0.0.5" }, { "t": "port", "v": { "number": 22, "proto": "tcp" } } ],
      "depth": "tested"
  }, "stop": "tool_use" } },
  { "AssistantText": { "text": "marked", "stop": "end_turn" } }
]
"#;

struct Fixture {
    _tmp: assert_fs::TempDir,
    project: std::path::PathBuf,
    store: RunStore,
}

/// `<tmp>/.rupu` as `RUPU_HOME` with the `marker` agent + `eng-wf` workflow,
/// cwd in `<tmp>/proj`, and a run of `eng-wf` launched under the `network`
/// profile, parked at its gate.
async fn parked_under_network(run_id: &str) -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global
        .child("agents/marker.md")
        .write_str(
            "---\nname: marker\nprovider: anthropic\nmodel: claude-sonnet-4-6\ntools: [report_finding]\n---\nyou mark assets.",
        )
        .unwrap();
    global
        .child("workflows/eng-wf.yaml")
        .write_str(WORKFLOW_GATE_THEN_MARK)
        .unwrap();
    let project = tmp.child("proj");
    project.create_dir_all().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    rupu(&[
        "workflow",
        "run",
        "eng-wf",
        "--engagement-profile",
        "network",
        "--mode",
        "bypass",
        "--run-id",
        run_id,
        "--plain",
    ])
    .await;

    let store = RunStore::new(global.path().join("runs"));
    let rec = store.load(run_id).expect("the launch created the run");
    assert_eq!(
        rec.status,
        RunStatus::AwaitingApproval,
        "parked at the gate"
    );
    assert_eq!(
        rec.engagement_profiles,
        ["network"],
        "the launch records its engagement on the run"
    );
    Fixture {
        project: project.path().to_path_buf(),
        _tmp: tmp,
        store,
    }
}

async fn rupu(args: &[&str]) {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let _exit = rupu_cli::run(argv).await;
}

/// The resumed `mark` step ran under the `network` engagement: its
/// `asset_mark` was offered and recorded the asset at `tested`.
fn assert_marked_under_engagement(fx: &Fixture, run_id: &str) {
    let rec = fx.store.load(run_id).unwrap();
    assert_eq!(
        rec.status,
        RunStatus::Completed,
        "the resumed run completes: {:?}",
        rec.error_message
    );
    assert_eq!(rec.engagement_profiles, ["network"], "still recorded");
    let paths = rupu_coverage::CoveragePaths::new(
        &fx.project,
        &rupu_coverage::target_id(&fx.project, "eng-wf"),
    );
    let assets = rupu_coverage::read_assets(&paths.assets).unwrap_or_default();
    assert_eq!(
        assets.len(),
        1,
        "the resumed step's asset_mark must reach the engagement asset ledger \
         — it is only offered under the run's engagement"
    );
    assert_eq!(assets[0].kind, "network:service");
    assert_eq!(assets[0].depth.as_deref(), Some("tested"));
}

/// `rupu workflow approve` resumes the run under its recorded engagement.
#[tokio::test(flavor = "multi_thread")]
async fn approve_resumes_under_the_recorded_engagement() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_engagement_approve";
    let fx = parked_under_network(run_id).await;

    rupu(&["workflow", "approve", run_id, "--mode", "bypass"]).await;
    assert_marked_under_engagement(&fx, run_id);
}

/// A web approve (the CP records the decision; the cp-serve resume worker
/// spawns `workflow resume`) resumes the run under its recorded engagement.
#[tokio::test(flavor = "multi_thread")]
async fn web_approve_resume_keeps_the_recorded_engagement() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_engagement_web_approve";
    let fx = parked_under_network(run_id).await;

    fx.store
        .request_resume_approval(run_id, "web", Some("bypass"), chrono::Utc::now(), None)
        .unwrap();
    rupu(&["workflow", "resume", run_id, "--plain"]).await;
    assert_marked_under_engagement(&fx, run_id);
}

/// `rupu workflow resume` of a run paused before its post-gate step
/// (`cmd::workflow::resume_run`) resumes it under its recorded engagement.
#[tokio::test(flavor = "multi_thread")]
async fn resume_of_a_paused_run_keeps_the_recorded_engagement() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_engagement_paused";
    let fx = parked_under_network(run_id).await;

    // A pause requested on the parked run stops the approve-resume at the
    // step boundary before `mark`.
    fx.store.set_pause_marker(run_id).unwrap();
    rupu(&["workflow", "approve", run_id, "--mode", "bypass"]).await;
    assert_eq!(
        fx.store.load(run_id).unwrap().status,
        RunStatus::Paused,
        "paused before `mark`"
    );

    rupu(&["workflow", "resume", run_id, "--mode", "bypass", "--plain"]).await;
    assert_marked_under_engagement(&fx, run_id);
}
