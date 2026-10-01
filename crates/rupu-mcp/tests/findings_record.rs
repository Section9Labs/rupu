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
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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
        .call_with_findings(
            "findings.record",
            host_finding(),
            rupu_coverage::FindingProfile::Summary,
            None,
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
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
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

// ── engagement profiles (Plan 2, Task 10) ────────────────────────────────

fn binary_engagement() -> Arc<rupu_coverage::profile::ActiveSet> {
    Arc::new(
        rupu_coverage::profile::builtin_registry()
            .unwrap()
            .active_set(&["binary".to_string()])
            .unwrap(),
    )
}

/// The full-profile fixture report with a disassembly block attached to its
/// first evidence claim — what the `binary` profile's `evidence_has_listing`
/// completeness check asks for.
fn binary_report(with_listing: bool) -> serde_json::Value {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    if with_listing {
        report["evidence"][0]["blocks"] = serde_json::json!([{
            "block": "disasm",
            "arch": "x86_64",
            "listing": [{ "addr": "0x401000", "text": "push rbp" }]
        }]);
    }
    report
}

fn function_asset() -> serde_json::Value {
    serde_json::json!({
        "kind": "binary:function",
        "locator": [
            { "sha256": "ab".repeat(32) },
            { "address": 4198400 },
            { "symbol": "main" }
        ]
    })
}

fn binary_call(with_listing: bool) -> serde_json::Value {
    serde_json::json!({
        "scope": "repo",
        "report": binary_report(with_listing),
        "asset": function_asset(),
    })
}

fn ledger_paths(workspace: &std::path::Path) -> rupu_coverage::CoveragePaths {
    rupu_coverage::CoveragePaths::new(
        workspace,
        &rupu_coverage::target_id(workspace, "chimera-campaign"),
    )
}

fn full_dispatcher(workspace: &std::path::Path) -> ToolDispatcher {
    ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(workspace, rupu_coverage::FindingProfile::Full))
}

#[test]
fn the_tool_schema_advertises_an_optional_asset() {
    let spec = rupu_mcp::tool_catalog()
        .into_iter()
        .find(|s| s.name == "findings.record")
        .expect("findings.record is in the catalog");
    let asset = &spec.input_schema["properties"]["asset"];
    assert_eq!(asset["type"], "object", "{asset}");
    assert_eq!(asset["required"], serde_json::json!(["kind", "locator"]));
    for prop in ["kind", "locator", "parent", "label"] {
        assert!(asset["properties"].get(prop).is_some(), "asset.{prop}");
    }
    // Optional: a native `code` step's call shape is unchanged.
    assert_eq!(
        spec.input_schema["required"],
        serde_json::json!(["scope"]),
        "asset must not become required"
    );
}

#[tokio::test]
async fn a_per_call_engagement_routes_the_record_and_registers_its_asset() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let out = dispatcher
        .call_with_findings(
            "findings.record",
            binary_call(true),
            rupu_coverage::FindingProfile::Full,
            Some(binary_engagement()),
        )
        .await
        .expect("a binary finding with a disasm listing records");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");

    let paths = ledger_paths(tmp.path());
    let recs = rupu_coverage::read_findings(&paths).unwrap();
    assert_eq!(recs.len(), 1);
    let asset_ref = recs[0].asset.clone().expect("record.asset is stamped");
    assert_eq!(asset_ref.kind, "binary:function");
    // The asset was registered alongside, with the label the profile derives.
    let graph = rupu_coverage::read_asset_graph(&paths);
    let assets: Vec<_> = graph.iter().collect();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].id.0, asset_ref.id);
    assert_eq!(assets[0].label, "main @ 0x401000");
}

#[tokio::test]
async fn a_per_call_engagement_gates_the_record_on_the_profiles_completeness_checks() {
    // The binary profile requires a disassembly/hexdump listing; a report
    // without one is refused, and nothing reaches the ledger.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let err = dispatcher
        .call_with_findings(
            "findings.record",
            binary_call(false),
            rupu_coverage::FindingProfile::Full,
            Some(binary_engagement()),
        )
        .await
        .expect_err("the binary profile's completeness gate must apply");
    let msg = err.to_string();
    assert!(msg.contains("evidence_has_listing"), "{msg}");
    let paths = ledger_paths(tmp.path());
    assert!(!paths.findings.exists(), "nothing written on rejection");
    assert!(!paths.assets.exists(), "no asset registered on rejection");
}

#[tokio::test]
async fn a_per_call_engagement_refuses_an_asset_kind_outside_the_active_set() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let mut call = binary_call(true);
    call["asset"]["kind"] = serde_json::json!("web:route");
    let err = dispatcher
        .call_with_findings(
            "findings.record",
            call,
            rupu_coverage::FindingProfile::Full,
            Some(binary_engagement()),
        )
        .await
        .expect_err("web:route belongs to no active profile");
    assert!(err.to_string().contains("web:route"), "{err}");
    assert!(!ledger_paths(tmp.path()).findings.exists());
}

#[tokio::test]
async fn the_engagement_is_per_call_and_does_not_stick_to_the_dispatcher() {
    // The dispatcher is shared by every action step of a run. A step with an
    // engagement must not leave it behind for the next step.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let plain = || {
        serde_json::json!({
            "scope": "repo",
            "report": binary_report(false),
        })
    };

    // Engaged: the same listing-less report is gated...
    dispatcher
        .call_with_findings(
            "findings.record",
            serde_json::json!({ "scope": "repo", "report": binary_report(false) }),
            rupu_coverage::FindingProfile::Full,
            Some(binary_engagement()),
        )
        .await
        .expect_err("gated under the binary engagement");
    // ...but a following call with no engagement is the native `code` path
    // and records the identical report.
    dispatcher
        .call_with_findings(
            "findings.record",
            plain(),
            rupu_coverage::FindingProfile::Full,
            None,
        )
        .await
        .expect("no engagement: native path, unchanged");
    dispatcher
        .call("findings.record", plain())
        .await
        .expect("a plain call after an engaged one is native too");
    let recs = rupu_coverage::read_findings(&ledger_paths(tmp.path())).unwrap();
    assert_eq!(recs.len(), 2);
    assert!(recs.iter().all(|r| r.asset.is_none()), "native: no asset");
}

#[tokio::test]
async fn an_asset_without_an_engagement_is_refused_not_dropped() {
    // With no engagement `report_finding` ignores `asset` (the native `code`
    // path never routes or registers one). Over MCP the schema always
    // advertises it, so silently ignoring it would let a workflow author
    // believe an asset was recorded when none was.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let err = dispatcher
        .call_with_findings(
            "findings.record",
            binary_call(true),
            rupu_coverage::FindingProfile::Full,
            None,
        )
        .await
        .expect_err("asset needs an engagement");
    let msg = err.to_string();
    assert!(msg.contains("`asset`"), "{msg}");
    assert!(msg.contains("engagement"), "{msg}");
    assert!(!ledger_paths(tmp.path()).findings.exists());
}

#[tokio::test]
async fn a_malformed_asset_names_its_field_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = full_dispatcher(tmp.path());
    let mut call = binary_call(true);
    call["asset"]["locator"] = serde_json::json!([{ "sha265": "ab" }]);
    let err = dispatcher
        .call_with_findings(
            "findings.record",
            call,
            rupu_coverage::FindingProfile::Full,
            Some(binary_engagement()),
        )
        .await
        .expect_err("an unknown coordinate is refused");
    assert!(err.to_string().contains("asset.locator"), "{err}");
}
