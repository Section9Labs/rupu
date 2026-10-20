//! The findings, coverage and query-help endpoints the CLI had alone:
//! `POST /api/findings/import`, `GET /api/findings/schema`,
//! `GET /api/coverage/:target/gap`, `POST /api/coverage/:target/rerun`,
//! `GET /api/query/:list/fields` and `POST /api/query/:list/check`.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use rupu_coverage::{
    Attribution, Concern, ConcernsBlock, CoveragePaths, FindingEvidence, FindingProfile,
    FindingRecord, FindingScope, FlatCatalog, RunManifest, Severity, Surface,
};
use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The finding the invented Notebin report cites on its `Finding ID:` line.
pub(super) const REPORTED: &str = "fnd_01J00000000000000000000001";
pub(super) const REPORT: &str =
    include_str!("../../../rupu-findings-report/tests/fixtures/import/notebin_plain.md");
pub(super) const TARGET: &str = "tgt1";
pub(super) const RUN: &str = "run_01JREPLAYAGENT0000000000001";
const SESSION_RUN: &str = "run_01JREPLAYSESSION00000000001";
const CONTINUED_RUN: &str = "run_01JREPLAYCONTINUED000000001";
const ELSEWHERE_RUN: &str = "run_01JREPLAYELSEWHERE000000001";

/// An agent launcher that records what it was asked to start.
#[derive(Default)]
pub(super) struct CapturingLauncher {
    pub(super) seen: Mutex<Vec<AgentLaunchRequest>>,
}

#[async_trait::async_trait]
impl AgentLauncher for CapturingLauncher {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        self.seen.lock().unwrap().push(req);
        Ok("run_01JREPLAYED0000000000000001".into())
    }
}

struct NoopRunLauncher;

#[async_trait::async_trait]
impl rupu_cp::launcher::RunLauncher for NoopRunLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

/// The repo the workspace `ws1` is registered at (canonical, so a manifest's
/// recorded path compares equal).
pub(super) fn repo(global: &Path) -> PathBuf {
    global.join("repo").canonicalize().unwrap()
}

fn summary_record(id: &str) -> FindingRecord {
    FindingRecord {
        id: id.to_string(),
        file_path: Some("src/routes/notes.rs".to_string()),
        line_range: Some([40, 58]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: "note lookup skips the owner check".to_string(),
        severity: Severity::Medium,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "why".to_string(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: RUN.to_string(),
            model: "m".to_string(),
            surface: Surface::Agent,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: "2026-09-29T00:00:00Z".parse().unwrap(),
        profile: FindingProfile::Summary,
        report: None,
        tags: Vec::new(),
    }
}

fn manifest(run_id: &str, surface: Surface, workspace: &Path) -> RunManifest {
    RunManifest {
        run_id: run_id.to_string(),
        started_at: "2026-10-01T00:00:00Z".parse().unwrap(),
        surface,
        agent_name: "notes-reviewer".to_string(),
        provider: "anthropic".to_string(),
        model: "m".to_string(),
        permission_mode: "readonly".to_string(),
        user_prompt: "Review the notes API for access-control gaps.".to_string(),
        concerns: ConcernsBlock::default(),
        scope_name: "notes-reviewer".to_string(),
        workspace_path: workspace.to_path_buf(),
        continued_from: None,
        skills: vec!["http-probing".to_string()],
        preload_skills: vec!["house-style".to_string()],
    }
}

/// Workspace `ws1` at `<global>/repo` with coverage target [`TARGET`]:
/// - a summary finding [`REPORTED`] waiting for its report;
/// - two concerns (`authz` on `src/**`, `logging` everywhere) over three
///   touched files, of which only `src/routes/notes.rs` is asserted for
///   `authz`, so `authz` has one gap and `logging` three;
/// - run manifests: an agent run, a session run, a continued run and one
///   recorded in another workspace.
pub(super) fn seed(global: &Path) {
    std::fs::create_dir_all(global.join("repo")).unwrap();
    let repo = repo(global);
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ws1.toml"),
        format!(
            "id = \"ws1\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            repo.display()
        ),
    )
    .unwrap();
    let paths = CoveragePaths::new(&repo, TARGET);
    paths.ensure_dir().unwrap();
    std::fs::write(
        &paths.findings,
        serde_json::to_string(&summary_record(REPORTED)).unwrap() + "\n",
    )
    .unwrap();

    let concern = |id: &str, globs: &[&str]| Concern {
        id: id.to_string(),
        name: id.to_string(),
        description: format!("{id} checks"),
        severity: Severity::High,
        applicable_globs: globs.iter().map(|g| g.to_string()).collect(),
        min_strength: rupu_coverage::TouchStrength::Read,
        references: vec![],
        tags: vec![],
    };
    rupu_coverage::write_snapshot(
        &FlatCatalog {
            concerns: vec![concern("authz", &["src/**"]), concern("logging", &["**"])],
            sources: Default::default(),
            render_modes: Default::default(),
        },
        &paths.catalog,
    )
    .unwrap();
    let read = |path: &str| {
        format!(
            "{{\"kind\":\"read\",\"path\":\"{path}\",\"line_range\":[1,40],\"tool\":\"read_file\",\
             \"run_id\":\"{RUN}\",\"model\":\"m\",\"surface\":\"agent\",\"at\":\"2026-10-01T00:00:00Z\"}}\n"
        )
    };
    let files: String = ["README.md", "src/routes/notes.rs", "src/routes/share.rs"]
        .iter()
        .map(|p| read(p))
        .collect();
    std::fs::write(&paths.files, files).unwrap();
    let assertion = json!({
        "concern_id": "authz",
        "file_path": "src/routes/notes.rs",
        "status": "finding",
        "evidence": {"summary": "owner check missing", "finding_ids": [REPORTED]},
        "declared_by": {"run_id": RUN, "model": "m", "surface": "agent"},
        "declared_at": "2026-10-01T00:00:00Z",
    });
    std::fs::write(&paths.concerns, format!("{assertion}\n")).unwrap();

    let mut continued = manifest(CONTINUED_RUN, Surface::Agent, &repo);
    continued.continued_from = Some(format!("/runs/{RUN}.jsonl"));
    for m in [
        manifest(RUN, Surface::Agent, &repo),
        manifest(SESSION_RUN, Surface::Session, &repo),
        continued,
        manifest(
            ELSEWHERE_RUN,
            Surface::Agent,
            Path::new("/srv/other-checkout"),
        ),
    ] {
        rupu_coverage::append_manifest(&paths, &m).unwrap();
    }
}

/// A CP over `global` serving `/api` and `/mcp`. `writable` installs the
/// launchers `rupu cp serve` has; `agent` is its agent launcher.
pub(super) async fn serve(
    global: &Path,
    writable: bool,
    agent: Option<Arc<CapturingLauncher>>,
) -> SocketAddr {
    let mut state =
        rupu_cp::state::AppState::new(global.to_path_buf(), rupu_config::PricingConfig::default());
    if writable {
        state = state.with_launcher(Some(Arc::new(NoopRunLauncher)));
    }
    if let Some(agent) = agent {
        state = state.with_agent_launcher(Some(agent));
    }
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
            web: false,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

pub(super) async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

pub(super) async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn ledger(global: &Path) -> String {
    std::fs::read_to_string(CoveragePaths::new(&repo(global), TARGET).findings).unwrap()
}

fn backups(global: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(CoveragePaths::new(&repo(global), TARGET).root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains("pre-import"))
        .collect()
}

/// A report file at `<global>/reports/NB-001.md`.
pub(super) fn write_report(global: &Path) -> PathBuf {
    let dir = global.join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("NB-001.md"), REPORT).unwrap();
    std::fs::write(dir.join("README.md"), "# Reports\n\nOne per finding.\n").unwrap();
    dir.join("NB-001.md")
}

// ── findings schema ──────────────────────────────────────────────────────

#[tokio::test]
async fn the_schema_is_the_embedded_one_or_its_advertised_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = serve(tmp.path(), false, None).await;
    let (status, canonical) = get(addr, "/api/findings/schema").await;
    assert_eq!(status, 200);
    assert_eq!(canonical, rupu_coverage::report::schema::canonical_schema());
    assert_eq!(
        canonical["$schema"],
        "http://json-schema.org/draft-07/schema#"
    );
    let (status, advertised) = get(addr, "/api/findings/schema?advertised=true").await;
    assert_eq!(status, 200);
    assert_eq!(
        advertised,
        rupu_coverage::report::schema::advertised_schema()
    );
    assert_ne!(advertised, canonical);
}

// ── findings import ──────────────────────────────────────────────────────

#[tokio::test]
async fn a_dry_run_import_reports_what_would_attach_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let report = write_report(tmp.path());
    let before = ledger(tmp.path());
    // A dry run needs no writable control plane.
    let addr = serve(tmp.path(), false, None).await;
    let dir = report.parent().unwrap().display().to_string();
    let (status, body) = post(
        addr,
        "/api/findings/import",
        json!({"paths": [dir], "dry_run": true}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({
            "dry_run": true,
            "files": [
                {"path": report.display().to_string(), "status": "attached", "finding_id": REPORTED},
                {
                    "path": report.with_file_name("README.md").display().to_string(),
                    "status": "skipped",
                    "reason": "not a finding report",
                },
            ],
            "backups": [],
            "attached": 1,
            "skipped": 1,
            "failed": 0,
        })
    );
    assert_eq!(ledger(tmp.path()), before);
    assert!(backups(tmp.path()).is_empty());
}

#[tokio::test]
async fn an_import_attaches_the_report_and_backs_the_ledger_up_first() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let report = write_report(tmp.path());
    let before = ledger(tmp.path());
    let addr = serve(tmp.path(), true, None).await;
    let (status, body) = post(
        addr,
        "/api/findings/import",
        json!({"paths": [report.display().to_string()]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["attached"], 1, "{body}");
    assert_eq!(body["files"][0]["status"], "attached");
    let after: Value = serde_json::from_str(ledger(tmp.path()).lines().next().unwrap()).unwrap();
    assert_eq!(after["profile"], "full");
    assert_eq!(
        after["report"]["title"],
        "Notes API returns another user's note by id"
    );
    let backups = backups(tmp.path());
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), before);
    assert_eq!(body["backups"], json!([backups[0].display().to_string()]));

    // Imported again, the finding already has its report: skipped, and no
    // second backup.
    let (status, again) = post(
        addr,
        "/api/findings/import",
        json!({"paths": [report.display().to_string()]}),
    )
    .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(
        again["files"][0]["reason"],
        format!("{REPORTED} already has a report")
    );
    assert_eq!(again["skipped"], 1);
}

#[tokio::test]
async fn a_report_for_no_known_finding_is_a_failed_line_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let file = tmp.path().join("other.md");
    std::fs::write(
        &file,
        REPORT.replace(REPORTED, "fnd_01J00000000000000000000009"),
    )
    .unwrap();
    let addr = serve(tmp.path(), true, None).await;
    let (status, body) = post(
        addr,
        "/api/findings/import",
        json!({"paths": [file.display().to_string()]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["failed"], 1);
    assert_eq!(
        body["files"][0]["reason"],
        "no finding fnd_01J00000000000000000000009 in any registered project"
    );
}

#[tokio::test]
async fn an_import_refuses_what_it_cannot_take() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let report = write_report(tmp.path());
    let addr = serve(tmp.path(), true, None).await;
    let dir = report.parent().unwrap().display().to_string();
    let missing = tmp.path().join("missing.md").display().to_string();
    let cases = [
        (json!({"paths": []}), "names no report"),
        (
            json!({"paths": ["reports/NB-001.md"]}),
            "is not an absolute path",
        ),
        (json!({"paths": [missing]}), "No such file or directory"),
        (
            json!({"paths": [dir], "id": REPORTED}),
            "`id` needs exactly one report file",
        ),
        (
            json!({"paths": [report.display().to_string()], "id": " "}),
            "`id` is blank",
        ),
    ];
    for (body, says) in cases {
        let (status, resp) = post(addr, "/api/findings/import", body.clone()).await;
        assert_eq!(status, 400, "{body}: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains(says),
            "{body}: {resp}"
        );
    }
    // An unknown field is refused rather than ignored.
    let r = reqwest::Client::new()
        .post(format!("http://{addr}/api/findings/import"))
        .json(&json!({"paths": [dir], "dryrun": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 422);
}

#[tokio::test]
async fn an_import_is_local_only_and_a_real_one_needs_cp_serve() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let report = write_report(tmp.path()).display().to_string();
    let addr = serve(tmp.path(), true, None).await;
    let (status, body) = post(
        addr,
        "/api/findings/import?host=peer",
        json!({"paths": [report], "dry_run": true}),
    )
    .await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("local-only"),
        "{body}"
    );

    let read_only = serve(tmp.path(), false, None).await;
    let (status, body) = post(
        read_only,
        "/api/findings/import",
        json!({"paths": [report]}),
    )
    .await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("rupu cp serve"),
        "{body}"
    );
}

// ── coverage gap ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_gap_lists_each_concerns_unasserted_in_scope_files() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = serve(tmp.path(), false, None).await;
    let (status, body) = get(addr, &format!("/api/coverage/{TARGET}/gap?ws_id=ws1")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({
            "kind": "coverage_gaps",
            "version": 1,
            "rows": [
                {"concern_id": "authz", "file": "src/routes/share.rs"},
                {"concern_id": "logging", "file": "README.md"},
                {"concern_id": "logging", "file": "src/routes/notes.rs"},
                {"concern_id": "logging", "file": "src/routes/share.rs"},
            ],
        })
    );
    // What `rupu coverage gap` prints is the same computation.
    let paths = CoveragePaths::new(&repo(tmp.path()), TARGET);
    assert_eq!(
        body,
        serde_json::to_value(rupu_coverage::run_audit(&paths).unwrap().gaps()).unwrap()
    );
}

#[tokio::test]
async fn the_gap_of_an_unknown_target_is_404_and_a_remote_host_is_501() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = serve(tmp.path(), false, None).await;
    let (status, _) = get(addr, "/api/coverage/nope/gap").await;
    assert_eq!(status, 404);
    let (status, body) = get(addr, &format!("/api/coverage/{TARGET}/gap?host=peer")).await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("local-only"),
        "{body}"
    );
}

// ── coverage rerun ───────────────────────────────────────────────────────

#[tokio::test]
async fn a_rerun_starts_the_recorded_agent_run_again_in_its_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let launcher = Arc::new(CapturingLauncher::default());
    let addr = serve(tmp.path(), true, Some(launcher.clone())).await;
    let (status, body) = post(
        addr,
        &format!("/api/coverage/{TARGET}/rerun"),
        json!({"run": RUN}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        body,
        json!({
            "run_id": "run_01JREPLAYED0000000000000001",
            "host_id": "local",
            "agent": "notes-reviewer",
            "target_id": TARGET,
            "ws_id": "ws1",
            "replays": RUN,
        })
    );
    let seen = launcher.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert_eq!(req.agent, "notes-reviewer");
    assert_eq!(
        req.prompt.as_deref(),
        Some("Review the notes API for access-control gaps.")
    );
    assert_eq!(req.mode.as_deref(), Some("readonly"));
    assert_eq!(
        req.working_dir.as_deref(),
        Some(repo(tmp.path()).to_str().unwrap())
    );
    assert_eq!(req.skills, ["http-probing"]);
    assert_eq!(req.preload_skills, ["house-style"]);
    assert!(req.target.is_none() && req.run_id.is_none() && req.codename.is_none());
    assert!(req.engagement_profiles.is_empty() && req.findings_profile.is_none());
    assert_eq!(req.messaging, rupu_runtime::argv::LaunchMessaging::Default);
}

#[tokio::test]
async fn a_rerun_refuses_what_it_cannot_replay() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let launcher = Arc::new(CapturingLauncher::default());
    let addr = serve(tmp.path(), true, Some(launcher.clone())).await;
    let rerun = |target: &str| format!("/api/coverage/{target}/rerun");
    let cases = [
        (rerun("nope"), RUN, 404, "coverage target nope not found"),
        (
            rerun(TARGET),
            "run_unknown",
            404,
            "no manifest for run 'run_unknown'",
        ),
        (
            rerun(TARGET),
            SESSION_RUN,
            501,
            "rerun of session runs not yet supported",
        ),
        (
            rerun(TARGET),
            CONTINUED_RUN,
            409,
            "rerun run_01JREPLAYAGENT0000000000001 instead",
        ),
        (
            rerun(TARGET),
            ELSEWHERE_RUN,
            409,
            "was recorded in workspace /srv/other-checkout",
        ),
        (
            format!("{}?host=peer", rerun(TARGET)),
            RUN,
            501,
            "local-only",
        ),
    ];
    for (path, run, want, says) in cases {
        let (status, body) = post(addr, &path, json!({"run": run})).await;
        assert_eq!(status, want, "{path} {run}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains(says),
            "{path} {run}: {body}"
        );
    }
    assert!(launcher.seen.lock().unwrap().is_empty());

    // A control plane that is not `rupu cp serve` launches nothing.
    let read_only = serve(tmp.path(), false, None).await;
    let (status, body) = post(read_only, &rerun(TARGET), json!({"run": RUN})).await;
    assert_eq!(status, 501, "{body}");
}

// ── query help ───────────────────────────────────────────────────────────

fn fixture(path: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn each_lists_fields_are_its_pinned_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = serve(tmp.path(), false, None).await;
    let cases = [
        (
            "findings",
            "../rupu-coverage/tests/fixtures/finding_query/fields.json",
        ),
        (
            "workflow_runs",
            "tests/fixtures/list_query/workflow_runs.fields.json",
        ),
        (
            "agent_runs",
            "tests/fixtures/list_query/agent_runs.fields.json",
        ),
        ("sessions", "tests/fixtures/list_query/sessions.fields.json"),
        (
            "autoflow_runs",
            "tests/fixtures/list_query/autoflow_runs.fields.json",
        ),
        ("events", "tests/fixtures/list_query/events.fields.json"),
    ];
    for (list, pinned) in cases {
        let (status, body) = get(addr, &format!("/api/query/{list}/fields")).await;
        assert_eq!(status, 200, "{list}");
        assert_eq!(
            body,
            json!({"list": list, "fields": fixture(pinned)}),
            "{list}"
        );
    }
    let (status, body) = get(addr, "/api/query/nope/fields").await;
    assert_eq!(status, 404);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("findings, workflow_runs"),
        "{body}"
    );
}

#[tokio::test]
async fn a_check_answers_the_terms_or_the_lists_own_error() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = serve(tmp.path(), false, None).await;
    let (status, body) = post(
        addr,
        "/api/query/findings/check",
        json!({"q": "sev>=high -tag:Needs-POC login"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"ok": true, "terms": [
            {"neg": false, "key": "severity", "op": "ge", "values": ["high"]},
            {"neg": true, "key": "tag", "op": "eq", "values": ["needs-poc"]},
            {"neg": false, "key": "text", "op": "eq", "values": ["login"]},
        ]})
    );
    let (status, body) = post(
        addr,
        "/api/query/sessions/check",
        json!({"q": "status:running,idle workspace:notes"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"ok": true, "terms": [
            {"neg": false, "key": "status", "op": "eq", "values": ["running", "idle"]},
            {"neg": false, "key": "project", "op": "eq", "values": ["notes"]},
        ]})
    );
    // A query the list would refuse: the same body as the list's 400, with
    // `ok: false`, and the check itself answers 200.
    let (status, body) = post(
        addr,
        "/api/query/workflow_runs/check",
        json!({"q": "status:running colour:red"}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (list_status, list_body) = get(
        addr,
        "/api/runs/workflows?q=status%3Arunning%20colour%3Ared",
    )
    .await;
    assert_eq!(list_status, 400);
    let mut want = list_body.clone();
    want["ok"] = json!(false);
    assert_eq!(body, want);
    assert_eq!(body["token"], 1);
    assert_eq!(body["code"], "unknown_key");

    let (status, body) = post(addr, "/api/query/findings/check", json!({"q": ""})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "terms": []})));
    let (status, _) = post(addr, "/api/query/nope/check", json!({"q": "x"})).await;
    assert_eq!(status, 404);
}
