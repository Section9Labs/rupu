//! `rupu agent create --describe` and `rupu workflow create --describe`
//! end-to-end via the `RUPU_MOCK_PROVIDER_SCRIPT` seam.
//!
//! Note: env vars are passed to the subprocess via `.env()`, not via
//! `std::env::set_var`, so ENV_LOCK guards against any in-process
//! serialization needs shared with other tests in this binary.

use assert_cmd::Command as AssertCommand;
use assert_fs::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const VALID_AGENT_MD: &str = "---\nname: gen-agent\ndescription: a test agent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou are a helpful test agent.\n";

const VALID_WF_YAML: &str = "name: gen-wf\ndescription: a test workflow\nsteps:\n  - id: main\n    agent: gen-agent\n    prompt: do the thing\n";

#[tokio::test]
async fn agent_create_describe_writes_valid_file() {
    let _g = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");

    let script = serde_json::json!([
        { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
    ])
    .to_string();

    let out = AssertCommand::cargo_bin("rupu")
        .unwrap()
        .args([
            "agent",
            "create",
            "gen-agent",
            "--scope",
            "global",
            "--describe",
            "a helpful test agent",
            "--gen-provider",
            "anthropic",
            "--gen-model",
            "claude-sonnet-4-6",
            "--editor",
            "true",
        ])
        .env("RUPU_HOME", global.path())
        .env("RUPU_MOCK_PROVIDER_SCRIPT", &script)
        .env("EDITOR", "true")
        .output()
        .expect("run");

    // Bind output before assertions so any panic doesn't leave env dirty.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );

    let written =
        std::fs::read_to_string(global.path().join("agents/gen-agent.md")).expect("file written");
    assert!(
        written.contains("name: gen-agent"),
        "generated file should contain `name: gen-agent`\ncontents:\n{written}"
    );
}

#[tokio::test]
async fn workflow_create_describe_writes_valid_file() {
    let _g = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path();
    // Seed an agent so load_agents finds a real name to pass as available_agents.
    std::fs::create_dir_all(home.join("agents")).unwrap();
    std::fs::write(home.join("agents/gen-agent.md"), VALID_AGENT_MD).unwrap();

    let script = serde_json::json!([
        { "AssistantText": { "text": VALID_WF_YAML, "stop": "end_turn" } }
    ])
    .to_string();

    let out = AssertCommand::cargo_bin("rupu")
        .unwrap()
        .args([
            "workflow",
            "create",
            "gen-wf",
            "--scope",
            "global",
            "--describe",
            "a workflow that does the thing",
            "--gen-provider",
            "anthropic",
            "--gen-model",
            "claude-sonnet-4-6",
            "--editor",
            "true",
        ])
        .env("RUPU_HOME", home)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", &script)
        .env("EDITOR", "true")
        .output()
        .expect("run");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {stdout}\nstderr: {stderr}"
    );

    let written =
        std::fs::read_to_string(home.join("workflows/gen-wf.yaml")).expect("file written");
    assert!(
        written.contains("agent: gen-agent"),
        "generated workflow should reference `agent: gen-agent`\ncontents:\n{written}"
    );
}
