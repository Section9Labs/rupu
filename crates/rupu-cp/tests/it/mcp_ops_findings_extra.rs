//! The operator tools over the findings import and schema, coverage gap and
//! rerun, and query-help endpoints.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::findings_extra_api::{
    get, post, repo, seed, serve, write_report, CapturingLauncher, REPORTED, RUN, TARGET,
};
use super::mcp::{assert_required_args, call_tool, json_of, rpc, text_of};
use rupu_coverage::CoveragePaths;
use serde_json::{json, Value};
use std::sync::Arc;

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn findings_extra_tools_are_listed_with_their_required_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = serve(tmp.path(), false, None).await;
    let table: &[(&str, &[&str])] = &[
        ("findings.import", &["paths"]),
        ("findings.schema", &[]),
        ("coverage.gap", &["target"]),
        ("coverage.rerun", &["run", "target"]),
        ("query.fields", &["list"]),
        ("query.check", &["list", "q"]),
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
    assert_eq!(hints("findings.schema")["readOnlyHint"], json!(true));
    assert_eq!(hints("coverage.gap")["readOnlyHint"], json!(true));
    assert_eq!(hints("query.fields")["readOnlyHint"], json!(true));
    // A check is a POST that changes nothing.
    assert_eq!(hints("query.check")["readOnlyHint"], json!(true));
    assert_eq!(hints("query.check")["idempotentHint"], json!(true));
    // A rerun starts a model run.
    assert_eq!(hints("coverage.rerun")["openWorldHint"], json!(true));
    assert_eq!(hints("coverage.rerun")["readOnlyHint"], json!(false));
    assert_eq!(hints("findings.import")["readOnlyHint"], json!(false));
}

#[tokio::test]
async fn findings_extra_reads_answer_what_the_api_answers() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = serve(tmp.path(), false, None).await;
    let gap = format!("/api/coverage/{TARGET}/gap?ws_id=ws1");
    let cases: &[(&str, Value, &str)] = &[
        ("findings.schema", json!({}), "/api/findings/schema"),
        (
            "findings.schema",
            json!({"advertised": true}),
            "/api/findings/schema?advertised=true",
        ),
        (
            "coverage.gap",
            json!({"target": TARGET, "ws_id": "ws1"}),
            &gap,
        ),
        (
            "query.fields",
            json!({"list": "findings"}),
            "/api/query/findings/fields",
        ),
        (
            "query.fields",
            json!({"list": "agent_runs"}),
            "/api/query/agent_runs/fields",
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        let (status, want) = get(addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}");
        assert!(!is_error(&r), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool} {args}");
    }
    // The check is a read too: the same answer as the POST.
    let r = call_tool(
        addr,
        "query.check",
        json!({"list": "findings", "q": "severity>=high has:poc"}),
    )
    .await;
    let (_, want) = post(
        addr,
        "/api/query/findings/check",
        json!({"q": "severity>=high has:poc"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r), want);
    assert_eq!(want["ok"], true);
    // A query error is a successful check, not a tool error.
    let r = call_tool(
        addr,
        "query.check",
        json!({"list": "findings", "q": "severity>=urgent"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["ok"], false);
    assert_eq!(json_of(&r)["code"], "bad_value");
}

#[tokio::test]
async fn an_import_through_mcp_attaches_the_report() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let report = write_report(tmp.path());
    let addr = serve(tmp.path(), true, None).await;
    let r = call_tool(
        addr,
        "findings.import",
        json!({"paths": [report.display().to_string()]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["attached"], 1, "{r}");
    let ledger =
        std::fs::read_to_string(CoveragePaths::new(&repo(tmp.path()), TARGET).findings).unwrap();
    let line: Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
    assert_eq!(line["id"], REPORTED);
    assert_eq!(line["profile"], "full");
    // A relative path is the API's 400.
    let r = call_tool(addr, "findings.import", json!({"paths": ["NB-001.md"]})).await;
    assert!(is_error(&r), "{r}");
    assert!(text_of(&r).starts_with("HTTP 400"), "{r}");
}

#[tokio::test]
async fn a_rerun_through_mcp_starts_the_replay() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let launcher = Arc::new(CapturingLauncher::default());
    let addr = serve(tmp.path(), true, Some(launcher.clone())).await;
    let r = call_tool(
        addr,
        "coverage.rerun",
        json!({"target": TARGET, "run": RUN}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(json_of(&r)["replays"], RUN, "{r}");
    let seen = launcher.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].agent, "notes-reviewer");
}

/// Review Focus 1: a path argument equal to a sibling route's literal
/// segment is refused before any handler runs.
#[tokio::test]
async fn a_path_argument_naming_a_sibling_route_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let launcher = Arc::new(CapturingLauncher::default());
    let addr = serve(tmp.path(), true, Some(launcher.clone())).await;
    let cases = [
        // `/api/coverage/templates/gap` is the template named `gap`.
        ("coverage.gap", json!({"target": "templates"})),
        ("coverage.rerun", json!({"target": "templates", "run": RUN})),
        // `/api/findings/schema` and `/api/findings/import` are literals.
        ("findings.get", json!({"id": "schema"})),
        ("findings.get", json!({"id": "import"})),
    ];
    for (tool, args) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        assert!(is_error(&r), "{tool} {args}: {r}");
        assert!(
            !text_of(&r).starts_with("HTTP 2"),
            "{tool} {args} reached a sibling: {r}"
        );
    }
    let r = call_tool(addr, "coverage.gap", json!({"target": "templates"})).await;
    assert!(text_of(&r).contains("routes to"), "{r}");
    let r = call_tool(addr, "findings.get", json!({"id": "schema"})).await;
    assert!(text_of(&r).contains("routes to"), "{r}");
    assert!(launcher.seen.lock().unwrap().is_empty());
}
