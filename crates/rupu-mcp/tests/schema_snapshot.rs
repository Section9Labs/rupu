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
    let tools = resp["result"]["tools"].clone();
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

    // Compare structurally (parsed serde_json::Value), not as raw strings.
    // `schemars`/serde_json object-key ordering is not part of the
    // catalog's contract and drifts across toolchains (ISSUES.md I-81) —
    // what matters is the tool catalog's *content*, not its serialized
    // field order. serde_json::Value equality ignores object-key order
    // (objects compare as maps) while still catching any real content
    // change: added/removed/renamed tools or fields, changed types,
    // descriptions, schemas, or array order (tool list order, `required`
    // arrays, `type` union order, etc. all still compare positionally).
    assert_eq!(
        tools, expected,
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
