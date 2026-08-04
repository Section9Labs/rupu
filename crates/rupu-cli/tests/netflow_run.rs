//! Two different claims, two different tests — do not conflate them.
//!
//! `a_flow_reaches_both_the_ledger_and_the_transcript` proves that
//! `FanoutSink` + `TranscriptSink` + `NetflowWriterHandle` compose
//! correctly: a flow recorded through a hand-built `FanoutSink` (passed
//! explicitly to `rupu_netflow::http::client_with`) lands in the ledger,
//! the transcript, AND a plain observer. It does NOT exercise `rupu run`,
//! `rupu_netflow::http::init`, or the process-wide `OnceLock` at all — it
//! would stay green even if `rupu-cli` never called `init()` anywhere.
//!
//! `rupu_run_through_a_real_http_client_populates_the_project_rooted_ledger`
//! is the one that actually proves the wiring: it drives the real
//! `rupu_cli::run(...)` entry point end to end — real `run_inner`, real
//! `rupu_netflow::http::init(...)` call, a real (migrated)
//! `OpenAiCompatibleClient` provider constructed by the real
//! `provider_factory`, making a real HTTP request against a local
//! `httpmock` server — and asserts the resulting ledger file exists,
//! rooted at the *project*, not the invoking (nested) `pwd`. This is the
//! test that would catch a regression moving `init()` after the provider
//! build, or reverting `netflow_root` back to raw `pwd`.

use assert_fs::prelude::*;
use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Proves `FanoutSink`/`TranscriptSink`/`NetflowWriterHandle` compose —
/// see the module doc comment for what this test does and does NOT prove.
#[tokio::test]
async fn a_flow_reaches_both_the_ledger_and_the_transcript() {
    use rupu_netflow::{FanoutSink, NetflowPaths, NetflowWriterHandle};
    use rupu_transcript::{JsonlWriter, TranscriptSink};

    let tmp = tempfile::TempDir::new().unwrap();
    let transcript = tmp.path().join("transcript.jsonl");
    JsonlWriter::create(&transcript).unwrap();

    let paths = NetflowPaths::new(tmp.path());
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let observed = Arc::new(MemorySink::default());
    let fanout = FanoutSink::new(vec![
        handle.writer.clone(),
        Arc::new(TranscriptSink::new(transcript.clone())),
        observed.clone(),
    ]);

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/ping");
            then.status(200).body("pong");
        })
        .await;

    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-x".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        Arc::new(fanout),
    )
    .unwrap();

    client.get(server.url("/ping")).send().await.unwrap();
    handle.shutdown().await;

    // Ledger
    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].ctx.run_id.as_deref(), Some("run-x"));

    // Transcript
    let text = std::fs::read_to_string(&transcript).unwrap();
    assert!(text.contains(r#""type":"net_flow""#));

    // And the observer saw it too — fanout reaches every child.
    assert_eq!(observed.records().len(), 1);
}

/// The decisive test: drives the REAL `rupu run` entry point (not a
/// hand-built sink) against a real, migrated `OpenAiCompatibleClient`
/// provider talking to a local `httpmock` server, and asserts the ledger
/// gets populated at the project root — not at the nested directory
/// `rupu run` was actually invoked from.
///
/// `rupu_netflow::http::init` is process-wide and first-call-wins; this
/// is the only test in this binary that calls `rupu_cli::run` (the sink
/// composition test above never touches the global one), so there is no
/// cross-test race — matches the precedent in
/// `rupu-providers/tests/netflow_capture.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_through_a_real_http_client_populates_the_project_rooted_ledger() {
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
    // No `provider:`/`model:` pinned — resolves through
    // `[providers.netflowmock]` below, same path
    // `dispatch_resolves_openai_compatible_provider_default_model`
    // (crates/rupu-cli/src/cmd/dispatch.rs) already exercises.
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

    // Project root: a `.rupu/` marker directory is all `project_root_for`
    // requires. `rupu run` is invoked from a NESTED subdirectory of it —
    // the exact "common invocation pattern" fix-round-1 Finding 1 flagged.
    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu").create_dir_all().unwrap();
    let nested_pwd = project.child("src/deep/nested");
    nested_pwd.create_dir_all().unwrap();

    // Hermetic credential store — a nonexistent file is a valid empty
    // store (`KeychainResolver::read_file_map`), so `netflowmock`'s
    // credential resolves purely from the `RUPU_NETFLOWMOCK_API_KEY`
    // env-var fallback below, never touching a real keychain or a real
    // `~/.rupu/auth.json`.
    let auth_file = tmp.child("auth.json");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_AUTH_FILE", auth_file.path());
    std::env::set_var("RUPU_NETFLOWMOCK_API_KEY", "test-key");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::set_current_dir(nested_pwd.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_NETFLOWMOCK_API_KEY");
    std::env::remove_var("RUPU_AUTH_FILE");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu run should exit 0 against the mocked openai-compatible endpoint"
    );

    // Poll for the background ledger writer task to land the write —
    // `rupu run` (like `build_netflow_sink`) doesn't wait on an explicit
    // ledger `shutdown()` before returning.
    let ledger_path = project.path().join(".rupu/netflow/flows.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let text = loop {
        if let Ok(t) = std::fs::read_to_string(&ledger_path) {
            if !t.trim().is_empty() {
                break t;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "netflow ledger at {ledger_path:?} was never written"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };

    assert!(
        text.contains(r#""path":"/v1/chat/completions""#),
        "expected the real provider request in the ledger, got: {text}"
    );
    // `OpenAiCompatibleClient::new` attributes every openai-compatible
    // provider generically as `"openai_compatible"` regardless of the
    // config-declared name ("netflowmock" here) — see
    // `crates/rupu-providers/src/openai_compatible.rs`.
    assert!(
        text.contains(r#""kind":"provider","name":"openai_compatible""#),
        "expected provider-attributed origin, got: {text}"
    );

    // The ledger must NOT fragment into the nested invocation directory.
    assert!(
        !nested_pwd.path().join(".rupu/netflow/flows.jsonl").exists(),
        "netflow must not fragment into the nested pwd when a project root exists"
    );
}
