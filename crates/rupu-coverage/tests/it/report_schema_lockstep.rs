//! The embedded JSON Schema and the Rust validator must agree.
//!
//! Tool definitions are generated from the schema; stored records are
//! checked by serde + `validate_report`. If they drift, an agent is told one
//! contract and held to another. Every case here must be accepted by both or
//! rejected by both. Checks only the validator can express (workspace-relative
//! paths, line order, cross-reference existence, size budget) are unit-tested
//! in `report::validate`.

use rupu_coverage::report::{
    schema::canonical_schema, validate_report, FindingReport, ValidateCtx,
};
use serde_json::{json, Value};

fn valid() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

fn rust_accepts(v: &Value) -> bool {
    match serde_json::from_value::<FindingReport>(v.clone()) {
        Ok(r) => validate_report(
            &r,
            &ValidateCtx {
                known_finding_ids: &[],
                scope: None,
                max_bytes: 262_144,
            },
        )
        .is_ok(),
        Err(_) => false,
    }
}

fn schema_accepts(v: &Value) -> bool {
    let schema = canonical_schema();
    let compiled = jsonschema::JSONSchema::compile(&schema).expect("schema compiles");
    compiled.is_valid(v)
}

fn mutated(f: impl FnOnce(&mut Value)) -> Value {
    let mut v = valid();
    f(&mut v);
    v
}

#[test]
fn valid_fixture_accepted_by_both() {
    let v = valid();
    assert!(rust_accepts(&v), "rust rejected the valid fixture");
    assert!(schema_accepts(&v), "schema rejected the valid fixture");
}

#[test]
fn not_provided_with_justification_accepted_by_both() {
    let v = mutated(|v| v["regression_test"] = json!("Not Provided — needs the physical board"));
    assert!(rust_accepts(&v));
    assert!(schema_accepts(&v));
}

#[test]
fn engagement_blocks_and_classifications_accepted_by_both() {
    let v = mutated(|v| {
        v["blocks"] = json!([
            { "kind": "scan_output", "tool": "nmap", "output": "22/tcp open ssh" },
            { "kind": "http_exchange", "request": "GET / HTTP/1.1", "response": "200 OK" },
        ]);
        v["classifications"] = json!([
            { "system": "CVE", "id": "CVE-2024-1234" },
            { "system": "ATT&CK", "id": "T1190", "vector": "network" },
        ]);
    });
    assert!(rust_accepts(&v), "rust rejected blocks+classifications");
    assert!(schema_accepts(&v), "schema rejected blocks+classifications");
}

#[test]
fn every_block_kind_accepted_by_both() {
    let v = mutated(|v| {
        v["blocks"] = json!([
            { "kind": "text", "text": "t" },
            { "kind": "code_slice", "file": "src/a.rs", "excerpt": "x", "lang": "rust" },
            { "kind": "code_slice", "excerpt": "x" },
            { "kind": "diff", "diff": "-a\n+b" },
            { "kind": "table", "headers": ["h"], "rows": [["r"]] },
            { "kind": "image", "artifact": { "path": "shot.png" }, "caption": "c" },
            { "kind": "hexdump", "base": 4096, "artifact": { "path": "dump.bin" }, "rendered": "00" },
            { "kind": "disasm", "arch": "x86_64", "listing": [
                { "address": 4096, "bytes": "90", "mnemonic": "nop" },
                { "address": 4097, "bytes": "c3", "mnemonic": "ret", "ops": "" }
            ] },
            { "kind": "decompile", "lang": "c", "listing": "int f();" },
            { "kind": "http_exchange", "request": "GET /", "response": "200" },
            { "kind": "scan_output", "tool": "nmap", "output": "open" },
            { "kind": "pcap_ref", "artifact": { "path": "cap.pcap" }, "summary": "s" },
        ]);
    });
    assert!(rust_accepts(&v), "rust rejected a block kind");
    assert!(schema_accepts(&v), "schema rejected a block kind");
}

#[test]
fn invalid_cases_rejected_by_both() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "missing root_cause",
            mutated(|v| {
                v.as_object_mut().unwrap().remove("root_cause");
            }),
        ),
        (
            "Unknown regression test",
            mutated(|v| v["regression_test"] = json!("Unknown")),
        ),
        (
            "empty justification",
            mutated(|v| v["regression_test"] = json!("Not Provided — ")),
        ),
        (
            "justification after a double space",
            mutated(|v| v["regression_test"] = json!("Not Provided —  needs the board")),
        ),
        (
            "bad ticket sentinel",
            mutated(|v| v["tickets"] = json!("TBD")),
        ),
        (
            "likelihood Critical",
            mutated(|v| v["rating"]["likelihood"] = json!("Critical")),
        ),
        (
            "no replication steps",
            mutated(|v| v["replication_steps"] = json!([])),
        ),
        ("bare cwe number", mutated(|v| v["cwe"] = json!(["306"]))),
        (
            "empty description",
            mutated(|v| v["description"] = json!("")),
        ),
        (
            "lowercase none",
            mutated(|v| v["cross_references"] = json!("none")),
        ),
        ("no evidence", mutated(|v| v["evidence"] = json!([]))),
        ("empty call chain", mutated(|v| v["call_chain"] = json!([]))),
        ("unknown field", mutated(|v| v["extra"] = json!(1))),
        (
            "unknown nested field",
            mutated(|v| v["rating"]["severity"] = json!("High")),
        ),
        (
            "classification with empty system",
            mutated(|v| v["classifications"] = json!([{ "system": "", "id": "CVE-1" }])),
        ),
        (
            "unknown block field",
            mutated(|v| {
                v["blocks"] = json!([{ "kind": "scan_output", "tool": "nmap", "output": "o", "exit_code": 0 }])
            }),
        ),
        (
            "block field from another kind",
            mutated(
                |v| v["blocks"] = json!([{ "kind": "http_exchange", "request": "GET /", "response": "200", "tool": "curl" }]),
            ),
        ),
        (
            "block missing its field",
            mutated(|v| v["blocks"] = json!([{ "kind": "scan_output", "tool": "nmap" }])),
        ),
        (
            "unknown block kind",
            mutated(|v| v["blocks"] = json!([{ "kind": "video", "text": "t" }])),
        ),
        (
            "unknown disasm line field",
            mutated(|v| {
                v["blocks"] = json!([{ "kind": "disasm", "arch": "arm", "listing": [
                    { "address": 0, "bytes": "00", "mnemonic": "nop", "comment": "c" }
                ] }])
            }),
        ),
        (
            "unknown block artifact field",
            mutated(|v| {
                v["blocks"] =
                    json!([{ "kind": "image", "artifact": { "path": "a.png", "url": "u" } }])
            }),
        ),
        (
            "classification missing id",
            mutated(|v| v["classifications"] = json!([{ "system": "CVE" }])),
        ),
    ];
    for (name, v) in cases {
        assert!(!rust_accepts(&v), "rust accepted invalid case: {name}");
        assert!(!schema_accepts(&v), "schema accepted invalid case: {name}");
    }
}
