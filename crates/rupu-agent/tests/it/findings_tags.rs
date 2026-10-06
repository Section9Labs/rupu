//! `query_findings` / `tag_findings`: explicit grants that read and tag the
//! workspace's findings.

use crate::findings_without_coverage::opts_for;
use chrono::Utc;
use rupu_agent::coverage_tools::{QueryFindingsTool, TagFindingsTool};
use rupu_agent::run_agent;
use rupu_agent::runner::ScriptedTurn;
use rupu_coverage::{
    append_record, read_tag_events, read_workspace_findings, Attribution, CoveragePaths,
    FindingEvidence, FindingProfile, FindingRecord, FindingScope, Ledger, Severity, Surface,
    TagActor, TagLog,
};
use rupu_providers::types::StopReason;
use rupu_tools::{Tool, ToolContext};

fn seed(ws: &std::path::Path, id: &str, severity: Severity) {
    let rec = FindingRecord {
        id: id.into(),
        file_path: Some("src/db.rs".into()),
        line_range: Some([10, 12]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: format!("{id}: string-built SQL"),
        severity,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_earlier".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: Utc::now(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    };
    append_record(
        &CoveragePaths::new(ws, "earlier-scan"),
        Ledger::Findings,
        &rec,
    )
    .unwrap();
}

fn tag_then_stop(input: serde_json::Value) -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "t1".into(),
            tool_name: "tag_findings".into(),
            tool_input: input,
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Tagged.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

#[tokio::test]
async fn a_granted_agent_tags_a_finding_another_scope_reported() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    run_agent(opts_for(
        &ws,
        Some(vec!["tag_findings".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["Class:SQLi"]})),
    ))
    .await
    .expect("agent run should succeed");

    let events = read_tag_events(&TagLog::for_workspace(&ws)).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0].by {
        TagActor::Agent(a) => {
            assert_eq!(a.run_id, "run_findings_test");
            assert_eq!(a.agent.as_deref(), Some("net-assessor"));
        }
        other => panic!("expected an agent actor, got {other:?}"),
    }
    let f = read_workspace_findings(&ws).unwrap();
    assert_eq!(f[0].tags[0].as_str(), "class:sqli");
}

#[tokio::test]
async fn tag_findings_is_absent_when_not_granted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let _ = run_agent(opts_for(
        &ws,
        Some(vec!["read_file".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["x"]})),
    ))
    .await;
    assert!(!TagLog::for_workspace(&ws).path.exists());
}

#[tokio::test]
async fn tag_events_reach_the_run_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let stream = tmp.path().join("runs/run_findings_test/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_findings_test").unwrap();
    let mut opts = opts_for(
        &ws,
        Some(vec!["tag_findings".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["x"]})),
    );
    opts.tool_context.coverage_stream = Some(stream.clone());
    run_agent(opts).await.expect("agent run should succeed");
    let found = std::fs::read_to_string(&stream).unwrap().lines().any(|l| {
        matches!(
            serde_json::from_str::<rupu_coverage::StreamLine>(l),
            Ok(rupu_coverage::StreamLine::Tags { scope_name, .. }) if scope_name == "net-assessor"
        )
    });
    assert!(found, "a tags line must reach the stream");
}

#[tokio::test]
async fn query_findings_pages_and_lists_the_vocabulary() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    seed(&ws, "fnd_b", Severity::Low);
    let tag = TagFindingsTool::new(TagLog::for_workspace(&ws));
    let ctx = ToolContext {
        workspace_path: ws.clone(),
        ..Default::default()
    };
    let out = tag
        .invoke(
            serde_json::json!({"finding_ids": ["fnd_b"], "add": ["needs-poc"]}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);

    let q = QueryFindingsTool::new(ws.clone());
    let out = q
        .invoke(serde_json::json!({"tags": ["needs-poc"]}), &ctx)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["rows"][0]["id"], "fnd_b");
    assert_eq!(
        v["tags_in_use"][0],
        serde_json::json!({"tag": "needs-poc", "count": 1})
    );

    let out = q
        .invoke(serde_json::json!({"limit": 1}), &ctx)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["rows"][0]["id"], "fnd_a");
    assert!(v["next_cursor"].is_string());
}

#[tokio::test]
async fn bad_input_is_an_error_the_agent_can_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let ctx = ToolContext {
        workspace_path: ws.clone(),
        ..Default::default()
    };
    let tag = TagFindingsTool::new(TagLog::for_workspace(&ws));
    let out = tag
        .invoke(
            serde_json::json!({"finding_ids": ["fnd_nope"], "add": ["x"]}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.error.unwrap().contains("fnd_nope"));
    let out = tag
        .invoke(
            serde_json::json!({"finding_ids": ["fnd_a"], "add": ["not ok"]}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(out.error.unwrap().contains("invalid tag"));
    let q = QueryFindingsTool::new(ws);
    assert!(q
        .invoke(serde_json::json!({"tagz": ["x"]}), &ctx)
        .await
        .is_err());
}
