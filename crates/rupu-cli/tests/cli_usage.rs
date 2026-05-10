use assert_cmd::Command;
use predicates::prelude::*;
use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};

fn write_usage_transcript(
    dir: &std::path::Path,
    run_id: &str,
    agent: &str,
    provider: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) {
    let path = dir.join(format!("{run_id}.jsonl"));
    let mut writer = JsonlWriter::create(&path).unwrap();
    writer
        .write(&Event::RunStart {
            run_id: run_id.into(),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: provider.into(),
            model: model.into(),
            started_at: chrono::Utc::now(),
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
            status: RunStatus::Ok,
            total_tokens: (input_tokens + output_tokens) as u64,
            duration_ms: 100,
            error: None,
        })
        .unwrap();
    writer.flush().unwrap();
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
        .stdout(predicate::str::contains("\"provider\": \"anthropic\""))
        .stdout(predicate::str::contains("\"agent\": \"reviewer\""));
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
            "provider,model,agent,input_tokens,output_tokens,cached_tokens,runs,cost_usd",
        ))
        .stdout(predicate::str::contains(
            "anthropic,claude-sonnet-4-6,reviewer,12,4,0,1,",
        ));
}

#[test]
fn unsupported_global_format_is_rejected() {
    Command::cargo_bin("rupu")
        .unwrap()
        .args(["--format", "json", "agent", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not support structured `--format json` output yet",
        ));
}
