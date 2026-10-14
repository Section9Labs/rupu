//! `report_finding`'s input contract: unknown keys are refused at every level
//! a caller writes, and a cross-reference outside the reporter's findings
//! scope is refused with an error that names the scope.

use rupu_coverage::report::{FindingProfile, FindingReport, FindingWriteOptions};
use rupu_coverage::tools::report_finding::{report_finding, ReportFindingInput};
use rupu_coverage::{target_id, Attribution, CoveragePaths, Surface};
use serde_json::{json, Value};

fn report() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

fn parse(v: Value) -> Result<ReportFindingInput, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

fn attribution(run_id: &str) -> Attribution {
    Attribution {
        run_id: run_id.into(),
        model: "m".into(),
        surface: Surface::Agent,
        codename: None,
        agent: None,
        provider: None,
    }
}

fn record(paths: &CoveragePaths, input: Value) -> Result<String, String> {
    let opts = FindingWriteOptions::default().with_profile(FindingProfile::Full);
    report_finding(paths, attribution("run_1"), parse(input)?, &opts)
        .map(|o| o.id)
        .map_err(|e| e.to_string())
}

#[test]
fn an_unknown_top_level_key_is_refused() {
    let err = parse(json!({ "scope": "repo", "report": report(), "severty": "high" }))
        .expect_err("unknown key refused");
    assert!(err.contains("unknown field `severty`"), "{err}");
}

#[test]
fn an_unknown_asset_key_is_refused() {
    let err = parse(json!({
        "scope": "repo",
        "report": report(),
        "asset": { "kind": "network:service", "coords": [] }
    }))
    .expect_err("unknown asset key refused");
    assert!(err.contains("unknown field `coords`"), "{err}");
}

#[test]
fn an_unknown_block_field_is_refused() {
    let mut r = report();
    r["blocks"] = json!([{ "kind": "http_exchange", "request": "GET /", "response": "200", "latency_ms": 4 }]);
    let err = serde_json::from_value::<FindingReport>(r).expect_err("unknown block field refused");
    assert!(
        err.to_string().contains("unknown field `latency_ms`"),
        "{err}"
    );
}

#[test]
fn every_documented_key_parses() {
    parse(json!({
        "file_path": "src/a.rs",
        "line_range": [1, 2],
        "target_ref": "198.51.100.7",
        "scope": "line",
        "summary": "s",
        "severity": "low",
        "concern_id": "c",
        "evidence": { "rationale": "r", "code_excerpt": "x", "references": ["u"] },
        "report": report(),
        "asset": { "kind": "network:service", "coordinates": [], "label": "l" },
        "tags": ["t"]
    }))
    .expect("every field ReportFindingInput documents parses");
}

#[test]
fn a_cross_reference_outside_the_scope_names_the_scope() {
    let ws = tempfile::tempdir().unwrap();
    let other = CoveragePaths::new(ws.path(), &target_id(ws.path(), "edge-recon"))
        .with_scope_name("edge-recon");
    let foreign = record(&other, json!({ "scope": "repo", "report": report() })).unwrap();

    let mine = CoveragePaths::new(ws.path(), &target_id(ws.path(), "edge-exploit"))
        .with_scope_name("edge-exploit");
    let mut r = report();
    r["cross_references"] = json!([{ "finding_id": foreign, "relation": "prerequisite" }]);
    let err = record(&mine, json!({ "scope": "repo", "report": r.clone() }))
        .expect_err("a cross-reference outside the scope is refused");
    assert!(
        err.contains(&format!(
            "report.cross_references[0].finding_id: `{foreign}` is not a finding in this run's findings scope `edge-exploit`"
        )),
        "{err}"
    );
    assert!(err.contains("`references` or `description`"), "{err}");

    // In the scope that recorded it, the same reference is accepted.
    record(&other, json!({ "scope": "repo", "report": r })).expect("same-scope reference");
}

#[test]
fn without_a_scope_name_the_error_names_no_scope() {
    let ws = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(ws.path(), "tgt");
    let mut r = report();
    r["cross_references"] = json!([{ "finding_id": "fnd_01ABC", "relation": "sibling" }]);
    let err = record(&paths, json!({ "scope": "repo", "report": r })).unwrap_err();
    assert!(
        err.contains("`fnd_01ABC` is not a finding in this findings scope;"),
        "{err}"
    );
}
