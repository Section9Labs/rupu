//! `findings.record` — the MCP-side path to the findings ledger.
//!
//! The agent-side `report_finding` builtin covers agent steps. An `action:`
//! step is not an agent: it calls one MCP tool and has no builtin registry,
//! so without this tool it can observe a weakness and have nowhere to put it.

use rupu_mcp::{FindingsContext, McpPermission, ToolDispatcher};
use rupu_scm::Registry;
use std::sync::Arc;

fn ctx(workspace: &std::path::Path) -> FindingsContext {
    FindingsContext {
        workspace_path: workspace.to_path_buf(),
        scope_name: "chimera-campaign".to_string(),
        run_id: "run_mcp_test".to_string(),
        model: "gpt-5.6-cyber".to_string(),
        surface: rupu_coverage::Surface::Workflow,
    }
}

fn host_finding() -> serde_json::Value {
    serde_json::json!({
        "scope": "host",
        "target_ref": "identity.us-westjordan-1.example",
        "summary": "Approval bypass reachable without authentication",
        "severity": "high",
        "rationale": "Observed on the live endpoint.",
        "references": ["https://example.invalid/issues/19"]
    })
}

#[tokio::test]
async fn records_a_host_finding_into_the_ledger() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));

    let out = dispatcher
        .call("findings.record", host_finding())
        .await
        .expect("record should succeed");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");

    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    let text = std::fs::read_to_string(&paths.findings).expect("ledger should exist");
    let rec: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(rec["scope"], "host");
    assert_eq!(rec["target_ref"], "identity.us-westjordan-1.example");
    assert_eq!(rec["severity"], "high");
    assert_eq!(rec["declared_by"]["run_id"], "run_mcp_test");
    assert_eq!(rec["declared_by"]["surface"], "workflow");
}

#[tokio::test]
async fn refuses_when_the_server_has_no_run_context() {
    // A dispatcher built without run context must NOT guess a workspace.
    // Filing a finding against the wrong project is worse than failing.
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all());
    let err = dispatcher
        .call("findings.record", host_finding())
        .await
        .expect_err("must refuse without context");
    let msg = err.to_string();
    assert!(
        msg.contains("without run context"),
        "error should say why, got: {msg}"
    );
}

#[tokio::test]
async fn locator_validation_applies_on_this_path_too() {
    // The same rule the agent builtin enforces. Two paths agreeing about a
    // contract only stays true when it is one path — both call
    // rupu_coverage::report_finding, so this asserts the shared enforcement
    // rather than a re-implementation.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));

    let mut bad = host_finding();
    bad.as_object_mut().unwrap().remove("target_ref");
    let err = dispatcher
        .call("findings.record", bad)
        .await
        .expect_err("host scope with no target_ref must be refused");
    assert!(err.to_string().contains("target_ref"), "got {err}");

    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    assert!(
        !paths.findings.exists(),
        "a refused finding must not reach the ledger"
    );
}
