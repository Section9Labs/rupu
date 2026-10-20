//! Snapshot of the catalog source's `tools/list` + jsonschema validity.
//!
//! Run with `BLESS=1 cargo test -p rupu-mcp --test it schema_snapshot:: ...`
//! to regenerate the snapshot file after intentionally changing the tools.

use crate::catalog::source;
use rupu_mcp::ToolSource;
use serde_json::{json, Value};

fn listed(root: &std::path::Path) -> Vec<Value> {
    source(root)
        .tools()
        .into_iter()
        .map(|t| json!({ "name": t.name, "inputSchema": t.input_schema }))
        .collect()
}

#[tokio::test]
async fn tools_list_matches_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tools = Value::Array(listed(tmp.path()));
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
}

#[tokio::test]
async fn every_listed_input_schema_compiles_as_jsonschema() {
    let tmp = tempfile::TempDir::new().unwrap();
    for tool in listed(tmp.path()) {
        jsonschema::JSONSchema::compile(&tool["inputSchema"])
            .unwrap_or_else(|e| panic!("tool {} has invalid inputSchema: {e}", tool["name"]));
    }
}
