//! The autoflow, cron and maintenance endpoints (operator MCP Plan 3a,
//! package E) over a recording `AdminOps` stub: each endpoint reaches the
//! port with the request it was given, maps the port's refusals onto
//! 400 / 404 / 409, refuses a remote `host` with a 501, and needs the port
//! (and `cp serve`'s write gate for a write) to be installed.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use rupu_cp::admin_ops::{
    AdminOpError, AdminOps, AutoflowRunRequest, CronTickOutcome, CronTickRequest, PruneKind,
    PruneRequest, RepairRequest,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

/// What the stub was asked, in order.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Call {
    Show(String, Option<String>),
    Explain(String, Option<String>),
    Doctor(Option<String>),
    Wakes(Option<String>),
    Repair(RepairRequest),
    Run(AutoflowRunRequest),
    CronList,
    CronEvents,
    Tick(CronTickRequest),
    Prune(PruneRequest),
}

/// Answers each call with a canned report, or with `refuse` when set.
#[derive(Default)]
pub(super) struct StubOps {
    pub(super) calls: Mutex<Vec<Call>>,
    pub(super) refuse: Mutex<Option<AdminOpError>>,
}

impl StubOps {
    fn answer(&self, call: Call, report: Value) -> Result<Value, AdminOpError> {
        self.calls.lock().unwrap().push(call);
        match self.refuse.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(report),
        }
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn refuse_with(&self, e: AdminOpError) {
        *self.refuse.lock().unwrap() = Some(e);
    }
}

#[async_trait]
impl AdminOps for StubOps {
    async fn autoflow_show(&self, name: &str, repo: Option<&str>) -> Result<Value, AdminOpError> {
        self.answer(
            Call::Show(name.into(), repo.map(Into::into)),
            json!({"kind": "autoflow_show", "version": 1, "item": {"name": name}}),
        )
    }
    async fn autoflow_explain(
        &self,
        issue_ref: &str,
        repo: Option<&str>,
    ) -> Result<Value, AdminOpError> {
        self.answer(
            Call::Explain(issue_ref.into(), repo.map(Into::into)),
            json!({"kind": "autoflow_explain", "version": 1, "item": {"issue": issue_ref, "status": "unclaimed"}}),
        )
    }
    async fn autoflow_doctor(&self, repo: Option<&str>) -> Result<Value, AdminOpError> {
        self.answer(
            Call::Doctor(repo.map(Into::into)),
            json!({"kind": "autoflow_doctor", "version": 1, "ok": true, "rows": []}),
        )
    }
    async fn autoflow_wakes(&self, repo: Option<&str>) -> Result<Value, AdminOpError> {
        self.answer(
            Call::Wakes(repo.map(Into::into)),
            json!({"kind": "autoflow_wakes", "version": 1, "rows": []}),
        )
    }
    async fn autoflow_repair(&self, req: RepairRequest) -> Result<Value, AdminOpError> {
        let report = json!({"issue_ref": req.issue_ref, "dry_run": req.dry_run, "repairs": []});
        self.answer(Call::Repair(req), report)
    }
    async fn autoflow_run(&self, req: AutoflowRunRequest) -> Result<Value, AdminOpError> {
        let report = json!({"started": true, "name": req.name, "issue": req.issue});
        self.answer(Call::Run(req), report)
    }
    async fn cron_list(&self) -> Result<Value, AdminOpError> {
        self.answer(
            Call::CronList,
            json!({"kind": "cron_list", "version": 1, "rows": [{"name": "nightly", "schedule": "0 4 * * *"}]}),
        )
    }
    async fn cron_events(&self) -> Result<Value, AdminOpError> {
        self.answer(
            Call::CronEvents,
            json!({"kind": "cron_events", "version": 1, "rows": [], "summary": {"poll_sources": []}}),
        )
    }
    async fn cron_tick(&self, req: CronTickRequest) -> Result<CronTickOutcome, AdminOpError> {
        let report = json!({"kind": "cron_tick", "version": 1, "dry_run": true, "would_fire": []});
        self.answer(Call::Tick(req), report.clone())?;
        Ok(if req.dry_run {
            CronTickOutcome::DryRun(report)
        } else {
            CronTickOutcome::Started
        })
    }
    async fn prune(&self, req: PruneRequest) -> Result<Value, AdminOpError> {
        let report = json!({"kind": req.kind.as_str(), "dry_run": req.dry_run, "would_remove": [], "errors": []});
        self.answer(Call::Prune(req), report)
    }
}

pub(super) struct NoopLauncher;

#[async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_noop".into())
    }
}

/// A CP over a fresh `RUPU_HOME`: `writable` installs `cp serve`'s write
/// gate (a launcher), `ops` the port.
async fn spawn(writable: bool, ops: Option<Arc<StubOps>>) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let mut state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
            .with_admin_ops(ops.map(|o| o as Arc<dyn AdminOps>));
    if writable {
        state = state.with_launcher(Some(Arc::new(NoopLauncher)));
    }
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp)
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

fn error_of(v: &Value) -> String {
    v["error"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn autoflow_reads_reach_the_port_with_their_arguments() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;

    let (status, body) = get(addr, "/api/autoflows/fix-issues?repo=github:acme/widgets").await;
    assert_eq!((status, &body["kind"]), (200, &json!("autoflow_show")));
    let (status, _) = get(
        addr,
        "/api/autoflows/explain?issue_ref=github:acme/widgets/issues/42",
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = get(addr, "/api/autoflows/doctor").await;
    assert_eq!((status, &body["ok"]), (200, &json!(true)));
    let (status, _) = get(addr, "/api/autoflows/wakes?repo=github:acme/widgets").await;
    assert_eq!(status, 200);

    assert_eq!(
        ops.calls(),
        vec![
            Call::Show("fix-issues".into(), Some("github:acme/widgets".into())),
            Call::Explain("github:acme/widgets/issues/42".into(), None),
            Call::Doctor(None),
            Call::Wakes(Some("github:acme/widgets".into())),
        ]
    );
}

/// `/api/autoflows/:name` never shadows its literal siblings: each answers
/// from its own handler, and the claims listing still lists claims.
#[tokio::test]
async fn the_name_route_does_not_shadow_its_literal_siblings() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    assert_eq!(
        get(addr, "/api/autoflows/doctor").await.1["kind"],
        "autoflow_doctor"
    );
    assert_eq!(
        get(addr, "/api/autoflows/wakes").await.1["kind"],
        "autoflow_wakes"
    );
    let (status, claims) = get(addr, "/api/autoflows/claims").await;
    assert_eq!((status, claims), (200, json!([])));
    // `explain` without its issue is the query's 400, not a show of `explain`.
    assert_eq!(get(addr, "/api/autoflows/explain").await.0, 400);
    assert_eq!(ops.calls(), vec![Call::Doctor(None), Call::Wakes(None)]);
}

#[tokio::test]
async fn the_ports_refusals_become_their_statuses() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    for (refusal, status) in [
        (AdminOpError::Invalid("bad ref".into()), 400),
        (AdminOpError::NotFound("no autoflow named `x`".into()), 404),
        (
            AdminOpError::Conflict("several autoflows named `x`".into()),
            409,
        ),
        (AdminOpError::Failed("disk".into()), 500),
    ] {
        let message = refusal.to_string();
        ops.refuse_with(refusal);
        let (got, body) = get(addr, "/api/autoflows/x").await;
        assert_eq!((got, error_of(&body)), (status, message));
    }
}

#[tokio::test]
async fn every_endpoint_is_local_only() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    for path in [
        "/api/autoflows/fix-issues?host=peer",
        "/api/autoflows/explain?issue_ref=github:a/b/issues/1&host=peer",
        "/api/autoflows/doctor?host=peer",
        "/api/autoflows/wakes?host=peer",
        "/api/cron?host=peer",
        "/api/cron/events?host=peer",
    ] {
        let (status, body) = get(addr, path).await;
        assert_eq!(status, 501, "{path}");
        assert!(error_of(&body).contains("local-only"), "{path}: {body}");
    }
    for (path, body) in [
        (
            "/api/autoflows/fix-issues/run?host=peer",
            json!({"issue": "github:a/b/issues/1"}),
        ),
        (
            "/api/autoflows/claims/repair?host=peer",
            json!({"issue_ref": "github:a/b/issues/1"}),
        ),
        ("/api/cron/tick?host=peer", json!({})),
        (
            "/api/maintenance/prune?host=peer",
            json!({"kind": "netflow"}),
        ),
    ] {
        let (status, answer) = post(addr, path, body).await;
        assert_eq!(status, 501, "{path}");
        assert!(error_of(&answer).contains("local-only"), "{path}: {answer}");
    }
    // `local` is this control plane.
    assert_eq!(get(addr, "/api/autoflows/doctor?host=local").await.0, 200);
    assert_eq!(ops.calls(), vec![Call::Doctor(None)]);
}

#[tokio::test]
async fn without_the_port_every_endpoint_is_a_501() {
    let (addr, _tmp) = spawn(true, None).await;
    for path in ["/api/autoflows/x", "/api/autoflows/doctor", "/api/cron"] {
        let (status, body) = get(addr, path).await;
        assert_eq!(status, 501, "{path}");
        assert!(
            error_of(&body).contains("requires `rupu cp serve`"),
            "{body}"
        );
    }
    let (status, _) = post(addr, "/api/maintenance/prune", json!({"kind": "sessions"})).await;
    assert_eq!(status, 501);
}

#[tokio::test]
async fn an_autoflow_run_is_accepted_and_reaches_the_port() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    let (status, body) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": "github:acme/widgets/issues/42", "mode": "readonly"}),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(body["started"], true);
    assert_eq!(
        ops.calls(),
        vec![Call::Run(AutoflowRunRequest {
            name: "fix-issues".into(),
            issue: "github:acme/widgets/issues/42".into(),
            repo: None,
            mode: Some("readonly".into()),
        })]
    );

    ops.refuse_with(AdminOpError::Conflict(
        "already has an active autoflow cycle".into(),
    ));
    let (status, _) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": "github:acme/widgets/issues/42"}),
    )
    .await;
    assert_eq!(status, 409);
    // The issue is required.
    let (status, _) = post(addr, "/api/autoflows/fix-issues/run", json!({})).await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn writes_need_cp_serve_but_dry_runs_do_not() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(false, Some(Arc::clone(&ops))).await;
    let issue = json!({"issue": "github:acme/widgets/issues/42"});
    assert_eq!(post(addr, "/api/autoflows/x/run", issue).await.0, 501);
    let repair = json!({"issue_ref": "github:acme/widgets/issues/42", "dry_run": false});
    assert_eq!(
        post(addr, "/api/autoflows/claims/repair", repair).await.0,
        501
    );
    assert_eq!(post(addr, "/api/cron/tick", json!({})).await.0, 501);
    let prune = json!({"kind": "netflow", "dry_run": false});
    assert_eq!(post(addr, "/api/maintenance/prune", prune).await.0, 501);
    assert!(ops.calls().is_empty(), "no write reached the port");

    // Dry runs change nothing, so they need no write gate.
    let repair = json!({"issue_ref": "github:acme/widgets/issues/42"});
    assert_eq!(
        post(addr, "/api/autoflows/claims/repair", repair).await.0,
        200
    );
    assert_eq!(
        post(addr, "/api/cron/tick", json!({"dry_run": true}))
            .await
            .0,
        200
    );
    assert_eq!(
        post(addr, "/api/maintenance/prune", json!({"kind": "netflow"}))
            .await
            .0,
        200
    );
    assert_eq!(ops.calls().len(), 3);
}

#[tokio::test]
async fn repair_and_prune_default_to_a_dry_run() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    let (status, body) = post(
        addr,
        "/api/autoflows/claims/repair",
        json!({"issue_ref": "github:acme/widgets/issues/42", "release": true}),
    )
    .await;
    assert_eq!((status, &body["dry_run"]), (200, &json!(true)));
    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "transcripts", "older_than": "7d"}),
    )
    .await;
    assert_eq!((status, &body["dry_run"]), (200, &json!(true)));
    let (status, _) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "cleanup", "dry_run": false}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        ops.calls(),
        vec![
            Call::Repair(RepairRequest {
                issue_ref: "github:acme/widgets/issues/42".into(),
                release: true,
                requeue: false,
                dry_run: true,
            }),
            Call::Prune(PruneRequest {
                kind: PruneKind::Transcripts,
                older_than: Some("7d".into()),
                dry_run: true,
            }),
            Call::Prune(PruneRequest {
                kind: PruneKind::Cleanup,
                older_than: None,
                dry_run: false,
            }),
        ]
    );

    // An unknown kind is the body's 422; a bad duration the port's 400.
    let (status, _) = post(addr, "/api/maintenance/prune", json!({"kind": "logs"})).await;
    assert_eq!(status, 422);
    ops.refuse_with(AdminOpError::Invalid("invalid duration `x`".into()));
    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "netflow", "older_than": "x"}),
    )
    .await;
    assert_eq!(
        (status, error_of(&body)),
        (400, "invalid duration `x`".into())
    );
}

#[tokio::test]
async fn cron_reads_and_ticks() {
    let ops = Arc::new(StubOps::default());
    let (addr, _tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    let (status, body) = get(addr, "/api/cron").await;
    assert_eq!((status, &body["rows"][0]["name"]), (200, &json!("nightly")));
    let (status, body) = get(addr, "/api/cron/events").await;
    assert_eq!((status, &body["kind"]), (200, &json!("cron_events")));

    let (status, body) = post(addr, "/api/cron/tick", json!({"dry_run": true})).await;
    assert_eq!((status, &body["kind"]), (200, &json!("cron_tick")));
    let (status, body) = post(addr, "/api/cron/tick", json!({"skip_events": true})).await;
    assert_eq!(
        (status, body),
        (202, json!({"started": true, "dry_run": false}))
    );
    let (status, _) = post(
        addr,
        "/api/cron/tick",
        json!({"skip_events": true, "only_events": true}),
    )
    .await;
    assert_eq!(status, 400);

    ops.refuse_with(AdminOpError::Conflict(
        "a cron tick is already running".into(),
    ));
    let (status, body) = post(addr, "/api/cron/tick", json!({})).await;
    assert_eq!(
        (status, error_of(&body)),
        (409, "a cron tick is already running".into())
    );
    assert_eq!(
        ops.calls()[2..4],
        [
            Call::Tick(CronTickRequest {
                dry_run: true,
                skip_events: false,
                only_events: false,
            }),
            Call::Tick(CronTickRequest {
                dry_run: false,
                skip_events: true,
                only_events: false,
            }),
        ]
    );
}
