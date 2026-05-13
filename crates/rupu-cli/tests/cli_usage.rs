use assert_cmd::Command;
use chrono::{Duration, Utc};
use predicates::prelude::*;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore, StepKind, StepResultRecord};
use rupu_runtime::{
    ExecutionRequest, RepoBinding, RunContext, RunEnvelope, RunKind, RunTrigger, RunTriggerSource,
    WorkflowBinding,
};
use rupu_transcript::{Event, JsonlReader, JsonlWriter, RunMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

fn init_git_checkout(path: &Path, origin_url: &str) {
    let status = ProcessCommand::new("git")
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
    let status = ProcessCommand::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", "origin", origin_url])
        .status()
        .unwrap();
    assert!(status.success());
}

#[allow(clippy::too_many_arguments)]
fn write_usage_transcript(
    dir: &Path,
    run_id: &str,
    agent: &str,
    provider: &str,
    model: &str,
    started_at: chrono::DateTime<Utc>,
    input_tokens: u32,
    output_tokens: u32,
) -> PathBuf {
    let path = dir.join(format!("{run_id}.jsonl"));
    let mut writer = JsonlWriter::create(&path).unwrap();
    writer
        .write(&Event::RunStart {
            run_id: run_id.into(),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: provider.into(),
            model: model.into(),
            started_at,
            mode: RunMode::Bypass,
        })
        .unwrap();
    writer
        .write(&Event::Usage {
            provider: provider.into(),
            model: model.into(),
            input_tokens,
            output_tokens,
            cached_tokens: 0,
        })
        .unwrap();
    writer
        .write(&Event::RunComplete {
            run_id: run_id.into(),
            status: rupu_transcript::RunStatus::Ok,
            total_tokens: (input_tokens + output_tokens) as u64,
            duration_ms: 100,
            error: None,
        })
        .unwrap();
    writer.flush().unwrap();
    path
}

fn write_standalone_usage_metadata(
    dir: &Path,
    run_id: &str,
    repo_ref: &str,
    issue_ref: Option<&str>,
) {
    let path = rupu_cli::standalone_run_metadata::metadata_path_for_run(dir, run_id);
    rupu_cli::standalone_run_metadata::write_metadata(
        &path,
        &rupu_cli::standalone_run_metadata::StandaloneRunMetadata {
            version: rupu_cli::standalone_run_metadata::StandaloneRunMetadata::VERSION,
            run_id: run_id.into(),
            workspace_path: PathBuf::from("/tmp/repo"),
            project_root: Some(PathBuf::from("/tmp/project")),
            repo_ref: Some(repo_ref.into()),
            issue_ref: issue_ref.map(str::to_owned),
            backend_id: "local_checkout".into(),
            worker_id: Some("worker_local_cli".into()),
            trigger_source: "run_cli".into(),
            target: issue_ref.map(str::to_owned),
            workspace_strategy: Some("direct_checkout".into()),
        },
    )
    .unwrap();
}

fn sample_run_record(
    id: &str,
    workflow_name: &str,
    issue_ref: &str,
    started_at: chrono::DateTime<Utc>,
    transcript_dir: &Path,
    status: RunStatus,
) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: workflow_name.into(),
        status,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_01".into(),
        workspace_path: PathBuf::from("/tmp/repo"),
        transcript_dir: transcript_dir.to_path_buf(),
        started_at,
        finished_at: Some(started_at + Duration::minutes(5)),
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: Some(issue_ref.into()),
        issue: None,
        parent_run_id: None,
        backend_id: Some("local_worktree".into()),
        worker_id: Some("worker_local_cli".into()),
        artifact_manifest_path: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
    }
}

fn sample_envelope(
    run_id: &str,
    workflow_name: &str,
    issue_ref: &str,
    repo_ref: &str,
) -> RunEnvelope {
    RunEnvelope {
        version: RunEnvelope::VERSION,
        run_id: run_id.into(),
        kind: RunKind::WorkflowRun,
        workflow: WorkflowBinding {
            name: workflow_name.into(),
            source_path: PathBuf::from(format!(".rupu/workflows/{workflow_name}.yaml")),
            fingerprint: "sha256:test".into(),
        },
        repo: Some(RepoBinding {
            repo_ref: Some(repo_ref.into()),
            project_root: Some(PathBuf::from("/tmp/repo")),
            workspace_id: "ws_01".into(),
            workspace_path: PathBuf::from("/tmp/repo"),
        }),
        trigger: RunTrigger {
            source: RunTriggerSource::Autoflow,
            wake_id: Some("wake_01".into()),
            event_id: Some("github.issue.opened".into()),
        },
        inputs: BTreeMap::new(),
        context: Some(RunContext {
            issue_ref: Some(issue_ref.into()),
            target: Some(issue_ref.into()),
            event_present: false,
            issue_present: true,
        }),
        execution: ExecutionRequest {
            backend: Some("local_worktree".into()),
            permission_mode: "bypass".into(),
            workspace_strategy: Some("managed_worktree".into()),
            strict_templates: true,
            attach_ui: false,
            use_canvas: false,
        },
        autoflow: None,
        correlation: None,
        worker: None,
    }
}

fn sample_step_result(run_id: &str, transcript_path: &Path) -> StepResultRecord {
    StepResultRecord {
        step_id: "implement".into(),
        run_id: run_id.into(),
        transcript_path: transcript_path.to_path_buf(),
        output: "ok".into(),
        success: true,
        skipped: false,
        rendered_prompt: "do work".into(),
        kind: StepKind::Linear,
        items: Vec::new(),
        findings: Vec::new(),
        iterations: 0,
        resolved: true,
        finished_at: Utc::now(),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_workflow_usage_run(
    home: &Path,
    run_id: &str,
    workflow_name: &str,
    issue_ref: &str,
    repo_ref: &str,
    status: RunStatus,
    agent: &str,
    provider: &str,
    model: &str,
    started_at: chrono::DateTime<Utc>,
    input_tokens: u32,
    output_tokens: u32,
) {
    let transcripts = home.join("transcripts");
    let runs_root = home.join("runs");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::create_dir_all(&runs_root).unwrap();

    let transcript_path = write_usage_transcript(
        &transcripts,
        &format!("{run_id}_step"),
        agent,
        provider,
        model,
        started_at,
        input_tokens,
        output_tokens,
    );
    let store = RunStore::new(runs_root);
    store
        .write_run_envelope(
            run_id,
            &sample_envelope(run_id, workflow_name, issue_ref, repo_ref),
        )
        .unwrap();
    store
        .create(
            sample_run_record(
                run_id,
                workflow_name,
                issue_ref,
                started_at,
                &transcripts,
                status,
            ),
            &format!("name: {workflow_name}\nsteps: []\n"),
        )
        .unwrap();
    store
        .append_step_result(run_id, &sample_step_result(run_id, &transcript_path))
        .unwrap();
}

#[test]
fn usage_supports_global_json_format() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    write_usage_transcript(
        &transcripts,
        "run_usage_json",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        12,
        4,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["usage", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"usage_breakdown\""))
        .stdout(predicate::str::contains("\"provider\": \"anthropic\""))
        .stdout(predicate::str::contains("\"top_providers\""));
}

#[test]
fn usage_supports_csv_format() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    write_usage_transcript(
        &transcripts,
        "run_usage_csv",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        12,
        4,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["--format", "csv", "usage"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "group,provider,model,agent,input_tokens,output_tokens,cached_tokens,runs,cost_usd,cost_partial",
        ))
        .stdout(predicate::str::contains(
            "anthropic / claude-sonnet-4-6 / reviewer,anthropic,claude-sonnet-4-6,reviewer,12,4,0,1,",
        ));
}

#[test]
fn usage_default_view_shows_summary_and_last_30d_window() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    write_usage_transcript(
        &transcripts,
        "run_usage_table",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        120,
        40,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["usage"])
        .assert()
        .success()
        .stdout(predicate::str::contains("METRIC"))
        .stdout(predicate::str::contains("last 30d"))
        .stdout(predicate::str::contains("Top Providers"))
        .stdout(predicate::str::contains("Top Agents"))
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("claude-sonnet-4-6"));
}

#[test]
fn usage_group_by_workflow_and_repo_filter_use_run_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    write_workflow_usage_run(
        &home,
        "run_phase",
        "phase-delivery-cycle",
        "github:Section9Labs/rupu/issues/42",
        "github:Section9Labs/rupu",
        RunStatus::Completed,
        "implementer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        80,
        20,
    );
    write_workflow_usage_run(
        &home,
        "run_other_repo",
        "code-review-panel",
        "github:OtherOrg/other/issues/9",
        "github:OtherOrg/other",
        RunStatus::Completed,
        "reviewer",
        "openai",
        "gpt-5",
        Utc::now(),
        50,
        10,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args([
            "usage",
            "--group-by",
            "workflow",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("phase-delivery-cycle"))
        .stdout(predicate::str::contains("code-review-panel").not());
}

#[test]
fn usage_repo_filter_includes_standalone_runs_with_sidecar_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&transcripts).unwrap();

    write_usage_transcript(
        &transcripts,
        "run_standalone_repo",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        24,
        8,
    );
    write_standalone_usage_metadata(
        &transcripts,
        "run_standalone_repo",
        "github:Section9Labs/rupu",
        Some("github:Section9Labs/rupu/issues/42"),
    );
    write_usage_transcript(
        &transcripts,
        "run_standalone_other",
        "planner",
        "openai",
        "gpt-5",
        Utc::now(),
        12,
        5,
    );
    write_standalone_usage_metadata(
        &transcripts,
        "run_standalone_other",
        "github:OtherOrg/other",
        Some("github:OtherOrg/other/issues/9"),
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args([
            "usage",
            "--group-by",
            "agent",
            "--repo",
            "github:Section9Labs/rupu",
            "--since",
            "30d",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("planner").not());
}

#[test]
fn usage_supports_issue_worker_backend_and_trigger_filters() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&transcripts).unwrap();

    write_usage_transcript(
        &transcripts,
        "run_standalone_issue_42",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now(),
        30,
        12,
    );
    write_standalone_usage_metadata(
        &transcripts,
        "run_standalone_issue_42",
        "github:Section9Labs/rupu",
        Some("github:Section9Labs/rupu/issues/42"),
    );
    write_workflow_usage_run(
        &home,
        "run_workflow_issue_43",
        "phase-delivery-cycle",
        "github:Section9Labs/rupu/issues/43",
        "github:Section9Labs/rupu",
        RunStatus::Completed,
        "implementer",
        "openai",
        "gpt-5",
        Utc::now(),
        45,
        15,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args([
            "usage",
            "--group-by",
            "agent",
            "--issue",
            "github:Section9Labs/rupu/issues/42",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("implementer").not());

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args([
            "usage",
            "runs",
            "--worker",
            "worker_local_cli",
            "--backend",
            "local_checkout",
            "--trigger",
            "run_cli",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("run_standalone_issue_42"))
        .stdout(predicate::str::contains(
            "github:Section9Labs/rupu/issues/42",
        ))
        .stdout(predicate::str::contains("run_cli"))
        .stdout(predicate::str::contains("local_checkout"))
        .stdout(predicate::str::contains("run_workflow_issue_43").not());
}

#[test]
fn usage_backfill_creates_sidecars_for_old_standalone_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    let transcripts = home.join("transcripts");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&transcripts).unwrap();
    init_git_checkout(&project, "git@github.com:Section9Labs/rupu.git");

    let store = rupu_workspace::WorkspaceStore {
        root: home.join("workspaces"),
    };
    let workspace = rupu_workspace::upsert(&store, &project).unwrap();
    let started_at = Utc::now();
    let transcript_path = write_usage_transcript(
        &transcripts,
        "run_old_standalone",
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        started_at,
        18,
        6,
    );
    let events = JsonlReader::iter(&transcript_path)
        .unwrap()
        .map(|event| event.unwrap())
        .collect::<Vec<_>>();
    let mut writer = JsonlWriter::create(&transcript_path).unwrap();
    writer
        .write(&Event::RunStart {
            run_id: "run_old_standalone".into(),
            workspace_id: workspace.id.clone(),
            agent: "reviewer".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at,
            mode: RunMode::Bypass,
        })
        .unwrap();
    for event in events.into_iter().skip(1) {
        writer.write(&event).unwrap();
    }
    writer.flush().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["usage", "backfill"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Backfilled"))
        .stdout(predicate::str::contains("1"));

    let metadata_path = rupu_cli::standalone_run_metadata::metadata_path_for_run(
        &transcripts,
        "run_old_standalone",
    );
    assert!(metadata_path.exists());

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args([
            "usage",
            "--group-by",
            "agent",
            "--repo",
            "github:Section9Labs/rupu",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"));
}

#[test]
fn usage_runs_support_failed_and_top_cost_views() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".rupu");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    write_workflow_usage_run(
        &home,
        "run_failed_large",
        "phase-delivery-cycle",
        "github:Section9Labs/rupu/issues/42",
        "github:Section9Labs/rupu",
        RunStatus::Failed,
        "implementer",
        "openai",
        "gpt-5",
        Utc::now(),
        200,
        100,
    );
    write_workflow_usage_run(
        &home,
        "run_completed_small",
        "phase-delivery-cycle",
        "github:Section9Labs/rupu/issues/43",
        "github:Section9Labs/rupu",
        RunStatus::Completed,
        "reviewer",
        "anthropic",
        "claude-sonnet-4-6",
        Utc::now() - Duration::hours(1),
        20,
        10,
    );

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["usage", "runs", "--failed"])
        .assert()
        .success()
        .stdout(predicate::str::contains("run_failed_large"))
        .stdout(predicate::str::contains("failed"))
        .stdout(predicate::str::contains("run_completed_small").not());

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&project)
        .env("RUPU_HOME", &home)
        .args(["usage", "runs", "--top-cost", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("run_failed_large"))
        .stdout(predicate::str::contains("worker_local_cli"))
        .stdout(predicate::str::contains("local_worktree"))
        .stdout(predicate::str::contains("run_completed_small").not());
}

#[test]
fn agent_list_supports_global_json_format() {
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["--format", "json", "agent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"agent_list\""))
        .stdout(predicate::str::contains("\"rows\""));
}
