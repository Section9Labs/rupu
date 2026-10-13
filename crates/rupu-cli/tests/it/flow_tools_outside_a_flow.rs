//! Grants are just names (W5 §6 test 3): a plain `rupu run` whose agent
//! lists a roster tool gets it (every launch site provides the catalog). And
//! every run has a message space (spec F1a, F1b): one whose agent lists a
//! board tool gets it; every run joins it as it starts, and its log is
//! created by its first message.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;

fn write_agent(dir: &Path, name: &str, tools: &str) {
    let agents = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: Lists the roster.\nprovider: anthropic\n\
             model: claude-sonnet-4-6\ntools: {tools}\n---\nYou look things up.\n"
        ),
    )
    .unwrap();
}

/// Run `agent` once as `run_id` on a scripted provider; its transcript.
fn run(dir: &Path, agent: &str, run_id: &str, script: &str) -> Vec<Value> {
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.join(".rupu"))
        .args(["run", agent, "--mode", "bypass", "--run-id", run_id, "go"])
        .assert()
        .success();
    std::fs::read_to_string(
        dir.join(".rupu/transcripts")
            .join(format!("{run_id}.jsonl")),
    )
    .unwrap()
    .lines()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect()
}

fn events<'a>(t: &'a [Value], kind: &str) -> Vec<&'a Value> {
    t.iter().filter(|e| e["type"] == kind).collect()
}

#[test]
fn catalog_tools_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "librarian", "[agents.list]");
    write_agent(dir.path(), "other", "[read_file]");
    let script = r#"[
        {"AssistantToolUse":{"text":null,"tool_id":"t1","tool_name":"agents.list","tool_input":{},"stop":"tool_use"}},
        {"AssistantText":{"text":"done","stop":"end_turn"}}
    ]"#;
    let t = run(dir.path(), "librarian", "run_catalog", script);

    let grant = events(&t, "tool_grant");
    assert_eq!(grant.len(), 1, "{t:?}");
    let entries = grant[0]["data"]["entries"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["tool"] == "agents.list"),
        "{entries:?}"
    );

    let results = events(&t, "tool_result");
    assert_eq!(results.len(), 1, "{t:?}");
    let listed: Value =
        serde_json::from_str(results[0]["data"]["output"].as_str().unwrap()).unwrap();
    let names: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["librarian", "other"]);
}

#[test]
fn board_tools_in_a_plain_run() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "poster", "[board.post]");
    let t = run(
        dir.path(),
        "poster",
        "run_board",
        r#"[
            {"AssistantToolUse":{"text":null,"tool_id":"t1","tool_name":"board.post","tool_input":{"kind":"note","body":"halfway there"},"stop":"tool_use"}},
            {"AssistantText":{"text":"done","stop":"end_turn"}}
        ]"#,
    );

    let grant = &events(&t, "tool_grant")[0]["data"];
    let entries = grant["entries"].as_array().unwrap();
    let post = entries.iter().find(|e| e["tool"] == "board.post");
    assert_eq!(post.unwrap()["reasons"][0], "declared", "{entries:?}");
    assert!(grant.get("unavailable").is_none(), "{grant}");

    let space = dir.path().join(".rupu/runs/run_board/messages");
    let log: Vec<Value> = std::fs::read_to_string(space.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["kind"], "post");
    assert_eq!(log[0]["body"], "halfway there");
    let sent = events(&t, "message");
    assert_eq!(sent.len(), 1, "{t:?}");
    assert_eq!(sent[0]["data"]["direction"], "sent");
    assert_eq!(sent[0]["data"]["id"], log[0]["id"]);
    // The run joined as it started, and left as it ended.
    let roster = std::fs::read_to_string(space.join("participants.jsonl")).unwrap();
    assert_eq!(roster.lines().count(), 2, "{roster}");

    // A run that never messages joins too, so the operator can reach it;
    // its log waits for a first message.
    write_agent(dir.path(), "quiet", "[board.post]");
    run(
        dir.path(),
        "quiet",
        "run_quiet",
        r#"[{"AssistantText":{"text":"done","stop":"end_turn"}}]"#,
    );
    let quiet = dir.path().join(".rupu/runs/run_quiet/messages");
    let roster: Vec<Value> = std::fs::read_to_string(quiet.join("participants.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(roster.len(), 2, "{roster:?}");
    assert_eq!(roster[0]["t"], "joined");
    assert_eq!(roster[0]["run_id"], "run_quiet");
    assert!(roster[0]["pid"].is_u64(), "{roster:?}");
    assert_eq!(roster[1]["t"], "left");
    assert!(!quiet.join("log.jsonl").exists());
}
