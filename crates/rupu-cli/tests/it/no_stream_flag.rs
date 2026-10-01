use assert_cmd::Command;

#[test]
fn no_stream_flag_runs_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join(".rupu/agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("hello.md"),
        "---\nname: hello\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nhi",
    )
    .unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&dir)
        .env("RUPU_HOME", dir.path().join(".rupu"))
        .env(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{"AssistantText":{"text":"ok","stop":"end_turn","input_tokens":1,"output_tokens":1}}]"#,
        )
        .args(["run", "hello", "--mode", "bypass", "--no-stream", "hi"])
        .assert()
        .success();
}
