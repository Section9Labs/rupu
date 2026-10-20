//! The findings-area operator tools: findings, tags, exports, coverage and
//! assets.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, rpc, spawn_operator_with, text_of};
use base64::Engine as _;
use rupu_coverage::report::{ArtifactKind, ArtifactRef, FindingReport};
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Ledger, Severity, Surface, TagLog,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const F: &str = "fnd_mcp_one";
const TITLE: &str = "Notes API returns another user's note by id";
const REFERENCED_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn record() -> FindingRecord {
    let mut report: FindingReport = serde_json::from_str(include_str!(
        "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    report.artifacts = vec![ArtifactRef {
        path: "poc/exploit.py".into(),
        sha256: REFERENCED_SHA.into(),
        size: 3,
        kind: Some(ArtifactKind::Text),
        stored: None,
        host: None,
    }];
    FindingRecord {
        id: F.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::File,
        summary: "s".into(),
        severity: Severity::High,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_A".into(),
            model: "m".into(),
            surface: Surface::Agent,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: chrono::Utc::now(),
        profile: FindingProfile::Full,
        report: Some(report),
        tags: Vec::new(),
    }
}

fn repo(global: &Path) -> PathBuf {
    global.join("repo")
}

/// One workspace `ws1` holding the finding `F` under coverage target `t1`.
fn seed(global: &Path) {
    let repo = repo(global);
    std::fs::create_dir_all(&repo).unwrap();
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ws1.toml"),
        format!(
            "id = \"ws1\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            repo.display()
        ),
    )
    .unwrap();
    let paths = CoveragePaths::new(&repo, "t1");
    paths.ensure_dir().unwrap();
    rupu_coverage::append_record(&paths, Ledger::Findings, &record()).unwrap();
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

fn tag_lines(global: &Path) -> Vec<Value> {
    let path = TagLog::for_workspace(&repo(global)).path;
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn findings_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn_operator_with(|_| {}, None).await;
    let table: &[(&str, &[&str])] = &[
        ("findings.list", &[]),
        ("findings.get", &["id"]),
        ("findings.tags", &[]),
        ("findings.tag", &[]),
        ("findings.export", &["format", "id"]),
        ("findings.export_many", &["format"]),
        ("findings.artifact", &["id", "sha256"]),
        ("coverage.list", &[]),
        ("coverage.get", &["target"]),
        ("coverage.audit", &["target"]),
        ("coverage.catalog", &["target"]),
        ("coverage.runs", &["target"]),
        ("coverage.diff", &["target"]),
        ("coverage.templates", &[]),
        ("coverage.template", &["name"]),
        ("assets.list", &[]),
    ];
    assert_required_args(addr, table).await;
    let list = rpc(addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let hints = |name: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"]
            .clone()
    };
    // A read-only POST is also idempotent.
    assert_eq!(hints("findings.export_many")["readOnlyHint"], json!(true));
    assert_eq!(hints("findings.export_many")["idempotentHint"], json!(true));
    assert_eq!(hints("findings.tag")["readOnlyHint"], json!(false));
    assert_eq!(hints("findings.get")["readOnlyHint"], json!(true));
}

#[tokio::test]
async fn findings_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let get_path = format!("/api/findings/{F}");
    let cases: &[(&str, Value, &str)] = &[
        ("findings.list", json!({}), "/api/findings"),
        (
            "findings.list",
            json!({"q": "severity>=high"}),
            "/api/findings?q=severity%3E%3Dhigh",
        ),
        ("findings.get", json!({"id": F}), &get_path),
        ("findings.tags", json!({}), "/api/findings/tags"),
        ("coverage.list", json!({}), "/api/coverage"),
        (
            "coverage.get",
            json!({"target": "t1", "ws_id": "ws1"}),
            "/api/coverage/t1?ws_id=ws1",
        ),
        ("coverage.templates", json!({}), "/api/coverage/templates"),
        ("assets.list", json!({}), "/api/assets"),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = http_get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
    // The seed is in the listing.
    let list = call_tool(addr, "findings.list", json!({})).await;
    assert_eq!(json_of(&list)["findings"][0]["id"], F);
}

#[tokio::test]
async fn tags_through_mcp_record_client_mcp_and_keep_via_cp() {
    let (addr, tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "findings.tag",
        json!({"finding_ids": [F], "add": ["needs-poc"]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let lines = tag_lines(tmp.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    let by = &lines[0]["by"];
    assert_eq!(by["kind"], "operator");
    assert_eq!(by["via"], "cp");
    assert_eq!(by["client"], "mcp");
    assert!(by["user"].is_string());

    // A plain HTTP POST records no `client` key.
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/findings/tags"))
        .json(&json!({"finding_ids": [F], "add": ["from-http"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let lines = tag_lines(tmp.path());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1]["by"]["via"], "cp");
    assert!(lines[1]["by"].get("client").is_none(), "{}", lines[1]);
}

#[tokio::test]
async fn a_markdown_export_is_a_text_resource() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(addr, "findings.export", json!({"id": F, "format": "md"})).await;
    assert!(!is_error(&r), "{r}");
    let res = &r["content"][0];
    assert_eq!(res["type"], "resource", "{r}");
    let text = res["resource"]["text"].as_str().expect("a text resource");
    assert!(text.contains(TITLE), "{text}");
    assert!(res["resource"]["_meta"]["name"]
        .as_str()
        .unwrap()
        .ends_with(".md"));
}

#[tokio::test]
async fn a_pdf_export_is_a_blob_resource() {
    if !cfg!(feature = "pdf") {
        eprintln!("skipped: rupu-cp built without the `pdf` feature");
        return;
    }
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(addr, "findings.export", json!({"id": F, "format": "pdf"})).await;
    assert!(!is_error(&r), "{r}");
    let res = &r["content"][0]["resource"];
    assert_eq!(res["mimeType"], "application/pdf", "{r}");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(res["blob"].as_str().expect("a blob resource"))
        .unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert!(res["_meta"]["name"].as_str().unwrap().ends_with(".pdf"));
}

#[tokio::test]
async fn an_artifact_the_finding_does_not_reference_is_an_error() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(
        addr,
        "findings.artifact",
        json!({"id": F, "sha256": OTHER_SHA}),
    )
    .await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).starts_with("HTTP 404"), "{r}");
}

#[tokio::test]
async fn export_many_answers_a_resource_and_refuses_a_bad_format() {
    let (addr, _tmp) = spawn_operator_with(seed, None).await;
    let r = call_tool(addr, "findings.export_many", json!({"format": "docx"})).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).starts_with("HTTP 400"), "{r}");
    let ok = call_tool(addr, "findings.export_many", json!({"format": "md"})).await;
    assert!(!is_error(&ok), "{ok}");
    assert_eq!(ok["content"][0]["type"], "resource");
}
