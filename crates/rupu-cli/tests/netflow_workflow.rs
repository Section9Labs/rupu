//! Proves the gap this plan closes for workflow runs specifically: before
//! `step_factory.rs`'s call site threaded a real per-run sink into
//! `provider_factory::build_for_provider_with_config`, a workflow agent
//! step's outbound HTTP was captured NOWHERE — the provider client was
//! built with a placeholder `Arc::new(NullSink)`. Modeled on
//! `crates/rupu-cli/tests/netflow_run.rs`'s
//! `rupu_run_through_a_real_http_client_populates_the_project_rooted_ledger`
//! (same `RUPU_HOME` / auth-file / `OpenAiCompatibleClient`-against-
//! `httpmock` setup, reused wholesale), but drives `rupu workflow run`
//! instead of `rupu run` and asserts on THAT run's own ledger file.

use assert_fs::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Drives a real workflow (one agent step) through `rupu workflow run`,
/// with the step's provider pointed at a local `httpmock` server, and
/// asserts the flow lands in that run's own `<run_id>.jsonl` ledger.
///
/// Workflow runs captured nothing before this plan — this is the gap it
/// exists to close.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_agent_step_records_flows_to_its_own_ledger() {
    let _guard = ENV_LOCK.lock().await;

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "id": "cmpl_1",
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hello from mock"},
                        "finish_reason": "stop"
                    }]
                }));
        })
        .await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global.child("workflows").create_dir_all().unwrap();

    // Same pattern as `netflow_run.rs`: no `provider:`/`model:` pinned on
    // the agent — resolves through `[providers.netflowmock]` below.
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: netflowmock\nmaxTurns: 1\n---\nyou echo.")
        .unwrap();
    global
        .child("config.toml")
        .write_str(&format!(
            "[providers.netflowmock]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\ndefault_model = \"mock-model\"\nstream = false\n",
            server.base_url(),
        ))
        .unwrap();
    global
        .child("workflows/netflow-demo.yaml")
        .write_str(
            "name: netflow-demo\n\
             description: single agent step, for netflow capture coverage\n\
             steps:\n\
             \x20\x20- id: say_hi\n\
             \x20\x20\x20\x20agent: echo\n\
             \x20\x20\x20\x20prompt: \"say hi\"\n",
        )
        .unwrap();

    // A plain tempdir with no `.rupu/` marker anywhere in its ancestry —
    // `project_root_for` resolves to `None`, so the run's ledger lands at
    // the global fallback `<global>/netflow/<run_id>.jsonl` (same rule
    // `paths::netflow_dir` applies everywhere else; not exercising the
    // project-vs-global routing itself is `netflow_run.rs`'s job, not
    // this test's).
    let pwd = assert_fs::TempDir::new().unwrap();

    // Hermetic credential store, mirrors `netflow_run.rs`.
    let auth_file = tmp.child("auth.json");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_AUTH_FILE", auth_file.path());
    std::env::set_var("RUPU_NETFLOWMOCK_API_KEY", "test-key");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::set_current_dir(pwd.path()).unwrap();

    let run_id = "run-netflow-workflow-fixed-id";

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "netflow-demo".into(),
        "--mode".into(),
        "bypass".into(),
        "--plain".into(),
        "--run-id".into(),
        run_id.into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_NETFLOWMOCK_API_KEY");
    std::env::remove_var("RUPU_AUTH_FILE");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu workflow run should exit 0 against the mocked openai-compatible endpoint"
    );

    // Poll for the background ledger writer task to land the write —
    // same reasoning as `netflow_run.rs`.
    // The workflow's own run id (pinned via `--run-id` above) names its
    // `run.json`/`events.jsonl`/`step_results.jsonl` — but each DISPATCHED
    // STEP mints its own fresh run id for its own transcript AND its own
    // netflow ledger file (`runner.rs`'s concurrent DAG scheduler:
    // `dispatch_one`'s `run_id`/`transcript_path` are per-step, not the
    // parent workflow's — see `step_factory.rs`'s `step_netflow_sink`,
    // which builds this run's sink from exactly that per-step id). So
    // this test does not predict a single ledger filename; it scans every
    // `.jsonl` file `netflow_dir` now holds for the one the step's real
    // HTTP request landed in — proving capture happened somewhere under
    // this workflow run, without over-pinning an internal id-minting
    // detail that isn't this test's job to lock down.
    let netflow_dir = global.path().join("netflow");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let text = loop {
        if let Some(t) =
            find_ledger_line_containing(&netflow_dir, r#""path":"/v1/chat/completions""#)
        {
            break t;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no netflow ledger file under {netflow_dir:?} ever recorded the workflow step's \
             /v1/chat/completions request — directory contents: {:?}",
            std::fs::read_dir(&netflow_dir)
                .map(|e| e.flatten().map(|e| e.path()).collect::<Vec<_>>())
                .unwrap_or_default()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };

    assert!(
        text.contains(r#""kind":"provider","name":"openai_compatible""#),
        "expected provider-attributed origin, got: {text}"
    );

    // `--run-id` really drove this invocation (not a coincidentally
    // passing empty run under some other id).
    assert!(
        global
            .path()
            .join("runs")
            .join(run_id)
            .join("run.json")
            .is_file(),
        "expected a persisted run.json under the pinned --run-id {run_id}"
    );
}

/// Scan every `.jsonl` file directly under `dir` (skipping the
/// self-ignoring `.gitignore` `NetflowPaths::ensure_dir` drops in) for one
/// whose contents contain `needle`, returning that file's full text.
fn find_ledger_line_containing(dir: &std::path::Path, needle: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        text.contains(needle).then_some(text)
    })
}
