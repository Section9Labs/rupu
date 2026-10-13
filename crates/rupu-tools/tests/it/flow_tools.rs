//! The flow tools as catalog tools (W5): `board.*` and `msg.send` over a
//! run's [`Bus`], the roster over a [`CatalogPort`], `goal.*` /
//! `budget.status` over a [`RunStatus`]. Built the way a run builds them
//! ([`bodies::body`] from its context).
//!
//! [`CatalogPort`]: rupu_tools::CatalogPort
//! [`RunStatus`]: rupu_tools::RunStatus

use crate::support::{StubCatalog, StubStatus};
use rupu_fleet::{Board, Bus, BusCaps, Mailbox};
use rupu_tools::bodies::body;
use rupu_tools::ports::{
    AgentEntry, BudgetReport, BudgetSet, BudgetState, GoalCoverageReport, GoalReport, GoalRow,
    GoalTally, UsdEnforcement, UsdLine, WallClockLine, WorkflowEntry,
};
use rupu_tools::{ToolContext, ToolError, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

fn bus_ctx(bus: Bus) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.services.message_bus = Some(Arc::new(bus));
    ctx
}

/// `participant`'s context on the flow at `root`.
fn on(root: &Path, participant: &str) -> ToolContext {
    bus_ctx(Bus::open(root, participant))
}

/// Register `participant` (running `agent`) on the space at `root`.
fn join(root: &Path, participant: &str, agent: &str) {
    Bus::open(root, participant)
        .join(rupu_fleet::Member {
            agent: agent.into(),
            run_id: format!("run_{participant}"),
            kind: "flow_unit".into(),
            ..Default::default()
        })
        .unwrap();
}

async fn call(ctx: &ToolContext, name: &str, input: Value) -> ToolOutput {
    try_call(ctx, name, input).await.unwrap()
}

async fn try_call(ctx: &ToolContext, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
    body(name, ctx)
        .unwrap_or_else(|| panic!("no body for {name}"))
        .invoke(input, ctx)
        .await
}

fn json_of(out: &ToolOutput) -> Value {
    assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    serde_json::from_str(&out.stdout).unwrap()
}

// ---- board.* / msg.send ----------------------------------------------------

#[tokio::test]
async fn board_post_then_read_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let lead = on(dir.path(), "lead");
    call(
        &lead,
        "board.post",
        json!({"kind":"observation","body":"found an open port"}),
    )
    .await;
    let out = call(&lead, "board.read", json!({})).await;
    assert!(out.error.is_none(), "{out:?}");
    assert!(out.stdout.contains("found an open port"), "{}", out.stdout);
    assert!(out.stdout.contains("lead"), "author shown: {}", out.stdout);
}

#[tokio::test]
async fn board_claim_is_exclusive_and_releasable() {
    let dir = tempfile::tempdir().unwrap();
    let first = on(dir.path(), "lead");
    let second = on(dir.path(), "worker-1");
    let args = json!({"work_unit": "host:1.1.2.2"});

    let out = call(&first, "board.claim", args.clone()).await;
    assert!(out.error.is_none(), "{out:?}");
    assert!(out.stdout.starts_with("granted"), "{}", out.stdout);

    // The same participant claiming again is told it already holds it.
    let again = call(&first, "board.claim", args.clone()).await;
    assert!(again.stdout.contains("already held"), "{}", again.stdout);

    // Another participant is denied, and told who holds it.
    let denied = call(&second, "board.claim", args.clone()).await;
    assert!(denied.error.is_none(), "{denied:?}");
    assert!(denied.stdout.starts_with("denied"), "{}", denied.stdout);
    assert!(denied.stdout.contains("lead"), "{}", denied.stdout);

    // Releasing frees it for the other participant.
    let released = call(&first, "board.release", args.clone()).await;
    assert!(
        released.stdout.starts_with("released"),
        "{}",
        released.stdout
    );
    let granted = call(&second, "board.claim", args.clone()).await;
    assert!(granted.stdout.starts_with("granted"), "{}", granted.stdout);

    // Releasing something this participant does not hold is a no-op that
    // must not free the real holder's claim.
    let noop = call(&first, "board.release", args.clone()).await;
    assert!(noop.stdout.starts_with("not held"), "{}", noop.stdout);
    let still_denied = call(&first, "board.claim", args).await;
    assert!(
        still_denied.stdout.starts_with("denied") && still_denied.stdout.contains("worker-1"),
        "{}",
        still_denied.stdout
    );
}

#[tokio::test]
async fn a_claim_stands_until_the_bus_goes_away() {
    let dir = tempfile::tempdir().unwrap();
    let args = json!({"work_unit": "host:9"});
    {
        let first = on(dir.path(), "lead");
        call(&first, "board.claim", args.clone()).await;
        let denied = call(&on(dir.path(), "w"), "board.claim", args.clone()).await;
        assert!(denied.stdout.starts_with("denied"), "{}", denied.stdout);
    }
    // The lead's bus (and its claim table) was dropped: the claim is freed.
    let granted = call(&on(dir.path(), "w"), "board.claim", args).await;
    assert!(granted.stdout.starts_with("granted"), "{}", granted.stdout);
}

#[tokio::test]
async fn msg_send_then_drain() {
    let dir = tempfile::tempdir().unwrap();
    join(dir.path(), "worker-1", "recon");
    let out = call(
        &on(dir.path(), "lead"),
        "msg.send",
        json!({"to":"worker-1","body":"scan the subnet"}),
    )
    .await;
    let reply = json_of(&out);
    assert_eq!(reply["delivery"], json!({"resolved": ["worker-1"]}));
    let id = reply["id"].as_str().unwrap().to_string();
    let got = Mailbox::new(dir.path()).drain("worker-1").unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].id.as_deref(), Some(id.as_str()));
    assert_eq!(got[0].from, "lead");
    assert_eq!(got[0].body, "scan the subnet");
    assert!(!got[0].ts.is_empty());
}

fn capped(root: &Path) -> ToolContext {
    bus_ctx(Bus::open(root, "lead").with_caps(BusCaps {
        msg_cap: 1,
        ..Default::default()
    }))
}

#[tokio::test]
async fn msg_send_broadcast_to_a_full_log_is_a_visible_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = capped(dir.path());
    let args = json!({"to":"broadcast","body":"hi"});
    assert!(call(&ctx, "msg.send", args.clone()).await.error.is_none());
    let full = call(&ctx, "msg.send", args).await;
    assert!(full
        .error
        .expect("reported")
        .contains("broadcast (broadcast_full)"));
}

#[tokio::test]
async fn msg_send_to_full_inbox_is_a_visible_error_not_a_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    join(dir.path(), "worker-1", "recon");
    let ctx = capped(dir.path());
    let args = json!({"to":"worker-1","body":"hi"});
    assert!(call(&ctx, "msg.send", args.clone()).await.error.is_none());
    let full = call(&ctx, "msg.send", args).await;
    let msg = full.error.expect("a full inbox is reported to the model");
    assert!(msg.contains("worker-1 (inbox_full)"), "{msg}");
}

#[tokio::test]
async fn board_post_rejects_unknown_kind() {
    let dir = tempfile::tempdir().unwrap();
    let err = try_call(
        &on(dir.path(), "lead"),
        "board.post",
        json!({"kind":"bogus","body":"x"}),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)), "{err:?}");
    // Fail closed: nothing was posted under a defaulted kind.
    assert!(Board::new(dir.path()).read_posts().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_arguments_are_invalid_input() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    for (name, args) in [
        ("board.claim", json!({})),
        ("board.claim", json!({"work_unit": "  "})),
        ("board.release", json!({"work_unit": 7})),
        ("board.post", json!({"body": "x"})),
        ("board.post", json!({"kind": "note"})),
        (
            "board.post",
            json!({"kind":"note","body":"x","addressed_to":3}),
        ),
        ("board.read", json!({"limit": "ten"})),
        ("msg.send", json!({"to": "w"})),
        ("msg.send", json!({"body": "hi"})),
    ] {
        let err = try_call(&ctx, name, args.clone()).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "{name} {args}: {err:?}"
        );
    }
}

#[tokio::test]
async fn board_read_filters_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");

    let empty = call(&ctx, "board.read", json!({})).await;
    assert_eq!(empty.stdout, "(no posts)");

    for (body, to) in [("one", None), ("two", Some("worker-1")), ("three", None)] {
        let mut args = json!({"kind":"note","body":body});
        if let Some(to) = to {
            args["addressed_to"] = json!(to);
        }
        call(&ctx, "board.post", args).await;
    }

    let addressed = call(&ctx, "board.read", json!({"addressed_to":"worker-1"})).await;
    assert!(
        addressed.stdout.contains("two") && !addressed.stdout.contains("one"),
        "{}",
        addressed.stdout
    );

    // `limit` keeps the newest posts, still oldest-first.
    let last_two = call(&ctx, "board.read", json!({"limit": 2})).await;
    assert!(!last_two.stdout.contains("one"), "{}", last_two.stdout);
    let (two_at, three_at) = (
        last_two.stdout.find("two").unwrap(),
        last_two.stdout.find("three").unwrap(),
    );
    assert!(two_at < three_at, "{}", last_two.stdout);
}

// ---- board.directive / board.retract ----------------------------------------

#[tokio::test]
async fn board_directive_writes_a_directive_a_fresh_board_reads() {
    let dir = tempfile::tempdir().unwrap();
    let v = json_of(
        &call(
            &on(dir.path(), "lead"),
            "board.directive",
            json!({ "body": "focus on auth" }),
        )
        .await,
    );
    assert_eq!(v["ok"], true);

    let ds = Board::new(dir.path()).read_directives().unwrap();
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].author, "lead");
    assert_eq!(ds[0].body, "focus on auth");
    assert_eq!(ds[0].addressed_to, None);
    assert_eq!(ds[0].id, v["id"].as_str().unwrap());
    assert!(chrono::DateTime::parse_from_rfc3339(&ds[0].ts).is_ok());
}

/// W5 §6 test 2: a directive is authored by the participant that wrote it,
/// not a hardcoded `"lead"`.
#[tokio::test]
async fn directive_author_is_caller() {
    let dir = tempfile::tempdir().unwrap();
    call(
        &on(dir.path(), "recon#1"),
        "board.directive",
        json!({ "body": "hosts 10.0.0.0/28 are mine" }),
    )
    .await;
    let ds = Board::new(dir.path()).read_directives().unwrap();
    assert_eq!(ds.len(), 1);
    assert_eq!(ds[0].author, "recon#1");
}

#[tokio::test]
async fn board_directive_can_be_addressed_and_blank_means_everyone() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    call(
        &ctx,
        "board.directive",
        json!({ "body": "stand down", "addressed_to": "scanner" }),
    )
    .await;
    call(
        &ctx,
        "board.directive",
        json!({ "body": "regroup", "addressed_to": "  " }),
    )
    .await;
    let ds = Board::new(dir.path()).read_directives().unwrap();
    assert_eq!(ds[0].addressed_to.as_deref(), Some("scanner"));
    assert_eq!(ds[1].addressed_to, None);
}

#[tokio::test]
async fn board_directive_rejects_a_blank_body_and_a_non_string_target() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    for bad in [
        json!({}),
        json!({ "body": "   " }),
        json!({ "body": "x", "addressed_to": 7 }),
    ] {
        let err = try_call(&ctx, "board.directive", bad.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "{bad}");
    }
    assert!(Board::new(dir.path()).read_directives().unwrap().is_empty());
}

/// A bus whose board root's parent is a FILE, so its logs can't be opened.
fn blocked(dir: &tempfile::TempDir) -> ToolContext {
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    on(&blocker.join("fleet"), "lead")
}

#[tokio::test]
async fn board_directive_reports_a_store_failure_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let out = call(&blocked(&dir), "board.directive", json!({ "body": "x" })).await;
    let err = out.error.expect("a store failure is reported, not raised");
    assert!(err.starts_with("board.directive failed:"), "{err}");
}

/// The raw line count of the flow's directive log.
fn directive_log_lines(root: &Path) -> usize {
    std::fs::read_to_string(root.join("board").join("directives.jsonl"))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

#[tokio::test]
async fn board_retract_lifts_a_directive() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    let first = json_of(&call(&ctx, "board.directive", json!({ "body": "focus on auth" })).await);
    call(
        &ctx,
        "board.directive",
        json!({ "body": "expand to staging" }),
    )
    .await;
    let id = first["id"].as_str().unwrap();

    let out = json_of(&call(&ctx, "board.retract", json!({ "id": id })).await);
    assert_eq!(out["ok"], true);
    assert_eq!(out["retracted"], id);

    // A fresh handle sees only what is still live; the log only grew.
    let live = Board::new(dir.path()).read_directives().unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].body, "expand to staging");
    assert_eq!(directive_log_lines(dir.path()), 3);
}

#[tokio::test]
async fn board_retract_of_an_unknown_or_already_retracted_id_reports_and_appends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    let v = json_of(&call(&ctx, "board.directive", json!({ "body": "x" })).await);
    let id = v["id"].as_str().unwrap().to_string();

    let out = call(&ctx, "board.retract", json!({ "id": "no-such-id" })).await;
    let err = out.error.expect("an unknown id is reported to the model");
    assert!(err.contains("no-such-id"), "{err}");
    assert_eq!(directive_log_lines(dir.path()), 1, "nothing was appended");

    call(&ctx, "board.retract", json!({ "id": id })).await;
    assert_eq!(directive_log_lines(dir.path()), 2);
    let again = call(&ctx, "board.retract", json!({ "id": id })).await;
    assert!(
        again.error.is_some(),
        "already retracted: nothing to retract"
    );
    assert_eq!(
        directive_log_lines(dir.path()),
        2,
        "and nothing was appended"
    );
}

/// D22: a participant retracts only directives it wrote. Another's is
/// refused with an explicit error and stays live; nothing is appended.
#[tokio::test]
async fn retract_requires_author() {
    let dir = tempfile::tempdir().unwrap();
    let lead = on(dir.path(), "lead");
    let unit = on(dir.path(), "recon#1");
    let v = json_of(&call(&lead, "board.directive", json!({ "body": "focus on auth" })).await);
    let id = v["id"].as_str().unwrap();

    let out = call(&unit, "board.retract", json!({ "id": id })).await;
    let err = out
        .error
        .expect("another participant's directive is refused");
    assert!(
        err.starts_with(&format!("not the author of directive {id}")),
        "{err}"
    );
    assert!(err.contains("lead"), "{err}");
    assert_eq!(Board::new(dir.path()).read_directives().unwrap().len(), 1);
    assert_eq!(directive_log_lines(dir.path()), 1, "nothing was appended");

    // Its author can.
    let out = json_of(&call(&lead, "board.retract", json!({ "id": id })).await);
    assert_eq!(out["retracted"], id);
    assert!(Board::new(dir.path()).read_directives().unwrap().is_empty());
}

#[tokio::test]
async fn board_retract_rejects_a_missing_or_blank_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = on(dir.path(), "lead");
    for bad in [json!({}), json!({ "id": "   " }), json!({ "id": 7 })] {
        let err = try_call(&ctx, "board.retract", bad.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "{bad}");
    }
    assert_eq!(directive_log_lines(dir.path()), 0);
}

#[tokio::test]
async fn board_retract_reports_a_store_failure_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let out = call(&blocked(&dir), "board.retract", json!({ "id": "x" })).await;
    let err = out.error.expect("a store failure is reported, not raised");
    assert!(err.starts_with("could not read directives:"), "{err}");
}

#[test]
fn the_directive_tool_describes_retraction() {
    let d = rupu_tools::board::DIRECTIVE.description;
    assert!(!d.contains("no retraction"), "{d}");
    assert!(d.contains("board.retract"), "{d}");
}

// ---- the roster -------------------------------------------------------------

fn catalog_ctx(catalog: StubCatalog) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.services.catalog = Some(Arc::new(catalog));
    ctx
}

fn roster() -> StubCatalog {
    StubCatalog {
        agents: vec![
            AgentEntry {
                name: "recon".into(),
                description: Some("Surveys the layout of a repository.".into()),
                tools: Some(vec!["read_file".into(), "grep".into()]),
                provider: Some("anthropic".into()),
                max_turns: Some(7),
                ..Default::default()
            },
            AgentEntry {
                name: "writer".into(),
                ..Default::default()
            },
        ],
        workflows: vec![WorkflowEntry {
            id: "plain-flow".into(),
            name: "Plain Flow".into(),
            description: Some("A single placeholder step.".into()),
            scope: "global".into(),
            input_keys: vec!["topic".into()],
            step_count: 1,
            parse_error: None,
        }],
        agents_error: None,
    }
}

#[tokio::test]
async fn agents_list_and_get_are_compact() {
    let ctx = catalog_ctx(roster());
    let v = json_of(&call(&ctx, "agents.list", json!({})).await);
    assert_eq!(
        v,
        json!([
            {"name": "recon", "description": "Surveys the layout of a repository.", "tools": ["read_file", "grep"]},
            {"name": "writer", "description": null, "tools": null},
        ])
    );
    let v = json_of(&call(&ctx, "agents.get", json!({"name": "recon"})).await);
    assert_eq!(
        v,
        json!({
            "name": "recon",
            "description": "Surveys the layout of a repository.",
            "provider": "anthropic",
            "model": null,
            "tools": ["read_file", "grep"],
            "dispatchable_agents": null,
            "permission_mode": null,
            "max_turns": 7,
        })
    );
    let out = call(&ctx, "agents.get", json!({"name": "nope"})).await;
    assert_eq!(out.error.as_deref(), Some("agent not found: nope"));
    let err = try_call(&ctx, "agents.get", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}

#[tokio::test]
async fn an_agent_listing_failure_is_reported_and_search_keeps_the_workflows() {
    let ctx = catalog_ctx(StubCatalog {
        agents_error: Some("could not load agents: parse broken.md".into()),
        ..roster()
    });
    let out = call(&ctx, "agents.list", json!({})).await;
    assert_eq!(
        out.error.as_deref(),
        Some("could not load agents: parse broken.md")
    );
    let v = json_of(&call(&ctx, "catalog.search", json!({"query": "plain"})).await);
    assert_eq!(v["hits"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["warnings"],
        json!(["could not load agents: parse broken.md"])
    );
}

#[tokio::test]
async fn workflows_are_addressed_by_id() {
    let ctx = catalog_ctx(roster());
    let v = json_of(&call(&ctx, "workflows.list", json!({})).await);
    assert_eq!(
        v,
        json!([{
            "id": "plain-flow",
            "name": "Plain Flow",
            "description": "A single placeholder step.",
            "scope": "global",
            "step_count": 1,
            "parse_error": null,
        }])
    );
    let v = json_of(&call(&ctx, "workflows.get", json!({"id": "plain-flow"})).await);
    assert_eq!(v["input_keys"], json!(["topic"]));
    let out = call(&ctx, "workflows.get", json!({"id": "Plain Flow"})).await;
    assert_eq!(out.error.as_deref(), Some("workflow not found: Plain Flow"));
}

#[tokio::test]
async fn catalog_search_matches_agents_and_workflows() {
    let ctx = catalog_ctx(roster());
    let v = json_of(&call(&ctx, "catalog.search", json!({"query": "RECON"})).await);
    assert_eq!(
        v,
        json!({"hits": [{"kind": "agent", "name": "recon", "description": "Surveys the layout of a repository."}]})
    );
    for q in ["plain-flow", "plain flow", "placeholder"] {
        let v = json_of(&call(&ctx, "catalog.search", json!({"query": q})).await);
        let hits = v["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{q}: {hits:?}");
        assert_eq!(hits[0]["kind"], "workflow");
        assert_eq!(hits[0]["id"], "plain-flow");
    }
}

// ---- goal.* / budget.status -------------------------------------------------

fn status_ctx(status: StubStatus) -> ToolContext {
    let mut ctx = ToolContext::default();
    ctx.services.run_status = Some(Arc::new(status));
    ctx
}

#[tokio::test]
async fn goal_status_renders_tallies_and_per_goal_errors() {
    let ctx = status_ctx(StubStatus {
        goals: GoalReport(vec![
            GoalRow {
                id: "one".into(),
                objective: "Record findings.".into(),
                required: true,
                result: Ok(GoalTally {
                    satisfied: false,
                    current: 0,
                    target: 1,
                    detail: "0/1 findings".into(),
                }),
            },
            GoalRow {
                id: "broken".into(),
                objective: "Reach a thing.".into(),
                required: false,
                result: Err("unowned kind".into()),
            },
        ]),
        ..Default::default()
    });
    let out = call(&ctx, "goal.status", json!({})).await;
    // Keys sorted, compact: the bytes the lead has always seen.
    assert_eq!(
        out.stdout,
        r#"[{"current":0,"detail":"0/1 findings","id":"one","objective":"Record findings.","required":true,"satisfied":false,"target":1},{"error":"unowned kind","id":"broken","objective":"Reach a thing.","required":false}]"#
    );
}

#[tokio::test]
async fn goal_coverage_renders_the_target_or_null_and_reports_a_failure() {
    let ctx = status_ctx(StubStatus {
        coverage: Some(GoalCoverageReport::NoTarget),
        ..Default::default()
    });
    assert_eq!(
        call(&ctx, "goal.coverage", json!({})).await.stdout,
        r#"{"coverage":null}"#
    );
    let ctx = status_ctx(StubStatus {
        coverage: Some(GoalCoverageReport::Evaluated {
            reach: 0.5,
            depth: None,
            fraction: 0.25,
            satisfied: false,
            per_kind: vec![("network:host".into(), 0.25)],
        }),
        ..Default::default()
    });
    assert_eq!(
        call(&ctx, "goal.coverage", json!({})).await.stdout,
        r#"{"depth":null,"fraction":0.25,"per_kind":{"network:host":0.25},"reach":0.5,"satisfied":false}"#
    );
    let out = call(
        &status_ctx(StubStatus::default()),
        "goal.coverage",
        json!({}),
    )
    .await;
    assert_eq!(
        out.error.as_deref(),
        Some("could not evaluate coverage: stub")
    );
}

#[tokio::test]
async fn budget_status_renders_each_set_dimension() {
    let ctx = status_ctx(StubStatus::default());
    assert_eq!(
        call(&ctx, "budget.status", json!({})).await.stdout,
        r#"{"budget":null,"detail":"no budget set"}"#
    );
    let ctx = status_ctx(StubStatus {
        budget: Some(BudgetReport::Set(BudgetSet {
            stage: BudgetState::Hard,
            tripped: Some("usd".into()),
            soft_at: Some(0.8),
            usd: Some(UsdLine {
                cap: 1.0,
                spent: 1.5,
                enforcement: UsdEnforcement::Enforced {
                    state: BudgetState::Hard,
                    remaining: 0.0,
                },
            }),
            tokens: None,
            rounds: None,
            wall_clock: Some(WallClockLine {
                cap: "bogus".into(),
                tally: Err("bad duration".into()),
            }),
        })),
        ..Default::default()
    });
    assert_eq!(
        call(&ctx, "budget.status", json!({})).await.stdout,
        r#"{"soft_at":0.8,"stage":"hard","tripped":"usd","usd":{"cap":1.0,"enforced":true,"remaining":0.0,"spent":1.5,"state":"hard"},"wall_clock":{"cap":"bogus","error":"bad duration"}}"#
    );
}

// ---- grants ------------------------------------------------------------------

/// D22: `*` never grants the authority tools; their exact name and `board.*`
/// do.
#[test]
fn star_never_grants_directive_or_retract() {
    use rupu_tools::{AliasScope, GrantInputs, ToolCatalog};
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = on(dir.path(), "recon#1");
    ctx.services.catalog = Some(Arc::new(roster()));
    let resolve = |declared: &[&str]| {
        let declared: Vec<String> = declared.iter().map(|s| s.to_string()).collect();
        ToolCatalog::builtin()
            .resolve_grant(GrantInputs {
                declared: Some(&declared),
                step_actions: &[],
                connector_cap: None,
                ambient: &[],
                available: &ctx.services.provided(false),
                alias_scope: AliasScope::Everywhere,
            })
            .unwrap()
    };
    let star = resolve(&["*"]);
    assert!(star.offers("board.post") && star.offers("agents.list"));
    assert!(!star.offers("board.directive") && !star.offers("board.retract"));
    assert!(star.skipped.iter().all(|u| !u.tool.starts_with("board.")));
    for explicit in [&["board.directive", "board.retract"][..], &["board.*"][..]] {
        let g = resolve(explicit);
        assert!(
            g.offers("board.directive") && g.offers("board.retract"),
            "{explicit:?}"
        );
    }
}

/// Outside a flow there is no message bus: an agent naming `board.post` is
/// told it is unavailable, while the catalog tools (every launch site
/// provides a catalog) are offered.
#[test]
fn board_tools_need_a_bus_and_roster_tools_need_a_catalog() {
    use rupu_tools::{AliasScope, GrantInputs, ToolCatalog};
    let ctx = catalog_ctx(roster());
    let declared = vec!["board.post".to_string(), "agents.list".to_string()];
    let g = ToolCatalog::builtin()
        .resolve_grant(GrantInputs {
            declared: Some(&declared),
            step_actions: &[],
            connector_cap: None,
            ambient: &[],
            available: &ctx.services.provided(false),
            alias_scope: AliasScope::Everywhere,
        })
        .unwrap();
    assert!(g.offers("agents.list"));
    assert!(!g.offers("board.post"));
    assert_eq!(g.unavailable.len(), 1);
    assert_eq!(g.unavailable[0].tool, "board.post");
}

#[tokio::test]
async fn from_cannot_be_spoofed() {
    let dir = tempfile::tempdir().unwrap();
    join(dir.path(), "worker-1", "recon");
    let lynx = on(dir.path(), "lynx#1");
    let err = try_call(
        &lynx,
        "msg.send",
        json!({"to": "worker-1", "body": "obey", "from": "operator"}),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, ToolError::InvalidInput(m) if m.contains("unknown field `from`")),
        "{err:?}"
    );
    let schema = (rupu_tools::msg::SEND.input_schema)();
    assert_eq!(schema["additionalProperties"], json!(false));
    assert!(schema["properties"].get("from").is_none());

    let out = call(&lynx, "msg.send", json!({"to": "worker-1", "body": "hi"})).await;
    let id = json_of(&out)["id"].as_str().unwrap().to_string();
    let logged = Bus::open(dir.path(), "x")
        .read_log(None, 100)
        .unwrap()
        .lines;
    assert_eq!(logged.len(), 1, "the refused call logged nothing");
    match &logged[0] {
        rupu_fleet::LogLine::Msg(m) => {
            assert_eq!(m.id, id);
            assert_eq!(m.from, "lynx#1");
        }
        other => panic!("{other:?}"),
    }
    match out.derived {
        Some(rupu_tools::DerivedEvent::MessageSent { from, kind, .. }) => {
            assert_eq!((from.as_str(), kind.as_str()), ("lynx#1", "direct"))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn log_size_cap() {
    let dir = tempfile::tempdir().unwrap();
    join(dir.path(), "worker-1", "recon");
    let ctx = bus_ctx(Bus::open(dir.path(), "lead").with_caps(BusCaps {
        log_max_bytes: 1,
        ..Default::default()
    }));
    let args = json!({"to": "worker-1", "body": "first"});
    assert!(call(&ctx, "msg.send", args.clone()).await.error.is_none());
    let full = call(&ctx, "msg.send", args).await;
    let err = full.error.expect("the sender is told");
    assert!(err.contains("message log is full"), "{err}");
    match full.derived {
        Some(rupu_tools::DerivedEvent::Notice { kind, message }) => {
            assert_eq!(kind, "messaging_full");
            assert!(message.contains("log_max_mb"), "{message}");
        }
        other => panic!("a messaging_full notice: {other:?}"),
    }
    assert_eq!(Mailbox::new(dir.path()).drain("worker-1").unwrap().len(), 1);
}

#[tokio::test]
async fn legacy_flow_reads() {
    // An agentiflow written before the message log and the registry: the
    // old post shape, and a unit's inbox under its sanitized `<agent>#n`
    // name.
    let dir = tempfile::tempdir().unwrap();
    let flow = dir.path().join("agentiflows").join("af_old");
    std::fs::create_dir_all(flow.join("board")).unwrap();
    std::fs::write(
        flow.join("board").join("posts.jsonl"),
        "{\"author\":\"recon#1\",\"ts\":\"t\",\"kind\":\"observation\",\"body\":\"ssh is open\"}\n",
    )
    .unwrap();
    std::fs::create_dir_all(flow.join("mailboxes").join("recon-1")).unwrap();

    let lead = on(&flow, "lead");
    let out = call(&lead, "board.read", json!({})).await;
    assert_eq!(out.stdout, "t recon#1 [observation]: ssh is open");
    let sent = json_of(&call(&lead, "msg.send", json!({"to": "recon#1", "body": "go on"})).await);
    assert_eq!(sent["delivery"], json!({"resolved": ["recon#1"]}));
    assert_eq!(
        Mailbox::new(&flow).drain("recon#1").unwrap()[0].body,
        "go on"
    );
}
