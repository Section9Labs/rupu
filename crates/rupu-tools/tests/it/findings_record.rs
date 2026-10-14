//! `findings.report` called as `findings.record`, the name an `action:`
//! step uses, with the flat `rationale` / `code_excerpt` / `references` its
//! `with:` sends (docs/workflow-format.md). One implementation serves this,
//! the agent loop and `rupu mcp serve` (spec W4 §3.2).

use crate::support::{findings_ctx, Caller};
use std::sync::Arc;

fn ctx(workspace: &std::path::Path) -> rupu_tools::ToolContext {
    ctx_with(workspace, rupu_coverage::FindingProfile::Summary)
}

fn ctx_with(
    workspace: &std::path::Path,
    profile: rupu_coverage::FindingProfile,
) -> rupu_tools::ToolContext {
    findings_ctx(
        workspace,
        "chimera-campaign",
        "run_mcp_test",
        "gpt-5.6-cyber",
        profile,
        Some("jade-reef"),
        Some("openai"),
    )
}

fn options(ctx: &mut rupu_tools::ToolContext) -> &mut rupu_coverage::FindingWriteOptions {
    ctx.services.findings.as_mut().expect("findings options")
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
    let dispatcher = Caller::new(ctx(tmp.path()));

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
async fn locator_validation_applies_on_this_path_too() {
    // The same rule the agent builtin enforces. Two paths agreeing about a
    // contract only stays true when it is one path — both call
    // rupu_coverage::report_finding, so this asserts the shared enforcement
    // rather than a re-implementation.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx(tmp.path()));

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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let err = dispatcher
        .call(
            "findings.record",
            serde_json::json!({ "scope": "host", "target_ref": "h.example" }),
        )
        .await
        .expect_err("no report under full must be refused");
    let msg = err.to_string();
    assert!(msg.contains("`report` is required"), "{msg}");
    assert!(msg.contains("findings.report tool schema"), "{msg}");
}

#[tokio::test]
async fn full_profile_refuses_stray_excerpt_and_references() {
    // `code_excerpt` / `references` are derived from the report too; sending
    // them alongside a report must fail loudly, not be silently dropped.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    options(&mut ctx).artifact_root = Some(store.path().to_path_buf());
    let dispatcher = Caller::new(ctx);
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
    let dispatcher = Caller::new(ctx(tmp.path()));
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
    let dispatcher = Caller::new(ctx(tmp.path()));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
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

#[tokio::test]
async fn record_accepts_declared_tags() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx(tmp.path()));
    let mut input = host_finding();
    input["tags"] = serde_json::json!(["Class:Authz", "needs-poc"]);
    dispatcher
        .call("findings.record", input)
        .await
        .expect("record should succeed");
    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    let rec = &rupu_coverage::read_findings(&paths).unwrap()[0];
    let tags: Vec<&str> = rec.tags.iter().map(|t| t.as_str()).collect();
    assert_eq!(tags, ["class:authz", "needs-poc"]);
}

fn network_ctx(workspace: &std::path::Path) -> rupu_tools::ToolContext {
    let mut ctx = ctx_with(workspace, rupu_coverage::FindingProfile::Full);
    options(&mut ctx).engagement = Some(Arc::new(
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".into()])
            .unwrap(),
    ));
    ctx
}

fn full_report() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    )))
    .unwrap()
}

fn service_finding(coordinates: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "scope": "host",
        "target_ref": "198.51.100.7",
        "report": full_report(),
        "asset": { "kind": "network:service", "coordinates": coordinates }
    })
}

fn paths_for(workspace: &std::path::Path) -> rupu_coverage::CoveragePaths {
    rupu_coverage::CoveragePaths::new(
        workspace,
        &rupu_coverage::target_id(workspace, "chimera-campaign"),
    )
}

#[tokio::test]
async fn an_engagement_asset_is_routed_and_stamped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(network_ctx(tmp.path()));
    let out = dispatcher
        .call(
            "findings.record",
            service_finding(serde_json::json!([
                { "t": "host", "v": "198.51.100.7" },
                { "t": "port", "v": { "number": 8443, "proto": "tcp" } }
            ])),
        )
        .await
        .expect("a complete network finding records");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");

    let paths = paths_for(tmp.path());
    assert_eq!(rupu_coverage::read_findings(&paths).unwrap().len(), 1);
    let assets = rupu_coverage::read_assets(&paths.assets).unwrap();
    assert_eq!(assets.len(), 1, "the asset must be stamped: {assets:?}");
    assert_eq!(assets[0].kind, "network:service");
}

#[tokio::test]
async fn an_engagement_asset_must_pass_its_profiles_completeness() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(network_ctx(tmp.path()));
    // A service pinned to a host but no port fails `service_identified`.
    let err = dispatcher
        .call(
            "findings.record",
            service_finding(serde_json::json!([{ "t": "host", "v": "198.51.100.7" }])),
        )
        .await
        .expect_err("an incomplete network finding must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("incomplete for engagement profile `network`"),
        "{msg}"
    );
    assert!(msg.contains("host + port"), "{msg}");
    let paths = paths_for(tmp.path());
    assert!(!paths.findings.exists(), "a refused finding must not land");
    assert!(!paths.assets.exists(), "nor its asset");
}

#[tokio::test]
async fn an_asset_kind_no_active_profile_owns_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(network_ctx(tmp.path()));
    let mut input = service_finding(serde_json::json!([]));
    input["asset"]["kind"] = serde_json::json!("binary:function");
    let err = dispatcher
        .call("findings.record", input)
        .await
        .expect_err("an unowned kind must be refused");
    assert!(
        err.to_string()
            .contains("not owned by any active engagement profile (network)"),
        "{err}"
    );
    assert!(!paths_for(tmp.path()).findings.exists());
}

#[tokio::test]
async fn without_an_engagement_the_asset_is_ignored() {
    // The native code path, exactly as `report_finding` treats it.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    dispatcher
        .call(
            "findings.record",
            service_finding(serde_json::json!([{ "t": "host", "v": "198.51.100.7" }])),
        )
        .await
        .expect("records on the code path");
    let paths = paths_for(tmp.path());
    assert_eq!(rupu_coverage::read_findings(&paths).unwrap().len(), 1);
    assert!(!paths.assets.exists(), "no engagement, no asset graph");
}

#[test]
fn the_record_schema_advertises_the_asset() {
    let d = rupu_tools::ToolCatalog::builtin()
        .resolve_name("findings.record", rupu_tools::AliasScope::Everywhere)
        .unwrap();
    assert_eq!(d.name, "findings.report");
    assert_eq!(
        (d.input_schema)()["properties"]["asset"],
        rupu_coverage::asset_schema_property()
    );
}

#[tokio::test]
async fn a_misspelled_top_level_key_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx(tmp.path()));
    let mut input = host_finding();
    input["concern"] = serde_json::json!("authz");
    let msg = dispatcher
        .call("findings.report", input)
        .await
        .expect_err("an unknown key must be refused, not dropped");
    assert!(msg.contains("unknown field `concern`"), "{msg}");
    assert!(
        msg.contains("concern_id"),
        "the message lists the real keys: {msg}"
    );
    assert!(!paths_for(tmp.path()).findings.exists());
}

#[tokio::test]
async fn a_misspelled_asset_key_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(network_ctx(tmp.path()));
    let mut input = service_finding(serde_json::json!([
        { "t": "host", "v": "198.51.100.7" },
        { "t": "port", "v": { "number": 8443, "proto": "tcp" } }
    ]));
    input["asset"]["lable"] = serde_json::json!("admin console");
    let msg = dispatcher
        .call("findings.report", input)
        .await
        .expect_err("an unknown asset key must be refused");
    assert!(msg.contains("asset"), "{msg}");
    assert!(msg.contains("unknown field `lable`"), "{msg}");
    assert!(!paths_for(tmp.path()).findings.exists());
}

#[tokio::test]
async fn an_unknown_evidence_block_field_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let mut report = full_report();
    report["blocks"] = serde_json::json!([
        { "kind": "text", "text": "context" },
        { "kind": "scan_output", "tool": "nmap", "output": "8443/tcp open", "exit_code": 0 }
    ]);
    let msg = dispatcher
        .call(
            "findings.report",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("an unknown block field must be refused");
    assert!(msg.contains("report.blocks[1]"), "{msg}");
    assert!(msg.contains("unknown field `exit_code`"), "{msg}");
}

#[tokio::test]
async fn every_documented_key_is_still_accepted() {
    // Summary profile, flat `action:` form: every summary key plus the flat
    // evidence keys.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(ctx(tmp.path()));
    dispatcher
        .call(
            "findings.record",
            serde_json::json!({
                "scope": "line",
                "file_path": "src/auth.rs",
                "line_range": [10, 12],
                "summary": "Token compared with ==",
                "severity": "medium",
                "concern_id": "authz",
                "tags": ["timing"],
                "rationale": "Non-constant-time comparison.",
                "code_excerpt": "if token == expected {",
                "references": ["https://example.invalid/ct"]
            }),
        )
        .await
        .expect("flat summary form records");
    // Summary profile, `evidence` object form.
    dispatcher
        .call(
            "findings.report",
            serde_json::json!({
                "scope": "endpoint",
                "target_ref": "https://198.51.100.7:8443/admin",
                "summary": "Admin console reachable",
                "severity": "high",
                "evidence": {
                    "rationale": "Served without auth.",
                    "code_excerpt": "HTTP/1.1 200 OK",
                    "references": ["https://example.invalid/admin"]
                }
            }),
        )
        .await
        .expect("evidence object form records");
    // Full profile under an engagement, with an asset carrying every key.
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = Caller::new(network_ctx(tmp.path()));
    let mut input = service_finding(serde_json::json!([
        { "t": "host", "v": "198.51.100.7" },
        { "t": "port", "v": { "number": 8443, "proto": "tcp" } }
    ]));
    input["asset"]["label"] = serde_json::json!("admin console");
    input["concern_id"] = serde_json::json!("exposure");
    input["tags"] = serde_json::json!(["edge"]);
    dispatcher
        .call("findings.report", input)
        .await
        .expect("full form with an asset records");
}

#[tokio::test]
async fn a_cross_reference_outside_the_scope_names_the_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let other = Caller::new(findings_ctx(
        tmp.path(),
        "edge-recon",
        "run_other",
        "m",
        rupu_coverage::FindingProfile::Full,
        None,
        None,
    ));
    let out = other
        .call(
            "findings.report",
            serde_json::json!({ "scope": "repo", "report": full_report() }),
        )
        .await
        .expect("seed finding in another scope");
    let foreign = out.trim_start_matches("finding_id: ").to_string();

    let dispatcher = Caller::new(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let mut report = full_report();
    report["cross_references"] =
        serde_json::json!([{ "finding_id": foreign, "relation": "sibling" }]);
    let msg = dispatcher
        .call(
            "findings.report",
            serde_json::json!({ "scope": "repo", "report": report }),
        )
        .await
        .expect_err("a cross-reference outside the scope is refused");
    assert!(
        msg.contains(&format!(
            "`{foreign}` is not a finding in this run's findings scope `chimera-campaign`"
        )),
        "{msg}"
    );
    assert!(msg.contains("`references` or `description`"), "{msg}");
    assert!(!msg.contains("in this project"), "{msg}");
}

#[tokio::test]
async fn verifying_a_finding_outside_the_scope_names_the_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let other = Caller::new(findings_ctx(
        tmp.path(),
        "edge-recon",
        "run_other",
        "m",
        rupu_coverage::FindingProfile::Full,
        None,
        None,
    ));
    let out = other
        .call(
            "findings.report",
            serde_json::json!({ "scope": "repo", "report": full_report() }),
        )
        .await
        .expect("seed finding in another scope");
    let foreign = out.trim_start_matches("finding_id: ").to_string();

    let dispatcher = Caller::new(ctx(tmp.path()));
    let msg = dispatcher
        .call(
            "findings.verify",
            serde_json::json!({ "finding_id": foreign, "status": "confirmed" }),
        )
        .await
        .expect_err("a finding in another scope cannot be verified here");
    assert!(
        msg.contains(&format!(
            "`{foreign}` is not a finding in this run's findings scope `chimera-campaign`"
        )),
        "{msg}"
    );
    assert!(msg.contains("findings.query"), "{msg}");
}
