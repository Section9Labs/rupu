//! The autoflow, cron and maintenance endpoints (operator MCP Plan 3a,
//! package E). The autoflow reads, repair, the run precheck, the cron
//! listings and every prune run their libraries over the test's
//! `RUPU_HOME`; only a cron tick and the spawn of an autoflow cycle reach
//! the recording `AdminOps` stub. Every endpoint maps refusals onto
//! 400 / 404 / 409 / 500, refuses a remote `host` with a 501, and a write
//! needs `cp serve`'s write gate.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use rupu_cp::admin_ops::{
    AdminOpError, AdminOps, AutoflowRunRequest, CronTickOutcome, CronTickRequest,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What the stub was asked, in order.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Call {
    Run(AutoflowRunRequest),
    Tick(CronTickRequest),
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
    async fn autoflow_run(&self, req: AutoflowRunRequest) -> Result<Value, AdminOpError> {
        let report = json!({"started": true, "name": req.name, "issue": req.issue});
        self.answer(Call::Run(req), report)
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

/// The issue every seeded autoflow sees.
pub(super) const ISSUE: &str = "github:acme/widgets/issues/42";

const FIX_ISSUES: &str = "name: fix-issues\nautoflow:\n  enabled: true\n  priority: 7\nsteps:\n  - id: decide\n    agent: echo\n    actions: []\n    prompt: hi\n";

/// Track `github:acme/widgets` at a checkout whose project layer holds the
/// `fix-issues` autoflow: what `rupu repos track` + a project's
/// `.rupu/workflows/` give a tick, and the control plane sees.
pub(super) fn seed_autoflow(global: &Path) {
    let checkout = global.join("checkouts/widgets");
    let workflows = checkout.join(".rupu/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("fix-issues.yaml"), FIX_ISSUES).unwrap();
    rupu_workspace::RepoRegistryStore {
        root: global.join("repos"),
    }
    .upsert("github:acme/widgets", &checkout, None, None)
    .unwrap();
}

/// An owned claim on `issue_ref` (another cycle holds it for an hour).
pub(super) fn seed_claim(global: &Path, issue_ref: &str) {
    let now = chrono::Utc::now();
    rupu_workspace::AutoflowClaimStore {
        root: global.join("autoflows/claims"),
    }
    .save(&rupu_workspace::AutoflowClaimRecord {
        issue_ref: issue_ref.into(),
        repo_ref: "github:acme/widgets".into(),
        source_ref: None,
        issue_display_ref: None,
        issue_title: None,
        issue_url: None,
        issue_state_name: None,
        issue_tracker: None,
        workflow: "fix-issues".into(),
        status: rupu_workspace::ClaimStatus::AwaitExternal,
        worktree_path: None,
        branch: None,
        last_run_id: None,
        last_error: None,
        last_summary: None,
        pr_url: None,
        artifacts: None,
        artifact_manifest_path: None,
        next_retry_at: None,
        claim_owner: None,
        lease_expires_at: Some((now + chrono::Duration::hours(1)).to_rfc3339()),
        pending_dispatch: None,
        contenders: vec![],
        updated_at: now.to_rfc3339(),
    })
    .unwrap();
}

/// An archived session last updated `days` ago.
fn seed_archived_session(global: &Path, id: &str, days: i64) -> std::path::PathBuf {
    let dir = global.join("sessions-archive").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let at = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let record = json!({
        "version": 1,
        "session_id": id,
        "agent_name": "a",
        "provider_name": "anthropic",
        "model": "m",
        "agent_system_prompt": "",
        "max_turns": 1,
        "permission_mode": "readonly",
        "no_stream": true,
        "workspace_id": "ws",
        "workspace_path": global,
        "transcripts_dir": global.join("transcripts"),
        "created_at": at,
        "updated_at": at,
        "status": "idle",
    });
    std::fs::write(dir.join("session.json"), record.to_string()).unwrap();
    dir
}

/// A CP over a fresh `RUPU_HOME`: `writable` installs `cp serve`'s write
/// gate (a launcher), `ops` the port.
async fn spawn(writable: bool, ops: Option<Arc<StubOps>>) -> (SocketAddr, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let mut state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
            // Never the test process's cwd: that is the rupu checkout, whose
            // `.rupu/` a prune or an autoflow read would otherwise reach.
            .with_workspace_dir(tmp.path().into())
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

/// The reads answer the reports the CLI prints with `--format json`, from
/// the library, with no port.
#[tokio::test]
async fn autoflow_reads_answer_from_the_library() {
    let (addr, tmp) = spawn(true, None).await;
    seed_autoflow(tmp.path());

    let (status, body) = get(addr, "/api/autoflows/fix-issues?repo=github:acme/widgets").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kind"], "autoflow_show");
    assert_eq!(body["item"]["name"], "fix-issues");
    assert_eq!(body["item"]["scope"], "project");
    assert_eq!(body["item"]["repo"], "github:acme/widgets");
    assert_eq!(body["item"]["priority"], 7);
    assert_eq!(body["item"]["body"], FIX_ISSUES);

    let (status, body) = get(addr, &format!("/api/autoflows/explain?issue_ref={ISSUE}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kind"], "autoflow_explain");
    assert_eq!(body["item"]["status"], "unclaimed");
    assert_eq!(body["item"]["candidate_workflows"], json!(["fix-issues"]));

    seed_claim(tmp.path(), ISSUE);
    let (status, body) = get(addr, &format!("/api/autoflows/explain?issue_ref={ISSUE}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["item"]["status"], "await_external");
    assert_eq!(body["item"]["claim"]["workflow"], "fix-issues");

    let (status, body) = get(addr, "/api/autoflows/doctor").await;
    assert_eq!((status, &body["kind"]), (200, &json!("autoflow_doctor")));
    assert_eq!(body["ok"], true, "{body}");
    let (status, body) = get(addr, "/api/autoflows/wakes?repo=github:acme/widgets").await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"kind": "autoflow_wakes", "version": 1, "rows": []})
        )
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
    assert!(ops.calls().is_empty());
}

#[tokio::test]
async fn library_refusals_become_their_statuses() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    seed_autoflow(tmp.path());

    let (status, body) = get(addr, "/api/autoflows/nope").await;
    assert_eq!(
        (status, error_of(&body)),
        (404, "no autoflow named `nope` is visible".into())
    );
    let (status, body) = get(addr, "/api/autoflows/doctor?repo=not-a-repo").await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("invalid repo filter"), "{body}");

    // Another cycle's claim: the run precheck's 409, before the port.
    seed_claim(tmp.path(), ISSUE);
    let (status, body) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": ISSUE}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert!(error_of(&body).contains("owned autoflow claim"), "{body}");
    let (status, body) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": "github:acme/widgets/issues/43", "mode": "ask"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(error_of(&body).contains("`ask`"), "{body}");
    let (status, _) = post(
        addr,
        "/api/autoflows/nope/run",
        json!({"issue": "github:acme/widgets/issues/43"}),
    )
    .await;
    assert_eq!(status, 404);
    // No claim to repair.
    let (status, _) = post(
        addr,
        "/api/autoflows/claims/repair",
        json!({"issue_ref": "github:acme/widgets/issues/9"}),
    )
    .await;
    assert_eq!(status, 404);
    assert!(ops.calls().is_empty(), "{:?}", ops.calls());
}

#[tokio::test]
async fn the_ports_refusals_become_their_statuses() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    seed_autoflow(tmp.path());
    for (refusal, status) in [
        (AdminOpError::Invalid("bad mode".into()), 400),
        (AdminOpError::Conflict("already running".into()), 409),
        (AdminOpError::Failed("disk".into()), 500),
    ] {
        let message = refusal.to_string();
        ops.refuse_with(refusal);
        let (got, body) = post(
            addr,
            "/api/autoflows/fix-issues/run",
            json!({"issue": ISSUE}),
        )
        .await;
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
    assert!(ops.calls().is_empty());
}

#[tokio::test]
async fn without_the_port_only_the_tick_and_the_cycle_spawn_are_501() {
    let (addr, tmp) = spawn(true, None).await;
    seed_autoflow(tmp.path());
    for (path, body) in [
        ("/api/autoflows/fix-issues/run", json!({"issue": ISSUE})),
        ("/api/cron/tick", json!({})),
    ] {
        let (status, answer) = post(addr, path, body).await;
        assert_eq!(status, 501, "{path}");
        assert!(
            error_of(&answer).contains("requires `rupu cp serve`"),
            "{answer}"
        );
    }

    // Everything else is a library call.
    assert_eq!(get(addr, "/api/autoflows/fix-issues").await.0, 200);
    assert_eq!(get(addr, "/api/autoflows/doctor").await.0, 200);
    assert_eq!(get(addr, "/api/autoflows/wakes").await.0, 200);
    assert_eq!(get(addr, "/api/cron").await.0, 200);
    assert_eq!(get(addr, "/api/cron/events").await.0, 200);
    for kind in ["transcripts", "sessions", "netflow", "cleanup"] {
        let (status, body) = post(addr, "/api/maintenance/prune", json!({"kind": kind})).await;
        assert_eq!(
            (status, &body["dry_run"]),
            (200, &json!(true)),
            "{kind}: {body}"
        );
    }
}

#[tokio::test]
async fn an_autoflow_run_is_prechecked_then_reaches_the_port() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    seed_autoflow(tmp.path());
    let (status, body) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": ISSUE, "mode": "readonly"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["started"], true);
    assert_eq!(
        ops.calls(),
        vec![Call::Run(AutoflowRunRequest {
            name: "fix-issues".into(),
            issue: ISSUE.into(),
            repo: None,
            mode: Some("readonly".into()),
        })]
    );
    // The issue is required.
    let (status, _) = post(addr, "/api/autoflows/fix-issues/run", json!({})).await;
    assert_eq!(status, 422);
}

#[tokio::test]
async fn writes_need_cp_serve_but_dry_runs_do_not() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(false, Some(Arc::clone(&ops))).await;
    seed_autoflow(tmp.path());
    seed_claim(tmp.path(), ISSUE);
    let issue = json!({"issue": "github:acme/widgets/issues/43"});
    assert_eq!(
        post(addr, "/api/autoflows/fix-issues/run", issue).await.0,
        501
    );
    let repair = json!({"issue_ref": ISSUE, "dry_run": false});
    assert_eq!(
        post(addr, "/api/autoflows/claims/repair", repair).await.0,
        501
    );
    assert_eq!(post(addr, "/api/cron/tick", json!({})).await.0, 501);
    for kind in ["netflow", "transcripts", "sessions", "cleanup"] {
        let prune = json!({"kind": kind, "dry_run": false});
        assert_eq!(
            post(addr, "/api/maintenance/prune", prune).await.0,
            501,
            "{kind}"
        );
    }
    assert!(ops.calls().is_empty(), "no write reached the port");

    // Dry runs change nothing, so they need no write gate.
    let repair = json!({"issue_ref": ISSUE});
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
    let prune = json!({"kind": "sessions"});
    assert_eq!(post(addr, "/api/maintenance/prune", prune).await.0, 200);
    assert_eq!(ops.calls().len(), 1, "only the tick is the port's");
}

/// A bare `42` / `#42` would resolve against `cp serve`'s working directory
/// in the CLI's parser, so the API takes only the full form.
#[tokio::test]
async fn a_bare_issue_number_is_refused_before_the_library() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    seed_autoflow(tmp.path());
    for bare in ["42", "%2342", "acme/widgets%2342"] {
        let (status, body) = get(addr, &format!("/api/autoflows/explain?issue_ref={bare}")).await;
        assert_eq!(status, 400, "{bare}: {body}");
        assert!(error_of(&body).contains("full"), "{body}");
    }
    let (status, body) = post(
        addr,
        "/api/autoflows/claims/repair",
        json!({"issue_ref": "42", "release": true, "dry_run": false}),
    )
    .await;
    assert_eq!(status, 400);
    assert!(error_of(&body).contains("full"), "{body}");
    let (status, body) = post(
        addr,
        "/api/autoflows/fix-issues/run",
        json!({"issue": "#42"}),
    )
    .await;
    assert_eq!(status, 400);
    assert!(error_of(&body).contains("full"), "{body}");
    assert!(ops.calls().is_empty(), "{:?}", ops.calls());
}

fn backdate(path: &std::path::Path, days: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400))
        .unwrap();
}

#[tokio::test]
async fn netflow_and_transcript_prunes_run_their_libraries() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    let netflow = tmp.path().join("netflow");
    std::fs::create_dir_all(&netflow).unwrap();
    let old = netflow.join("run_old.jsonl");
    let new = netflow.join("run_new.jsonl");
    std::fs::write(&old, "{}\n").unwrap();
    std::fs::write(&new, "{}\n").unwrap();
    backdate(&old, 60);

    let (status, body) = post(addr, "/api/maintenance/prune", json!({"kind": "netflow"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["would_remove"][0]["run_id"], "run_old");
    assert_eq!(body["would_remove"].as_array().unwrap().len(), 1);
    assert!(old.exists(), "a dry run deletes nothing");

    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "netflow", "dry_run": false}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["removed"][0]["status"], "deleted");
    assert!(!old.exists() && new.exists());

    // A bad argument is the caller's 400; a bad configured retention is a
    // 500 configuration error.
    for kind in ["netflow", "transcripts", "sessions", "cleanup"] {
        let (status, _) = post(
            addr,
            "/api/maintenance/prune",
            json!({"kind": kind, "older_than": "soon"}),
        )
        .await;
        assert_eq!(status, 400, "{kind}");
    }
    std::fs::write(
        tmp.path().join("config.toml"),
        "[storage]\narchived_transcript_retention = \"soon\"\narchived_session_retention = \"later\"\n",
    )
    .unwrap();
    for kind in ["transcripts", "sessions", "cleanup"] {
        let (status, body) = post(addr, "/api/maintenance/prune", json!({"kind": kind})).await;
        assert_eq!(status, 500, "{kind}: {body}");
    }
    assert!(ops.calls().is_empty(), "no prune reached the port");
}

#[tokio::test]
async fn repair_and_prune_default_to_a_dry_run() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    seed_claim(tmp.path(), ISSUE);
    let (status, body) = post(
        addr,
        "/api/autoflows/claims/repair",
        json!({"issue_ref": ISSUE, "release": true}),
    )
    .await;
    assert_eq!(
        (status, body),
        (
            200,
            json!({"issue_ref": ISSUE, "dry_run": true, "repairs": [format!("would release claim {ISSUE}")]})
        )
    );
    assert!(
        tmp.path()
            .join("autoflows/claims")
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "a dry run kept the claim"
    );
    let (status, body) = post(
        addr,
        "/api/autoflows/claims/repair",
        json!({"issue_ref": ISSUE, "release": true, "requeue": true}),
    )
    .await;
    assert_eq!(status, 400, "{body}");

    let old = seed_archived_session(tmp.path(), "ses_old", 10);
    let recent = seed_archived_session(tmp.path(), "ses_recent", 1);
    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "sessions", "older_than": "7d"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kind"], "sessions");
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["would_remove"][0]["session_id"], "ses_old");
    assert_eq!(body["would_remove"][0]["action"], "would_delete");
    assert_eq!(body["would_remove"].as_array().unwrap().len(), 1);
    assert!(old.exists(), "a dry run deletes nothing");

    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "cleanup", "older_than": "7d", "dry_run": false}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["removed"],
        json!([{
            "kind": "session",
            "id": "ses_old",
            "scope": "archived",
            "retained_at": body["removed"][0]["retained_at"],
            "action": "deleted",
            "detail": "status idle",
        }])
    );
    assert!(!old.exists() && recent.exists());

    // An unknown kind is the body's 422.
    let (status, _) = post(addr, "/api/maintenance/prune", json!({"kind": "logs"})).await;
    assert_eq!(status, 422);
    let (status, body) = post(
        addr,
        "/api/maintenance/prune",
        json!({"kind": "sessions", "older_than": "x"}),
    )
    .await;
    assert_eq!(
        (status, error_of(&body)),
        (
            400,
            "invalid duration `x`: cannot parse integer from empty string".into()
        )
    );
    assert!(ops.calls().is_empty());
}

const CRON_WF: &str = "name: nightly\ntrigger:\n  on: cron\n  cron: \"0 4 * * *\"\nsteps:\n  - id: one\n    agent: a\n    actions: []\n    prompt: hi\n";

#[tokio::test]
async fn cron_reads_and_ticks() {
    let ops = Arc::new(StubOps::default());
    let (addr, tmp) = spawn(true, Some(Arc::clone(&ops))).await;
    let wfs = tmp.path().join("workflows");
    std::fs::create_dir_all(&wfs).unwrap();
    std::fs::write(wfs.join("nightly.yaml"), CRON_WF).unwrap();

    let (status, body) = get(addr, "/api/cron").await;
    assert_eq!(status, 200);
    assert_eq!(body["kind"], "cron_list");
    assert_eq!(body["rows"][0]["name"], "nightly");
    assert_eq!(body["rows"][0]["schedule"], "0 4 * * *");
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
        ops.calls()[..2],
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
