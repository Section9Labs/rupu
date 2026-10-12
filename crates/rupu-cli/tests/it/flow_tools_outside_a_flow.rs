//! W5 §6 tests 3 and 4: grants are just names. A plain `rupu run` whose
//! agent lists a roster tool gets it (every launch site provides the
//! catalog), and one that lists a board tool is told it is unavailable —
//! there is no message bus outside an agentiflow.

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
fn board_outside_flow_notice() {
    let dir = tempfile::tempdir().unwrap();
    write_agent(dir.path(), "poster", "[board.post]");
    let t = run(
        dir.path(),
        "poster",
        "run_board",
        r#"[{"AssistantText":{"text":"done","stop":"end_turn"}}]"#,
    );

    let grant = &events(&t, "tool_grant")[0]["data"];
    let entries = grant["entries"].as_array().unwrap();
    assert!(
        !entries.iter().any(|e| e["tool"] == "board.post"),
        "{entries:?}"
    );
    assert_eq!(grant["unavailable"][0]["tool"], "board.post", "{grant}");
    assert_eq!(grant["unavailable"][0]["missing"][0], "message_bus");

    let notices: Vec<_> = events(&t, "notice")
        .into_iter()
        .filter(|n| n["data"]["kind"] == "tool_unavailable")
        .collect();
    assert_eq!(notices.len(), 1, "{t:?}");
    assert!(notices[0]["data"]["message"]
        .as_str()
        .unwrap()
        .contains("board.post"));
}
