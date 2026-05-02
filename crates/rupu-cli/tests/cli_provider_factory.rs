use rupu_auth::JsonFileBackend;
use rupu_cli::provider_factory::build_for_provider;

fn fresh_backend() -> JsonFileBackend {
    let tmp = assert_fs::TempDir::new().unwrap();
    JsonFileBackend {
        path: tmp.path().join("auth.json"),
    }
}

#[tokio::test]
async fn anthropic_factory_requires_credential() {
    // No stored secret; no env var. Should fail with a clear message.
    // Defensively unset the env var since tests share process state.
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let backend = fresh_backend();
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", &backend).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("anthropic") || err.contains("credential"),
        "expected clear missing-credential error, got: {err}"
    );
}

#[tokio::test]
async fn unknown_provider_errors_clearly() {
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let backend = fresh_backend();
    let res = build_for_provider("teleport", "model-x", &backend).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("teleport"),
        "expected provider name in error: {err}"
    );
}

#[tokio::test]
async fn deferred_provider_returns_blocked_error() {
    // openai/copilot/gemini/local are defined types but v0 wires only
    // anthropic. Expect a clear "not wired in v0" error.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let backend = fresh_backend();
    for p in ["openai", "copilot", "gemini", "local"] {
        let res = build_for_provider(p, "x", &backend).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }
}
