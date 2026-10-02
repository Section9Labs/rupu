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

/// A finished run whose log carries two `step_warning`s (one about a fan-out
/// unit, one about the step) laid out under `<home>/runs/run_WARN`.
fn write_warned_run(home: &std::path::Path, workspace: &std::path::Path) {
    let run_dir = home.join("runs").join("run_WARN");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("run.json"),
        serde_json::json!({
            "id": "run_WARN", "workflow_name": "sweep-hosts", "status": "completed",
            "inputs": {}, "workspace_id": "ws", "workspace_path": workspace,
            "transcript_dir": workspace, "started_at": "2026-09-30T10:00:00Z",
            "finished_at": "2026-09-30T10:00:18Z", "codename": "mint-tundra"
        })
        .to_string(),
    )
    .unwrap();
    let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
    for line in [
        r#"{"type":"run_started","event_version":1,"run_id":"run_WARN","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}"#,
        r#"{"type":"step_started","run_id":"run_WARN","step_id":"sweep","kind":"for_each","agent":null}"#,
        r#"{"type":"step_warning","run_id":"run_WARN","step_id":"sweep","index":2,"message":"host gpu-9 sent no coverage stream"}"#,
        r#"{"type":"step_warning","run_id":"run_WARN","step_id":"sweep","message":"coverage merge skipped"}"#,
        r#"{"type":"step_completed","run_id":"run_WARN","step_id":"sweep","success":true,"duration_ms":18000}"#,
        r#"{"type":"run_completed","run_id":"run_WARN","status":"completed","finished_at":"2026-09-30T10:00:18Z"}"#,
    ] {
        writeln!(f, "{line}").unwrap();
    }
}

#[test]
fn completion_summary_from_a_run_dir_lists_every_step_warning() {
    // The same `RunView::from_run_dir` + `render_completion_summary` pair that
    // `print_completion_summary` runs after a `workflow run` on every
    // interactive surface — including `--plain` / a non-tty stdout, which
    // reaches it through the line-printer branch.
    let tmp = tempfile::tempdir().unwrap();
    write_warned_run(tmp.path(), tmp.path());
    let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
    let v = rupu_cli::output::run_model::RunView::from_run_dir(
        &store,
        "run_WARN",
        &rupu_config::PricingConfig::default(),
    );
    let block = rupu_cli::output::run_summary::render_completion_summary(&v, chrono::Utc::now());
    assert!(
        block.contains(
            "⚠ sweep[2]: host gpu-9 sent no coverage stream\n⚠ sweep: coverage merge skipped\n"
        ),
        "{block}"
    );
    // a warning is information: the run still reads completed
    assert!(block.contains("completed"), "{block}");
}

#[test]
fn workflow_show_run_prints_the_same_warning_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    write_warned_run(&home, &project);

    let run = |format: &str| {
        let out = assert_cmd::Command::cargo_bin("rupu")
            .unwrap()
            .env("RUPU_HOME", &home)
            .env_remove("FORCE_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .current_dir(&project)
            .args(["--format", format, "workflow", "show-run", "run_WARN"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };

    let pretty = run("pretty");
    assert!(
        pretty.contains(
            "⚠ sweep[2]: host gpu-9 sent no coverage stream\n⚠ sweep: coverage merge skipped\n"
        ),
        "{pretty}"
    );

    let json: serde_json::Value = serde_json::from_str(&run("json")).unwrap();
    assert_eq!(
        json["item"]["warnings"],
        serde_json::json!([
            "sweep[2]: host gpu-9 sent no coverage stream",
            "sweep: coverage merge skipped"
        ]),
        "{json}"
    );
}

#[test]
fn workflow_show_run_of_a_clean_run_prints_no_warning_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    write_warned_run(&home, &project);
    // Drop the two warning lines from the log.
    let log = home.join("runs/run_WARN/events.jsonl");
    let clean: String = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| !l.contains("step_warning"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&log, clean).unwrap();

    let out = assert_cmd::Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(&project)
        .args(["--format", "pretty", "workflow", "show-run", "run_WARN"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pretty = String::from_utf8(out).unwrap();
    assert!(!pretty.contains('⚠'), "{pretty}");
}
