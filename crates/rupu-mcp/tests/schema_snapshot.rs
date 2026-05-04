//! Snapshot test for the tools/list response + jsonschema validity check.
//!
//! Run with `BLESS=1 cargo test -p rupu-mcp --test schema_snapshot ...`
//! to regenerate the snapshot file after intentionally adding/changing tools.

use rupu_mcp::{serve_in_process, McpPermission, Transport};
use rupu_scm::Registry;
use std::sync::Arc;

#[tokio::test]
async fn tools_list_matches_snapshot() {
    let registry = Arc::new(Registry::empty());
    let permission = McpPermission::allow_all();
    let (client, handle) = serve_in_process(registry, permission);

    client
        .send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
        }))
        .await
        .unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    let tools = serde_json::to_string_pretty(&resp["result"]["tools"]).unwrap();

    let path = "tests/snapshots/tools_list.json";
    if std::env::var("BLESS").is_ok() {
        std::fs::create_dir_all("tests/snapshots").unwrap();
        std::fs::write(path, &tools).unwrap();
        eprintln!("snapshot rewritten at {path}");
    }

    let expected =
        std::fs::read_to_string(path).expect("snapshot missing — run with BLESS=1 to generate");
    assert_eq!(
        tools.trim(),
        expected.trim(),
        "tools/list snapshot drift — re-run with BLESS=1 to update if intentional"
    );

    drop(client);
    let _ = handle.join.await;
}

#[test]
fn every_tool_input_schema_compiles_as_jsonschema() {
    for spec in rupu_mcp::tool_catalog() {
        jsonschema::JSONSchema::compile(&spec.input_schema)
            .unwrap_or_else(|e| panic!("tool {} has invalid input_schema: {e}", spec.name));
    }
}
