use std::io::Write;

#[test]
fn completion_summary_is_rendered_from_a_run_dir() {
    // Exercises the public path the `workflow run` / `resume` wiring calls:
    // build a RunView from a fixture run dir and assert the block that gets
    // printed. (The stdout "prints exactly once" behavior lives in the
    // command layer and is verified by a manual smoke of a real run.)
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let run_dir = runs.join("run_TEST");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("run.json"),
        serde_json::json!({
            "id": "run_TEST", "workflow_name": "assess-services", "status": "completed",
            "inputs": {}, "workspace_id": "ws", "workspace_path": tmp.path(),
            "transcript_dir": tmp.path(), "started_at": "2026-09-30T10:00:00Z",
            "finished_at": "2026-09-30T10:00:18Z", "codename": "mint-tundra"
        })
        .to_string(),
    )
    .unwrap();
    let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
    writeln!(f, r#"{{"type":"run_started","event_version":1,"run_id":"run_TEST","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}}"#).unwrap();
    writeln!(f, r#"{{"type":"step_started","run_id":"run_TEST","step_id":"preflight","kind":"run","agent":null}}"#).unwrap();
    writeln!(f, r#"{{"type":"step_completed","run_id":"run_TEST","step_id":"preflight","success":true,"duration_ms":18000}}"#).unwrap();
    writeln!(f, r#"{{"type":"run_completed","run_id":"run_TEST","status":"completed","finished_at":"2026-09-30T10:00:18Z"}}"#).unwrap();

    let store = rupu_orchestrator::RunStore::new(runs);
    let v = rupu_cli::output::run_model::RunView::from_run_dir(
        &store,
        "run_TEST",
        &rupu_config::PricingConfig::default(),
    );
    let block = rupu_cli::output::run_summary::render_completion_summary(&v, chrono::Utc::now());
    assert!(block.contains("assess-services"));
    assert!(block.contains("mint-tundra"));
    assert!(block.contains("preflight"));
    assert!(block.contains("show-run"));
}
