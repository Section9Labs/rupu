//! End-to-end binary engagement: the sample `binary-analyst` agent, run under
//! the built-in `binary` profile against a scripted provider, records a
//! function-level finding that is routed to the profile, gated by its
//! completeness checks, stamped with its asset, and leaves the asset in the
//! project's asset graph — and the run manifest remembers the engagement.
//!
//! This is the capstone of the engagement-profile wiring: each layer has its
//! own unit tests, and this is the test that fails if the layers stop meeting.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts, AgentSpec};
use rupu_coverage::{
    read_asset_graph, read_findings, read_manifests, target_id, CatalogMode, ConcernsBlock,
    ConcernsEntry, CoveragePaths, FindingProfile, FindingRecord, FindingWriteOptions,
    IncludeDirective, RunManifest,
};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The sample agent the repo ships under `.rupu/agents/`. Driving the run from
/// it (rather than a hand-built option set) keeps the sample honest: if its
/// frontmatter drifts away from what the engagement needs, this test breaks.
fn sample_agent() -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.rupu/agents/binary-analyst.md")
        .canonicalize()
        .expect("the binary-analyst sample agent exists");
    AgentSpec::parse_file(&path).expect("the binary-analyst sample agent parses")
}

/// The engagement the sample agent itself declares (`engagementProfiles`),
/// resolved against the built-in registry — so a sample that stopped selecting
/// `binary` would stop activating it here too.
fn binary_engagement() -> Option<Arc<rupu_coverage::profile::ActiveSet>> {
    let set = rupu_coverage::profile::builtin_registry()
        .unwrap()
        .active_set(&sample_agent().engagement_profiles)
        .unwrap();
    Some(Arc::new(set))
}

fn stride() -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    }
}

fn function_asset() -> serde_json::Value {
    serde_json::json!({
        "kind": "binary:function",
        "locator": [{ "sha256": "ab".repeat(32) }, { "address": 4198400 }, { "symbol": "parse_header" }]
    })
}

/// A complete, valid binary report: root cause, CWE, and an evidence claim
/// whose proof is a disassembly listing.
fn report_with_listing() -> serde_json::Value {
    let mut report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    report["title"] =
        "parse_header copies a length-prefixed field into a fixed stack buffer".into();
    report["root_cause"] = "`parse_header` passes the attacker-supplied length byte to `memcpy` \
                            without comparing it to the 64-byte destination."
        .into();
    report["cwe"] = serde_json::json!(["CWE-787"]);
    report["classifications"] = serde_json::json!([]);
    report["evidence"] = serde_json::json!([{
        "claim": "The length byte reaches memcpy unchecked.",
        "binary_va": "0x401012",
        "blocks": [{
            "block": "disasm",
            "arch": "x86_64",
            "listing": [
                { "addr": "0x40100a", "text": "movzx edx, byte ptr [rsi]" },
                { "addr": "0x40100d", "text": "lea rdi, [rsp+0x10]" },
                { "addr": "0x401012", "text": "call memcpy" }
            ]
        }]
    }]);
    // The fixture's hops cite source files; this is a binary target, so
    // address them by virtual address instead.
    report["call_chain"] = serde_json::json!([
        { "label": "main", "binary_va": "0x401000", "role": "source" },
        { "label": "parse_header", "binary_va": "0x401008", "role": "hop" },
        { "label": "memcpy", "binary_va": "0x401012", "role": "sink" }
    ]);
    report
}

fn report_without_listing() -> serde_json::Value {
    let mut report = report_with_listing();
    report["evidence"][0]
        .as_object_mut()
        .unwrap()
        .remove("blocks");
    report
}

fn report_call(id: &str, report: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "report_finding".into(),
        tool_input: serde_json::json!({
            "scope": "repo",
            "asset": function_asset(),
            "report": report,
        }),
        stop: StopReason::ToolUse,
    }
}

fn asset_mark_call(id: &str, depth: &str) -> ScriptedTurn {
    let mut input = function_asset();
    input["depth"] = depth.into();
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "asset_mark".into(),
        tool_input: input,
        stop: StopReason::ToolUse,
    }
}

fn done() -> ScriptedTurn {
    ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }
}

/// Run the sample agent in `ws` under `findings`, optionally on the
/// coverage-harness (`concerns:`) path, and return the transcript text.
async fn run(
    ws: &Path,
    findings: FindingWriteOptions,
    concerns: Option<ConcernsBlock>,
    turns: Vec<ScriptedTurn>,
) -> String {
    let spec = sample_agent();
    let transcript_path = ws.join("run.jsonl");
    let opts = AgentRunOpts {
        on_usage: None,
        seed_source: None,
        agent_name: spec.name.clone(),
        agent_system_prompt: spec.system_prompt.clone(),
        agent_tools: spec.tools.clone(),
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_binary_e2e".into(),
        workspace_id: "ws_binary_e2e".into(),
        workspace_path: ws.to_path_buf(),
        transcript_path: transcript_path.clone(),
        max_turns: 8,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: ws.to_path_buf(),
            findings: Some(findings),
            ..Default::default()
        },
        user_message: "Analyse the sample binary.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns,
        scope_name: None,
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        surface_tag: Some("agent".into()),
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
        codename: None,
    };
    run_agent(opts).await.expect("run succeeds");
    std::fs::read_to_string(transcript_path).expect("transcript written")
}

fn paths(ws: &Path) -> CoveragePaths {
    CoveragePaths::new(ws, &target_id(ws, &sample_agent().name))
}

fn findings(ws: &Path) -> Vec<FindingRecord> {
    read_findings(&paths(ws)).unwrap()
}

#[test]
fn the_sample_agent_declares_the_binary_engagement() {
    let spec = sample_agent();
    assert_eq!(spec.engagement_profiles, vec!["binary"]);
    let tools = spec.tools.expect("the sample lists its tools");
    assert!(tools.iter().any(|t| t == "report_finding"), "{tools:?}");
    assert!(tools.iter().any(|t| t == "asset_mark"), "{tools:?}");
}

#[tokio::test]
async fn a_binary_finding_with_a_listing_is_routed_gated_and_persisted() {
    let ws = tempfile::TempDir::new().unwrap();
    let transcript = run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![report_call("t1", report_with_listing()), done()],
    )
    .await;

    // Recorded, with the asset stamped on the record.
    let recorded = findings(ws.path());
    assert_eq!(recorded.len(), 1, "{transcript}");
    let rec = &recorded[0];
    assert_eq!(rec.profile, FindingProfile::Full);
    let asset_ref = rec.asset.as_ref().expect("record.asset is stamped");
    assert_eq!(asset_ref.kind, "binary:function");
    assert!(rec.report.is_some(), "the full report is kept");

    // Routed: the stamped kind belongs to the `binary` profile, and `binary`
    // is the only profile this run has, so it is the one that gated the report.
    let set = binary_engagement().unwrap();
    assert_eq!(set.profile_for_kind(&asset_ref.kind).unwrap().id, "binary");

    // The asset is in the store, and the record points at it.
    let graph = read_asset_graph(&paths(ws.path()));
    let assets: Vec<_> = graph.iter().collect();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].kind, "binary:function");
    assert_eq!(assets[0].id.0, asset_ref.id);
    assert_eq!(assets[0].label, "parse_header @ 0x401000");
    assert!(
        paths(ws.path()).assets.is_file(),
        "assets.jsonl sits beside the findings ledger"
    );

    // The agent was told about the engagement and its listing requirement.
    assert!(
        transcript.contains("## Engagement profiles"),
        "{transcript}"
    );
    // The depth ladder is guidance-only text (the scripted calls never send
    // it), so its presence shows the profile's own declarations reached the
    // prompt.
    assert!(
        transcript.contains("located -> disassembled -> analyzed"),
        "{transcript}"
    );
}

#[tokio::test]
async fn a_binary_finding_without_a_listing_is_rejected_and_writes_nothing() {
    let ws = tempfile::TempDir::new().unwrap();
    let transcript = run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![report_call("t1", report_without_listing()), done()],
    )
    .await;

    // The profile's completeness check refused it, by name, to the agent.
    assert!(
        transcript.contains("engagement profile `binary` requires `evidence_has_listing`"),
        "the rejection names the profile and the unmet check: {transcript}"
    );
    // The checks the report DID meet are not reported as problems.
    assert!(
        !transcript.contains("requires `has_root_cause`"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("requires `classified`"),
        "{transcript}"
    );

    // Nothing reached either ledger: no finding, and no asset registered for
    // a finding that was never recorded.
    assert!(findings(ws.path()).is_empty());
    assert_eq!(read_asset_graph(&paths(ws.path())).iter().count(), 0);
    assert!(!paths(ws.path()).findings.exists());
    assert!(!paths(ws.path()).assets.exists());
}

#[tokio::test]
async fn a_rejected_report_can_be_fixed_and_resent() {
    let ws = tempfile::TempDir::new().unwrap();
    let transcript = run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![
            report_call("t1", report_without_listing()),
            report_call("t2", report_with_listing()),
            done(),
        ],
    )
    .await;
    // The first attempt really was refused by the gate (so the single record
    // below is the retry, not a deduplicated pair)...
    assert!(
        transcript.contains("requires `evidence_has_listing`"),
        "{transcript}"
    );
    // ...and the fixed resend is the only thing recorded.
    assert_eq!(
        findings(ws.path()).len(),
        1,
        "exactly the retry is recorded"
    );
    assert_eq!(read_asset_graph(&paths(ws.path())).iter().count(), 1);
}

#[tokio::test]
async fn asset_mark_sets_depth_and_a_later_finding_does_not_reset_it() {
    let ws = tempfile::TempDir::new().unwrap();
    // mark, THEN report on the same function with no depth, and nothing after:
    // the stored depth can only still be `analyzed` if `report_finding`'s
    // asset upsert preserved it.
    run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![
            asset_mark_call("t1", "analyzed"),
            report_call("t2", report_with_listing()),
            done(),
        ],
    )
    .await;

    let graph = read_asset_graph(&paths(ws.path()));
    let assets: Vec<_> = graph.iter().collect();
    assert_eq!(assets.len(), 1, "one function, however many touches");
    assert_eq!(assets[0].kind, "binary:function");
    assert_eq!(assets[0].depth.as_deref(), Some("analyzed"));

    let recorded = findings(ws.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].asset.as_ref().unwrap().id, assets[0].id.0);
}

#[tokio::test]
async fn a_shallower_asset_mark_is_clamped_and_the_tool_reports_the_deeper_depth() {
    let ws = tempfile::TempDir::new().unwrap();
    // Depth is monotonic: `located` after `analyzed` must not walk it back.
    let transcript = run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![
            asset_mark_call("t1", "analyzed"),
            asset_mark_call("t2", "located"),
            done(),
        ],
    )
    .await;

    let graph = read_asset_graph(&paths(ws.path()));
    let assets: Vec<_> = graph.iter().collect();
    assert_eq!(assets.len(), 1);
    assert_eq!(
        assets[0].depth.as_deref(),
        Some("analyzed"),
        "the stored depth stays at the furthest rung"
    );
    // Both tool results report `analyzed`: the second shows the clamp instead
    // of pretending `located` was set.
    assert_eq!(
        transcript.matches("depth: analyzed").count(),
        2,
        "the clamped call returns the effective depth: {transcript}"
    );
    assert!(!transcript.contains("depth: located"), "{transcript}");
}

#[tokio::test]
async fn an_unknown_depth_rung_is_refused() {
    let ws = tempfile::TempDir::new().unwrap();
    let transcript = run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        None,
        vec![asset_mark_call("t1", "exhaustively"), done()],
    )
    .await;
    assert!(
        transcript.contains("`exhaustively` is not a depth for `binary:function`"),
        "the refusal names the bad rung: {transcript}"
    );
    assert_eq!(read_asset_graph(&paths(ws.path())).iter().count(), 0);
}

/// Regression guard for the run manifest: `runs.jsonl` records the engagement
/// the run was conducted under, so a replay can rebuild it. The recording
/// leg lives in `run_agent`, and only the coverage-harness (`concerns:`) path
/// writes a manifest at all, so this runs the sample agent on that path.
#[tokio::test]
async fn the_run_manifest_records_the_engagement_profiles() {
    let ws = tempfile::TempDir::new().unwrap();
    run(
        ws.path(),
        FindingWriteOptions::default().with_engagement(binary_engagement()),
        Some(stride()),
        vec![report_call("t1", report_with_listing()), done()],
    )
    .await;

    let manifests: Vec<RunManifest> = read_manifests(&paths(ws.path())).unwrap();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].run_id, "run_binary_e2e");
    assert_eq!(manifests[0].engagement_profiles, vec!["binary"]);

    // And the finding was gated and recorded on the concerns path too.
    let recorded = findings(ws.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].asset.as_ref().map(|a| a.kind.as_str()),
        Some("binary:function")
    );
}

/// The control for the guard above: with no engagement the manifest records
/// none, so the assertion above cannot pass by accident.
#[tokio::test]
async fn a_run_without_an_engagement_records_no_engagement_profiles() {
    let ws = tempfile::TempDir::new().unwrap();
    run(
        ws.path(),
        FindingWriteOptions::default().with_profile(FindingProfile::Summary),
        Some(stride()),
        vec![done()],
    )
    .await;
    let manifests = read_manifests(&paths(ws.path())).unwrap();
    assert_eq!(manifests.len(), 1);
    assert!(manifests[0].engagement_profiles.is_empty());
}
