//! Snapshot of `rupu mcp serve`'s default `tools/list` + jsonschema validity.
//!
//! Run with `BLESS=1 cargo test -p rupu-mcp --test it schema_snapshot:: ...`
//! to regenerate the snapshot file after intentionally changing the tools.

use crate::serve::{list, server};

#[tokio::test]
async fn tools_list_matches_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), None, rupu_tools::PermissionMode::Bypass);
    let tools = serde_json::Value::Array(list(&client).await);
    let tools_pretty = serde_json::to_string_pretty(&tools).unwrap();

    let path = "tests/snapshots/tools_list.json";
    if std::env::var("BLESS").is_ok() {
        std::fs::create_dir_all("tests/snapshots").unwrap();
        std::fs::write(path, &tools_pretty).unwrap();
        eprintln!("snapshot rewritten at {path}");
    }

    let expected_raw =
        std::fs::read_to_string(path).expect("snapshot missing — run with BLESS=1 to generate");
    let expected: serde_json::Value =
        serde_json::from_str(&expected_raw).expect("snapshot is not valid JSON");

    // Compare structurally (parsed serde_json::Value), not as raw strings:
    // object-key ordering is not part of the contract and drifts across
    // toolchains (ISSUES.md I-81); array order (tool list order, `required`
    // arrays) still compares positionally.
    assert_eq!(
        tools, expected,
        "tools/list snapshot drift — re-run with BLESS=1 to update if intentional"
    );

    drop(client);
    let _ = handle.await;
}

#[tokio::test]
async fn every_listed_input_schema_compiles_as_jsonschema() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (client, handle, _) = server(tmp.path(), Some(&["*"]), rupu_tools::PermissionMode::Bypass);
    for tool in list(&client).await {
        jsonschema::JSONSchema::compile(&tool["inputSchema"])
            .unwrap_or_else(|e| panic!("tool {} has invalid inputSchema: {e}", tool["name"]));
    }
    drop(client);
    let _ = handle.await;
}
