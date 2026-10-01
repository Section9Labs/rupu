//! `findings.record` — the MCP-side path to the findings ledger.
//!
//! The agent-side `report_finding` builtin covers agent steps. An `action:`
//! step is not an agent: it calls one MCP tool and has no builtin registry,
//! so without this tool it can observe a weakness and have nowhere to put it.

use rupu_mcp::{FindingsContext, McpPermission, ToolDispatcher};
use rupu_scm::Registry;
use std::sync::Arc;

fn ctx(workspace: &std::path::Path) -> FindingsContext {
    ctx_with(workspace, rupu_coverage::FindingProfile::Summary)
}

fn ctx_with(
    workspace: &std::path::Path,
    profile: rupu_coverage::FindingProfile,
) -> FindingsContext {
    FindingsContext {
        workspace_path: workspace.to_path_buf(),
        scope_name: "chimera-campaign".to_string(),
        run_id: "run_mcp_test".to_string(),
        model: "gpt-5.6-cyber".to_string(),
        surface: rupu_coverage::Surface::Workflow,
        options: rupu_coverage::FindingWriteOptions::default().with_profile(profile),
        codename: Some("jade-reef".to_string()),
        provider: Some("openai".to_string()),
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
    // Crew-only: one FindingsContext per workflow, not per step.
    assert_eq!(rec["declared_by"]["codename"], "jade-reef");
    assert_eq!(rec["declared_by"]["provider"], "openai");
    // An action step is not an agent: no `agent` key.
    assert!(rec["declared_by"].get("agent").is_none());
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

#[tokio::test]
async fn full_profile_records_a_report() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let out = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect("full report records");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");

    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    let text = std::fs::read_to_string(&paths.findings).expect("ledger should exist");
    let rec: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert!(
        rec["report"].is_object(),
        "report must reach the ledger: {rec}"
    );
}

#[tokio::test]
async fn full_profile_refuses_a_summary_only_call() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let err = dispatcher
        .call("findings.record", host_finding())
        .await
        .expect_err("summary-shaped call must be refused under full");
    let msg = err.to_string();
    // The author supplied `rationale`, not `evidence`; the message must
    // name the field they actually sent.
    assert!(msg.contains("derived from `report`"), "{msg}");
    assert!(
        !msg.contains("`evidence`"),
        "must name `rationale`, got: {msg}"
    );
    assert!(msg.contains("`rationale`"), "{msg}");
}

#[tokio::test]
async fn full_profile_requires_a_report() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "host", "target_ref": "h.example" }),
        )
        .await
        .expect_err("no report under full must be refused");
    let msg = err.to_string();
    assert!(msg.contains("`report` is required"), "{msg}");
    assert!(msg.contains("findings.record tool schema"), "{msg}");
}

#[tokio::test]
async fn full_profile_refuses_stray_excerpt_and_references() {
    // `code_excerpt` / `references` are derived from the report too; sending
    // them alongside a report must fail loudly, not be silently dropped.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({
                "scope": "repo",
                "report": report,
                "references": ["https://example.invalid/x"]
            }),
        )
        .await
        .expect_err("stray references must be refused under full");
    let msg = err.to_string();
    assert!(
        msg.contains("not accepted under the full findings profile"),
        "{msg}"
    );
    assert!(msg.contains("report.references"), "{msg}");
}

#[tokio::test]
async fn full_profile_refuses_a_stray_code_excerpt() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({
                "scope": "repo",
                "report": report,
                "code_excerpt": "let x = 1;"
            }),
        )
        .await
        .expect_err("stray code_excerpt must be refused under full");
    let msg = err.to_string();
    assert!(
        msg.contains("not accepted under the full findings profile"),
        "{msg}"
    );
    assert!(msg.contains("report.evidence"), "{msg}");
}

#[tokio::test]
async fn error_mapping_does_not_rewrite_an_artifact_path_named_evidence() {
    // Errors are mapped by type, not by string replace: an artifact path the
    // author happened to name `evidence` must reach them verbatim.
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tempfile::TempDir::new().unwrap();
    let mut ctx = ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full);
    ctx.options.artifact_root = Some(store.path().to_path_buf());
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx);
    let mut report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    report["artifacts"] = serde_json::json!([{ "path": "evidence" }]);
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("a missing artifact must be refused");
    let msg = err.to_string();
    assert!(msg.contains("artifact `evidence`"), "{msg}");
    assert!(!msg.contains("artifact `rationale`"), "{msg}");
}

#[tokio::test]
async fn summary_profile_missing_rationale_names_rationale() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));
    let mut bad = host_finding();
    bad.as_object_mut().unwrap().remove("rationale");
    let err = dispatcher
        .call("findings.record", bad)
        .await
        .expect_err("summary profile needs a rationale");
    let msg = err.to_string();
    assert!(msg.contains("`rationale` is required"), "{msg}");
    assert!(!msg.contains("`evidence`"), "{msg}");
}

#[tokio::test]
async fn summary_profile_refuses_a_report() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));
    let report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("report under summary must be refused");
    assert!(err.to_string().contains("summary profile"), "{err}");
}

#[tokio::test]
async fn full_profile_refuses_an_agent_supplied_verification() {
    // Verification is the verdict of a later verification run; a step cannot
    // confirm the finding it is recording.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let mut report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    report["verification"] = serde_json::json!({ "status": "confirmed" });
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("self-verification must be refused");
    let msg = err.to_string();
    assert!(msg.contains("report.verification"), "{msg}");
    assert!(msg.contains("set by verification runs"), "{msg}");
    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    assert!(!paths.findings.exists(), "nothing written on rejection");
}

#[tokio::test]
async fn a_per_call_profile_overrides_the_run_default() {
    // The dispatcher is built once per run with the run default (`full`);
    // an action step's own `findings_profile: summary` arrives per call.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    dispatcher
        .call("findings.record", host_finding())
        .await
        .expect_err("summary-shaped call is refused under the run default");
    let out = dispatcher
        .call_with_findings_profile(
            "findings.record",
            host_finding(),
            rupu_coverage::FindingProfile::Summary,
        )
        .await
        .expect("summary-shaped call records under a per-call summary profile");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");
    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    let recs = rupu_coverage::read_findings(&paths).unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].profile, rupu_coverage::FindingProfile::Summary);
}

#[tokio::test]
async fn structural_errors_name_the_field_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let mut report: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap();
    report["call_chain"][0]["role"] = serde_json::json!("entrypoint");
    let msg = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report.clone() }),
        )
        .await
        .expect_err("a bad nested variant must be refused")
        .to_string();
    assert!(msg.contains("report.call_chain[0].role"), "{msg}");
    assert!(msg.contains("unknown variant `entrypoint`"), "{msg}");

    report["call_chain"][0]["role"] = serde_json::json!("sink");
    let hop = report["call_chain"][0].as_object_mut().unwrap();
    let label = hop.remove("label").unwrap();
    hop.insert("lable".into(), label);
    let msg = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("a nested typo must be refused")
        .to_string();
    assert!(msg.contains("report.call_chain[0]"), "{msg}");
    assert!(msg.contains("lable"), "{msg}");
}
