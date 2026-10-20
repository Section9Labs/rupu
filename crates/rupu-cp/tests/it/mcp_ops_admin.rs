//! The operations-admin operator tools (Plan 3a, package E): the autoflow
//! operations, cron and maintenance, over the `admin_api` recording stub.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::admin_api::{Call, NoopLauncher, StubOps};
use super::mcp::{assert_required_args, call_tool, json_of, rpc, text_of};
use rupu_cp::admin_ops::{AdminOps, CronTickRequest, PruneKind, PruneRequest, RepairRequest};
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// A writable CP serving `/mcp`, with `ops` as its `AdminOps` port.
async fn spawn(ops: Arc<StubOps>) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
            .with_launcher(Some(Arc::new(NoopLauncher)))
            .with_admin_ops(Some(ops as Arc<dyn AdminOps>));
    let server = rupu_cp::mcp::build_server(
        &state,
        &McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: true,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp)
}

async fn http_get(addr: SocketAddr, path: &str) -> Value {
    reqwest::get(format!("http://{addr}{path}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn is_error(r: &Value) -> bool {
    r["isError"] == json!(true)
}

#[tokio::test]
async fn admin_tools_are_listed_with_their_required_arguments() {
    let (addr, _tmp) = spawn(Arc::new(StubOps::default())).await;
    let table: &[(&str, &[&str])] = &[
        ("autoflows.get", &["name"]),
        ("autoflows.explain", &["issue_ref"]),
        ("autoflows.doctor", &[]),
        ("autoflows.wakes", &[]),
        ("autoflows.run", &["name", "issue"]),
        ("claims.repair", &["issue_ref"]),
        ("cron.list", &[]),
        ("cron.events", &[]),
        ("cron.tick", &[]),
        ("maintenance.prune", &["kind"]),
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
    assert_eq!(hints("autoflows.doctor")["readOnlyHint"], json!(true));
    assert_eq!(hints("autoflows.run")["openWorldHint"], json!(true));
    assert_eq!(hints("cron.tick")["openWorldHint"], json!(true));
    assert_eq!(hints("claims.repair")["destructiveHint"], json!(true));
    assert_eq!(hints("maintenance.prune")["destructiveHint"], json!(true));
}

#[tokio::test]
async fn admin_reads_answer_what_the_api_answers() {
    let (addr, _tmp) = spawn(Arc::new(StubOps::default())).await;
    let cases: &[(&str, Value, &str)] = &[
        (
            "autoflows.get",
            json!({"name": "fix-issues", "repo": "github:acme/widgets"}),
            "/api/autoflows/fix-issues?repo=github:acme/widgets",
        ),
        (
            "autoflows.explain",
            json!({"issue_ref": "github:acme/widgets/issues/42"}),
            "/api/autoflows/explain?issue_ref=github:acme/widgets/issues/42",
        ),
        ("autoflows.doctor", json!({}), "/api/autoflows/doctor"),
        ("autoflows.wakes", json!({}), "/api/autoflows/wakes"),
        ("cron.list", json!({}), "/api/cron"),
        ("cron.events", json!({}), "/api/cron/events"),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(addr, tool, args.clone()).await;
        assert!(!is_error(&r), "{tool}: {}", text_of(&r));
        assert_eq!(json_of(&r), http_get(addr, path).await, "{tool}");
    }
}

#[tokio::test]
async fn admin_writes_record_through_the_mcp() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(Arc::clone(&ops)).await;

    let r = call_tool(
        addr,
        "maintenance.prune",
        json!({"kind": "netflow", "older_than": "14d"}),
    )
    .await;
    assert!(!is_error(&r), "{}", text_of(&r));
    assert_eq!(json_of(&r)["dry_run"], json!(true), "a dry run by default");

    let r = call_tool(addr, "cron.tick", json!({})).await;
    assert_eq!(json_of(&r), json!({"started": true, "dry_run": false}));

    let r = call_tool(
        addr,
        "claims.repair",
        json!({"issue_ref": "github:acme/widgets/issues/42", "requeue": true, "dry_run": false}),
    )
    .await;
    assert_eq!(json_of(&r)["dry_run"], json!(false));

    let r = call_tool(
        addr,
        "autoflows.run",
        json!({"name": "fix-issues", "issue": "github:acme/widgets/issues/42"}),
    )
    .await;
    assert_eq!(json_of(&r)["started"], json!(true));

    let calls = ops.calls();
    assert_eq!(
        calls[..3],
        [
            Call::Prune(PruneRequest {
                kind: PruneKind::Netflow,
                older_than: Some("14d".into()),
                dry_run: true,
            }),
            Call::Tick(CronTickRequest::default()),
            Call::Repair(RepairRequest {
                issue_ref: "github:acme/widgets/issues/42".into(),
                release: false,
                requeue: true,
                dry_run: false,
            }),
        ]
    );
    assert!(matches!(&calls[3], Call::Run(r) if r.name == "fix-issues"));
}

#[tokio::test]
async fn a_refusal_is_an_error_result_with_its_status() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(Arc::clone(&ops)).await;
    ops.refuse_with(rupu_cp::admin_ops::AdminOpError::Conflict(
        "a cron tick is already running".into(),
    ));
    let r = call_tool(addr, "cron.tick", json!({})).await;
    assert!(is_error(&r));
    let text = text_of(&r);
    assert!(
        text.contains("409") && text.contains("already running"),
        "{text}"
    );
}

/// Review Focus 1: `autoflows.get` with a name equal to a literal sibling
/// segment is refused before any handler runs — never answered by the
/// sibling.
#[tokio::test]
async fn autoflows_get_never_reaches_a_literal_sibling() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(Arc::clone(&ops)).await;
    for name in ["doctor", "wakes", "explain", "claims"] {
        let r = call_tool(addr, "autoflows.get", json!({"name": name})).await;
        assert!(is_error(&r), "{name}: {r}");
        assert!(text_of(&r).contains("routes to"), "{name}: {}", text_of(&r));
    }
    assert!(ops.calls().is_empty(), "no sibling ran: {:?}", ops.calls());
}
