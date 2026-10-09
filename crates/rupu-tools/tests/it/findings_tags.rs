//! `findings.query` / `findings.tag` as an `action:` step or `rupu mcp
//! serve` calls them.

use crate::support::{findings_ctx, Caller};
use rupu_tools::{CallOutcome, PermissionMode};

fn ctx(workspace: &std::path::Path) -> rupu_tools::ToolContext {
    findings_ctx(
        workspace,
        "triage-flow",
        "run_mcp_tags",
        "m",
        rupu_coverage::FindingProfile::Summary,
        Some("jade-reef"),
        None,
    )
}

async fn record(d: &Caller, summary: &str) -> String {
    let out = d
        .call(
            "findings.record",
            serde_json::json!({"scope": "repo", "summary": summary, "severity": "high", "rationale": "r"}),
        )
        .await
        .unwrap();
    out.trim_start_matches("finding_id: ").to_string()
}

#[tokio::test]
async fn tag_then_query_by_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let d = Caller::new(ctx(tmp.path()));
    let a = record(&d, "first").await;
    let _b = record(&d, "second").await;

    let out = d
        .call(
            "findings.tag",
            serde_json::json!({"finding_ids": [a], "add": ["Needs-POC"]}),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcomes"][0]["after"], serde_json::json!(["needs-poc"]));

    let out = d
        .call("findings.query", serde_json::json!({"q": "tag:needs-poc"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["rows"][0]["id"], a);

    let events =
        rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(tmp.path())).unwrap();
    match &events[0].by {
        rupu_coverage::TagActor::Agent(at) => {
            assert_eq!(at.run_id, "run_mcp_tags");
            assert_eq!(at.agent, None);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn readonly_mode_allows_findings_tag_and_query() {
    // D4 (W1): readonly denies workspace writes and external actions, never
    // rupu's own bookkeeping — `findings.tag` is a Record tool, so it reaches
    // the ledger (here: refused for an unknown id, not by the mode).
    let tmp = tempfile::TempDir::new().unwrap();
    let d = Caller::new(ctx(tmp.path())).with_mode(PermissionMode::Readonly);
    let res = d
        .outcome(
            "findings.tag",
            serde_json::json!({"finding_ids": ["fnd_x"], "add": ["x"]}),
        )
        .await;
    let CallOutcome::Failed(err) = res else {
        panic!("not blocked by the mode, refused by the ledger: {res:?}");
    };
    assert!(err.contains("unknown finding id"), "{err}");
    assert!(d
        .call("findings.query", serde_json::json!({}))
        .await
        .is_ok());
}
